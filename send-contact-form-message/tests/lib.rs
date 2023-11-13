mod fake_smtp;

use crate::fake_smtp::{setup_smtp, SMTP_PORT};
use aws_config::SdkConfig;
use aws_sdk_lambda::{
    primitives::Blob,
    types::{Environment, FunctionCode, FunctionConfiguration, Runtime, State},
};
use googletest::prelude::*;
use serde::Deserialize;
use simplelog::{ColorChoice, CombinedLogger, Config, LevelFilter, TermLogger, TerminalMode};
use std::{process::Command, time::Duration};
use testcontainers::{clients::Cli, core::WaitFor, GenericImage, RunnableImage};
use tokio::time::{sleep, timeout};
use url::Url;

#[googletest::test]
#[tokio::test]
async fn sends_email_to_recipient() -> Result<()> {
    setup_logging();
    let config = setup_aws().await;
    setup_secrets(&config).await;
    let rx = setup_smtp();
    let (lambda_client, function_name) = setup_lambda(&config).await;
    const PAYLOAD: &str = r#"{"headers":{"Content-Type":"application/json"},"body":"{\"name\":\"Arbitrary sender\",\"email\":\"email@example.com\",\"subject\":\"Test\",\"body\":\"Test message\",\"language\":\"en\",\"frc-captcha-solution\":\"arbitrary captcha solution\"}"}"#;

    let output = lambda_client
        .invoke()
        .function_name(function_name)
        .payload(Blob::new(PAYLOAD))
        .send()
        .await;

    verify_that!(output, ok(anything()))?;
    verify_that!(
        String::from_utf8(output.unwrap().payload.unwrap().into_inner()),
        ok(not(contains_substring("errorMessage")))
    )?;
    verify_that!(
        timeout(Duration::from_secs(10), rx).await,
        ok(ok(contains_substring("Test message")))
    )
}

fn setup_logging() {
    CombinedLogger::init(vec![TermLogger::new(
        LevelFilter::Trace,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    )])
    .unwrap();
}

async fn setup_aws() -> SdkConfig {
    let aws_endpoint = get_aws_endpoint_url();
    aws_config::from_env()
        .endpoint_url(&aws_endpoint)
        .load()
        .await
}

fn get_aws_endpoint_url() -> String {
    if let Ok(_) = std::env::var("USE_RUNNING_LOCALSTACK") {
        get_endpoint_url_from_running_localstack()
    } else {
        run_localstack_returning_endpoint_url()
    }
}

fn get_endpoint_url_from_running_localstack() -> String {
    #[derive(Deserialize)]
    struct LocalStackStatus {
        container_ip: String,
    }
    let localstack_status: LocalStackStatus = serde_json::from_slice(
        &Command::new("localstack")
            .args(["status", "docker", "-f", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    format!("http://{}:4566", localstack_status.container_ip)
}

fn run_localstack_returning_endpoint_url() -> String {
    let docker = Cli::default();
    let container = docker.run(
        RunnableImage::from(
            GenericImage::new("localstack/localstack", "2.3.2")
                .with_volume("/var/run/docker.sock", "/var/run/docker.sock")
                .with_wait_for(WaitFor::Healthcheck),
        )
        .with_network("bridge")
        .with_env_var(("DEBUG", "1")),
    );
    format!(
        "http://{}:{}",
        container.get_bridge_ip_address(),
        container.get_host_port_ipv4(4566)
    )
}

async fn setup_secrets(config: &SdkConfig) {
    let secrets_client = aws_sdk_secretsmanager::Client::new(config);
    provision_secret(
        &secrets_client,
        "smtp-ses-credentials",
        r#"{
            "SMTP_USERNAME": "fake SMTP username",
            "SMTP_PASSWORD": "fake SMTP password"
        }"#,
    )
    .await;
    provision_secret(
        &secrets_client,
        "friendlycaptcha-data",
        r#"{
            "FRIENDLYCAPTCHA_SITEKEY": "fake sitekey",
            "FRIENDLYCAPTCHA_SECRET": "fake secret"
        }"#,
    )
    .await;
}

async fn provision_secret(
    secrets_client: &aws_sdk_secretsmanager::Client,
    name: &str,
    content: &str,
) {
    let _ = secrets_client
        .create_secret()
        .name(name)
        .secret_string(content)
        .send()
        .await;
}

async fn setup_lambda(config: &SdkConfig) -> (aws_sdk_lambda::Client, String) {
    let lambda_client = aws_sdk_lambda::Client::new(&config);
    let create_function_result = lambda_client
        .create_function()
        .function_name("send-contact-form-message")
        .runtime(Runtime::Providedal2)
        .code(build_function_code())
        .role("arn:aws:iam::000000000000:role/localstack-does-not-care")
        .environment(build_lambda_environment(config))
        .send()
        .await
        .unwrap();
    let function_name = create_function_result.function_name().unwrap();
    wait_for_lambda_to_be_ready(&lambda_client, function_name).await;
    (lambda_client, function_name.into())
}

fn build_function_code() -> FunctionCode {
    FunctionCode::builder()
        .s3_bucket("hot-reload")
        .s3_key(format!(
            "{}/target/lambda/send-contact-form-message",
            std::env::current_dir().unwrap().to_string_lossy()
        ))
        .build()
}

fn build_lambda_environment(config: &SdkConfig) -> Environment {
    let container_host = get_container_host(config);
    Environment::builder()
        .variables(
            "AWS_ENDPOINT_URL",
            config.endpoint_url().unwrap_or_default(),
        )
        .variables("SMTP_URL", format!("smtp://172.17.0.1:{SMTP_PORT}"))
        .variables(
            "FRIENDLYCAPTCHA_VERIFY_URL",
            // TODO
            format!("http://{container_host}:12000"),
        )
        .build()
}

fn get_container_host(config: &SdkConfig) -> String {
    let endpoint_url = Url::parse(config.endpoint_url().unwrap_or_default()).unwrap();
    endpoint_url.host_str().unwrap().into()
}

async fn wait_for_lambda_to_be_ready(lambda_client: &aws_sdk_lambda::Client, function_name: &str) {
    let mut configuration = None;
    while configuration.is_none() {
        sleep(Duration::from_millis(100)).await;
        let function = lambda_client
            .get_function()
            .function_name(function_name)
            .send()
            .await
            .unwrap();
        let new_configuration = function.configuration().unwrap();
        configuration = if new_configuration.state() != Some(&State::Pending) {
            Some(new_configuration.clone())
        } else {
            None
        }
    }
    assert_that!(
        configuration,
        some(matches_pattern!(FunctionConfiguration {
            state: some(eq(State::Active)),
            state_reason: none()
        }))
    );
}
