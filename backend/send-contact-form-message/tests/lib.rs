use aws_sdk_lambda::{
    primitives::Blob,
    types::{Environment, FunctionCode, FunctionConfiguration, Runtime, State},
};
use googletest::prelude::*;
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};
use test_support::{
    clean_payload,
    fake_friendlycaptcha::FakeFriendlyCaptcha,
    fake_smtp::{FakeSmtpServer, SMTP_PORT},
    localstack_config::{LocalStackConfig, LOCALSTACK_PORT},
    secrets::setup_secrets,
    setup_logging, HOST_IP,
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
    let fake_friendlycaptcha =
        FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET);
    tokio::spawn(fake_friendlycaptcha.serve());
    setup_secrets(
        &config,
        FAKE_FRIENDLYCAPTCHA_SITEKEY,
        FAKE_FRIENDLYCAPTCHA_SECRET,
    )
    .await;
    let fake_smtp_server = FakeSmtpServer::new();
    fake_smtp_server.start();
    let (lambda_client, function_name) = setup_lambda(&config).await;
    let payload = clean_payload(
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
                eq("https://hovinen.tech/email-sent.html")
            ),
            error_message: none(),
        }))
    )?;
    verify_that!(
        timeout(
            Duration::from_secs(10),
            fake_smtp_server.last_mail_content()
        )
        .await,
        ok(ok(all!(
            contains_substring("To: \"Bradford Hovinen\" <bradford@hovinen.tech>"),
            contains_substring("From: \"Web contact form\" <noreply@hovinen.tech>"),
            contains_substring("Reply-To: \"Arbitrary sender\" <email@example.com>"),
            contains_substring("Subject: Test"),
            contains_substring("Test message")
        )))
    )
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
            "{}/target/lambda/send-contact-form-message",
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
