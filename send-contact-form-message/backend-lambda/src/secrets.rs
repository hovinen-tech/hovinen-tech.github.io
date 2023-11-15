use async_trait::async_trait;
use serde::de::DeserializeOwned;

use crate::EnvironmentError;

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
        let mut loader = aws_config::from_env().region("eu-north-1");
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
    use super::SecretRepository;
    use crate::{FRIENDLYCAPTCHA_DATA_NAME, SMTP_CREDENTIALS_NAME};
    use async_trait::async_trait;
    use serde::de::DeserializeOwned;

    pub const FAKE_FRIENDLYCAPTCHA_SITEKEY: &str = "arbitrary sitekey";
    pub const FAKE_FRIENDLYCAPTCHA_SECRET: &str = "arbitrary secret";

    pub struct FakeSecretRepsitory;

    #[async_trait]
    impl SecretRepository for FakeSecretRepsitory {
        async fn open() -> Self {
            Self
        }

        async fn get_secret<T: DeserializeOwned>(
            &self,
            name: &'static str,
        ) -> std::result::Result<T, lambda_http::Error> {
            match name {
                SMTP_CREDENTIALS_NAME => Ok(serde_json::from_str(
                    r#"{
                        "SMTP_USERNAME": "fake SMTP username",
                        "SMTP_PASSWORD": "fake SMTP password"
                    }"#,
                )?),
                FRIENDLYCAPTCHA_DATA_NAME => Ok(serde_json::from_str(
                    format!(
                        r#"{{
                            "FRIENDLYCAPTCHA_SITEKEY": "{FAKE_FRIENDLYCAPTCHA_SITEKEY}",
                            "FRIENDLYCAPTCHA_SECRET": "{FAKE_FRIENDLYCAPTCHA_SECRET}"
                        }}"#
                    )
                    .as_str(),
                )?),
                _ => panic!("Unknown secret {name}"),
            }
        }
    }
}
