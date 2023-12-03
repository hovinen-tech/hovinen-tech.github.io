use crate::EnvironmentError;
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use serde::de::DeserializeOwned;

#[async_trait]
pub trait SecretRepository {
    async fn open() -> Self;

    async fn get_secret<T: DeserializeOwned>(
        &self,
        name: &'static str,
    ) -> Result<T, lambda_http::Error>;
}

pub struct AwsSecretsManagerSecretRepository(aws_sdk_secretsmanager::Client);

#[async_trait]
impl SecretRepository for AwsSecretsManagerSecretRepository {
    async fn open() -> Self {
        let mut loader = aws_config::defaults(BehaviorVersion::latest()).region("eu-north-1");
        if let Ok(url) = std::env::var("AWS_ENDPOINT_URL") {
            loader = loader.endpoint_url(url);
        }
        let config = loader.load().await;
        let secrets_client = aws_sdk_secretsmanager::Client::new(&config);
        Self(secrets_client)
    }

    async fn get_secret<T: DeserializeOwned>(
        &self,
        name: &'static str,
    ) -> Result<T, lambda_http::Error> {
        let secret = self.0.get_secret_value().secret_id(name).send().await?;
        let Some(secret_value) = secret.secret_string() else {
            return Err(Box::new(EnvironmentError::MissingSecret(name)));
        };
        Ok(serde_json::from_str(secret_value)?)
    }
}

#[cfg(test)]
pub mod test_support {
    use std::collections::HashMap;

    use super::SecretRepository;
    use crate::{FRIENDLYCAPTCHA_DATA_NAME, SMTP_CREDENTIALS_NAME};
    use async_trait::async_trait;
    use aws_sdk_secretsmanager::types::error::ResourceNotFoundException;
    use serde::de::DeserializeOwned;

    pub const FAKE_FRIENDLYCAPTCHA_SITEKEY: &str = "arbitrary sitekey";
    pub const FAKE_FRIENDLYCAPTCHA_SECRET: &str = "arbitrary secret";

    pub struct FakeSecretRepsitory(HashMap<&'static str, String>);

    impl FakeSecretRepsitory {
        pub fn remove_secret(&mut self, name: &'static str) {
            self.0.remove(name);
        }

        pub fn add_secret(&mut self, name: &'static str, value: impl Into<String>) {
            self.0.insert(name, value.into());
        }
    }

    #[async_trait]
    impl SecretRepository for FakeSecretRepsitory {
        async fn open() -> Self {
            Self(HashMap::from([
                (
                    SMTP_CREDENTIALS_NAME,
                    r#"{
                        "SMTP_USERNAME": "fake SMTP username",
                        "SMTP_PASSWORD": "fake SMTP password"
                    }"#
                    .into(),
                ),
                (
                    FRIENDLYCAPTCHA_DATA_NAME,
                    format!(
                        r#"{{
                            "FRIENDLYCAPTCHA_SITEKEY": "{FAKE_FRIENDLYCAPTCHA_SITEKEY}",
                            "FRIENDLYCAPTCHA_SECRET": "{FAKE_FRIENDLYCAPTCHA_SECRET}"
                        }}"#
                    ),
                ),
            ]))
        }

        async fn get_secret<T: DeserializeOwned>(
            &self,
            name: &'static str,
        ) -> std::result::Result<T, lambda_http::Error> {
            let string_value = self.0.get(name).ok_or(Box::new(
                aws_sdk_secretsmanager::Error::ResourceNotFoundException(
                    ResourceNotFoundException::builder()
                        .message(format!("No such secret {name}"))
                        .build(),
                ),
            ))?;
            Ok(serde_json::from_str(string_value)?)
        }
    }
}
