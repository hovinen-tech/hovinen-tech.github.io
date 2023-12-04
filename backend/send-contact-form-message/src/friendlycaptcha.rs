use crate::{secrets::SecretRepository, ContactFormError};
use async_once_cell::OnceCell;
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use tracing::warn;

pub const FRIENDLYCAPTCHA_DATA_NAME: &str = "friendlycaptcha-data";
const FRIENDLYCAPTCHA_VERIFY_URL: &str = "https://api.friendlycaptcha.com/api/v1/siteverify";

pub struct FriendlyCaptchaVerifier<SecretRepositoryT: SecretRepository> {
    secrets_repository: SecretRepositoryT,
    friendlycaptcha_data: OnceCell<FriendlyCaptchaData>,
}

impl<SecretRepositoryT: SecretRepository> FriendlyCaptchaVerifier<SecretRepositoryT> {
    pub fn new(secrets_repository: SecretRepositoryT) -> Self {
        Self {
            secrets_repository,
            friendlycaptcha_data: Default::default(),
        }
    }

    pub async fn verify_token(&self, solution: &str) -> Result<(), FriendlyCaptchaError> {
        let payload = match self.build_payload(solution).await {
            Ok(response) => response,
            Err(FriendlyCaptchaError::BackendError) => return Ok(()),
            Err(e) => return Err(e),
        };
        let response = match Self::send_solution(payload).await {
            Ok(response) => response,
            Err(FriendlyCaptchaError::BackendError) => return Ok(()),
            Err(e) => return Err(e),
        };
        self.process_response(response).await?;
        Ok(())
    }

    async fn build_payload<'a>(
        &'a self,
        solution: &'a str,
    ) -> Result<FriendlyCaptchaVerifyPayload<'a>, FriendlyCaptchaError> {
        let data = match self
            .friendlycaptcha_data
            .get_or_try_init(
                self.secrets_repository
                    .get_secret(FRIENDLYCAPTCHA_DATA_NAME),
            )
            .await
        {
            Ok(data) => data,
            Err(error) => {
                warn!("Could not retrieve FriendlyCaptcha credentials {FRIENDLYCAPTCHA_DATA_NAME} from AWS secrets manager: {error}");
                warn!("Letting request pass without verification.");
                return Err(FriendlyCaptchaError::BackendError);
            }
        };

        Ok(FriendlyCaptchaVerifyPayload {
            solution,
            sitekey: &data.sitekey,
            secret: &data.secret,
        })
    }

    async fn send_solution<'a>(
        payload: FriendlyCaptchaVerifyPayload<'a>,
    ) -> Result<Response, FriendlyCaptchaError> {
        Ok(
            match Client::new()
                .post(Self::verification_url().as_ref())
                .json(&payload)
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    if let Some(status) = error.status() {
                        if status.is_client_error() {
                            return Err(FriendlyCaptchaError::ClientError(error));
                        }
                    }
                    warn!("Error verifying FriendlyCaptcha solution: {error}");
                    warn!("Letting request pass without verification.");
                    return Err(FriendlyCaptchaError::BackendError);
                }
            },
        )
    }

    fn verification_url() -> Cow<'static, str> {
        std::env::var("FRIENDLYCAPTCHA_VERIFY_URL")
            .map(Cow::Owned)
            .unwrap_or(FRIENDLYCAPTCHA_VERIFY_URL.into())
    }

    async fn process_response(&self, response: Response) -> Result<(), FriendlyCaptchaError> {
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(FriendlyCaptchaError::IncorrectSecret);
        }
        let response_body: FriendlyCaptchaResponse = match response.json().await {
            Ok(body) => body,
            Err(error) => {
                warn!("Error fetching body from FriendlyCaptcha: {error}");
                warn!("Letting request pass without verification.");
                return Ok(());
            }
        };
        if response_body.success {
            Ok(())
        } else if response_body.errors.iter().any(|e| e == "solution_invalid") {
            Err(FriendlyCaptchaError::SolutionInvalid)
        } else if response_body
            .errors
            .iter()
            .any(|e| e == "solution_timeout_or_duplicate")
        {
            Err(FriendlyCaptchaError::SolutionTimeoutOrDuplicate)
        } else {
            Err(FriendlyCaptchaError::UnrecognizedError(
                response_body.errors,
            ))
        }
    }
}

#[derive(Deserialize, Clone)]
struct FriendlyCaptchaData {
    #[serde(rename = "FRIENDLYCAPTCHA_SITEKEY")]
    sitekey: String,
    #[serde(rename = "FRIENDLYCAPTCHA_SECRET")]
    secret: String,
}

#[derive(Serialize)]
struct FriendlyCaptchaVerifyPayload<'a> {
    solution: &'a str,
    secret: &'a str,
    sitekey: &'a str,
}

#[derive(Deserialize)]
struct FriendlyCaptchaResponse {
    success: bool,
    #[serde(default)]
    errors: Vec<String>,
}

#[derive(Debug)]
pub enum FriendlyCaptchaError {
    ClientError(reqwest::Error),
    IncorrectSecret,
    SolutionInvalid,
    SolutionTimeoutOrDuplicate,
    UnrecognizedError(Vec<String>),
    BackendError,
}

impl FriendlyCaptchaError {
    pub fn into_contact_form_error(
        self,
        subject: String,
        body: String,
        language: String,
    ) -> ContactFormError {
        match self {
            FriendlyCaptchaError::ClientError(error) => ContactFormError::InternalError {
                description: format!("FriendlyCaptcha client error: {error}"),
                subject,
                body,
                language,
            },
            FriendlyCaptchaError::IncorrectSecret => ContactFormError::InternalError {
                description: "Incorrect FriendlyCaptcha secret".into(),
                subject,
                body,
                language,
            },
            FriendlyCaptchaError::SolutionInvalid => {
                ContactFormError::ClientError("Invalid FriendlyCaptcha solution".into())
            }
            FriendlyCaptchaError::SolutionTimeoutOrDuplicate => ContactFormError::ClientError(
                "FriendlyCaptcha solution timeout or duplicate".into(),
            ),
            FriendlyCaptchaError::UnrecognizedError(errors) => ContactFormError::InternalError {
                description: format!("FriendlyCaptcha error: {errors:?}"),
                subject,
                body,
                language,
            },
            FriendlyCaptchaError::BackendError => ContactFormError::InternalError {
                description: "FriendlyCaptcha backend error".into(),
                subject,
                body,
                language,
            },
        }
    }
}

impl std::fmt::Display for FriendlyCaptchaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FriendlyCaptchaError::ClientError(error) => write!(f, "Client error: {error}"),
            FriendlyCaptchaError::IncorrectSecret => write!(f, "Incorrect secret"),
            FriendlyCaptchaError::SolutionInvalid => write!(f, "Solution invalid"),
            FriendlyCaptchaError::SolutionTimeoutOrDuplicate => {
                write!(f, "Solution timeout or duplicate")
            }
            FriendlyCaptchaError::UnrecognizedError(errors) => {
                write!(f, "Unrecognised error: {errors:?}")
            }
            FriendlyCaptchaError::BackendError => write!(f, "FriendlyCaptcha backend error"),
        }
    }
}

impl std::error::Error for FriendlyCaptchaError {}
