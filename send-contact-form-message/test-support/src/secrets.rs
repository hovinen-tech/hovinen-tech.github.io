use crate::localstack_config::LocalStackConfig;

pub async fn setup_secrets(
    config: &LocalStackConfig,
    friendlycaptcha_sitekey: &str,
    friendlycaptcha_secret: &str,
) {
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
                "FRIENDLYCAPTCHA_SITEKEY": "{friendlycaptcha_sitekey}",
                "FRIENDLYCAPTCHA_SECRET": "{friendlycaptcha_secret}"
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
