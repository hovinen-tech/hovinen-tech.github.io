use aws_sdk_lambda::{
    primitives::Blob,
    types::{Environment, FunctionCode, FunctionConfiguration, Runtime, State},
};
use googletest::prelude::*;
use regex::Regex;
use serde::Deserialize;
use simplelog::{ColorChoice, CombinedLogger, Config, LevelFilter, TermLogger, TerminalMode};
use std::{borrow::Cow, collections::HashMap, sync::Arc, time::Duration};
use test_support::{
    fake_friendlycaptcha::FakeFriendlyCaptcha,
    fake_smtp::{setup_smtp, SMTP_PORT},
    localstack_config::{LocalStackConfig, LOCALSTACK_PORT},
    HOST_IP,
};
use tokio::time::{sleep, timeout};

const FAKE_FRIENDLYCAPTCHA_SITEKEY: &str = "arbitrary sitekey";
const FAKE_FRIENDLYCAPTCHA_SECRET: &str = "arbitrary secret";

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct LambdaResponsePayload {
    status_code: Option<u32>,
    headers: HashMap<String, String>,
    error_message: Option<String>,
}

#[googletest::test]
#[tokio::test]
async fn sends_email_to_recipient() -> Result<()> {
    setup_logging();
    let config = LocalStackConfig::new().await;
    let fake_friendlycaptcha: Arc<FakeFriendlyCaptcha> =
        FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET);
    tokio::spawn(fake_friendlycaptcha.serve());
    setup_secrets(&config).await;
    let mail_content = setup_smtp();
    let (lambda_client, function_name) = setup_lambda(&config).await;
    let payload = clean(
        r#"{
            "headers": {
                "Content-Type": "application/json"
            },
            "body": "{
                \"name\":\"Arbitrary sender\",
                \"email\":\"email@example.com\",
                \"subject\":\"Test\",
                \"body\":\"Test message\",
                \"language\":\"en\",
                \"frc-captcha-solution\":\"arbitrary captcha solution\"
            }"
        }"#,
    );

    let output = lambda_client
        .invoke()
        .function_name(function_name)
        .payload(Blob::new(payload.as_bytes()))
        .send()
        .await;

    verify_that!(output, ok(anything()))?;
    verify_that!(
        serde_json::from_slice(&output.unwrap().payload.unwrap().into_inner()),
        ok(matches_pattern!(LambdaResponsePayload {
            status_code: some(eq(303)),
            headers: has_entry(
                "location".to_string(),
                eq("https://hovinen-tech.github.io/email-sent.html")
            ),
            error_message: none(),
        }))
    )?;
    verify_that!(
        timeout(Duration::from_secs(10), mail_content).await,
        ok(ok(all!(
            contains_substring("To: \"Bradford Hovinen\" <hovinen@hovinen.tech>"),
            contains_substring("From: \"Web contact form\" <noreply@hovinen.tech>"),
            contains_substring("Reply-To: \"Arbitrary sender\" <email@example.com>"),
            contains_substring("Subject: Test"),
            contains_substring("Test message")
        )))
    )
}

fn setup_logging() {
    CombinedLogger::init(vec![TermLogger::new(
        LevelFilter::Info,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    )])
    .unwrap();
}

async fn setup_secrets(config: &LocalStackConfig) {
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config.sdk_config);
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
        format!(
            r#"{{
                "FRIENDLYCAPTCHA_SITEKEY": "{FAKE_FRIENDLYCAPTCHA_SITEKEY}",
                "FRIENDLYCAPTCHA_SECRET": "{FAKE_FRIENDLYCAPTCHA_SECRET}"
            }}"#
        )
        .as_str(),
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

async fn setup_lambda(config: &LocalStackConfig) -> (aws_sdk_lambda::Client, String) {
    let lambda_client = aws_sdk_lambda::Client::new(&config.sdk_config);
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
            "{}/target/lambda/backend-lambda",
            std::env::current_dir()
                .unwrap()
                .parent() // Current directory is the workspace member, lambda was built in the
                .unwrap() // top-level workspace.
                .to_string_lossy()
        ))
        .build()
}

fn build_lambda_environment(config: &LocalStackConfig) -> Environment {
    Environment::builder()
        .variables(
            "AWS_ENDPOINT_URL",
            format!("http://{}:{LOCALSTACK_PORT}", config.aws_host_from_subject),
        )
        .variables("SMTP_URL", format!("smtp://{HOST_IP}:{SMTP_PORT}"))
        .variables(
            "FRIENDLYCAPTCHA_VERIFY_URL",
            FakeFriendlyCaptcha::verify_url(),
        )
        .build()
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

fn clean(raw: &str) -> Cow<str> {
    let line_break = Regex::new("\n +").unwrap();
    line_break.replace_all(raw, "")
}
