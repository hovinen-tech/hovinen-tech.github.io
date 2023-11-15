mod error_page;
mod secrets;

use error_page::render_error_page;
use lambda_http::{run, service_fn, Body, Error, Request, RequestPayloadExt, Response};
use lazy_static::lazy_static;
use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::authentication::{Credentials, Mechanism},
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use reqwest::{Client, StatusCode};
use secrets::{AwsSecretsManagerSecretRepository, SecretRepository};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{borrow::Cow, fmt::Display, future::Future, marker::PhantomData, sync::Arc};
use tokio::sync::Mutex;
use tracing::{error, warn};

lazy_static! {
    static ref MAILER: Mutex<Option<Arc<AsyncSmtpTransport<Tokio1Executor>>>> = Mutex::new(None);
    static ref FRIENDLYCAPTCHA_DATA: Mutex<Option<FriendlyCaptchaData>> = Mutex::new(None);
    static ref FROM_ADDRESS: Mailbox = "Web contact form <noreply@hovinen.tech>".parse().unwrap();
    static ref TO_ADDRESS: Mailbox = "Bradford Hovinen <hovinen@hovinen.tech>".parse().unwrap();
}

const SMTP_URL: &str = "smtps://email-smtp.eu-north-1.amazonaws.com";
const SMTP_CREDENTIALS_NAME: &str = "smtp-ses-credentials";

const FRIENDLYCAPTCHA_DATA_NAME: &str = "friendlycaptcha-data";
const FRIENDLYCAPTCHA_VERIFY_URL: &str = "https://api.friendlycaptcha.com/api/v1/siteverify";

const BASE_HOST: &str = "hovinen-tech.github.io";

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .without_time()
        .init();

    run(service_fn(
        ContactFormMessageHandler::<AwsSecretsManagerSecretRepository>::handle,
    ))
    .await
}

struct ContactFormMessageHandler<SecretRepositoryT: SecretRepository>(
    PhantomData<SecretRepositoryT>,
);

impl<SecretRepositoryT: SecretRepository> ContactFormMessageHandler<SecretRepositoryT> {
    async fn handle(event: Request) -> Result<Response<Body>, Error> {
        let Some(message) = event.payload()? else {
            let error = ContactFormError::InternalError {
                description: "Missing event payload".into(),
                subject: "(Unable to retrieve)".into(),
                body: "(Unable to retrieve)".into(),
                language: "en".into(),
            };
            error.log();
            return Ok(error.to_response());
        };
        match Self::send_message(message).await {
            Ok(language) => Ok(Response::builder()
                .status(303)
                .header("Location", Self::create_success_url(language.as_str()))
                .body("".into())
                .unwrap()),
            Err(error) => {
                error.log();
                Ok(error.to_response())
            }
        }
    }

    async fn send_message(message: ContactFormMessage) -> Result<String, ContactFormError> {
        let ContactFormMessage {
            name,
            email: Some(email),
            subject: Some(subject),
            body: Some(body),
            language: Some(language),
            friendlycaptcha_token: Some(friendlycaptcha_token),
        } = message
        else {
            return Err(ContactFormError::ClientError(
                "Missing fields in request".into(),
            ));
        };
        Self::verify_friendlycaptcha_token(friendlycaptcha_token)
            .await
            .map_err(|e| {
                e.to_contact_form_error(subject.clone(), body.clone(), language.clone())
            })?;
        let reply_to_string = if let Some(name) = name {
            format!("{} <{}>", name, email)
        } else {
            email.clone()
        };
        let Ok(reply_to_email) = reply_to_string.parse() else {
            return Err(ContactFormError::ClientError(format!(
                "Invalid email address {email}"
            )));
        };
        let email = Message::builder()
            .from(FROM_ADDRESS.clone())
            .reply_to(reply_to_email)
            .to(TO_ADDRESS.clone())
            .subject(subject.clone())
            .header(ContentType::TEXT_PLAIN)
            .body(body.clone())
            .map_err(|error| ContactFormError::InternalError {
                description: format!("Error building message: {error}"),
                subject: subject.clone(),
                body: body.clone(),
                language: language.clone(),
            })?;
        let mailer = get_memoized(&MAILER, Self::initialise_mailer)
            .await
            .unwrap();
        match mailer.send(email).await {
            Ok(_) => Ok(language),
            Err(error) => Err(ContactFormError::InternalError {
                description: format!("Error sending message: {error}"),
                subject,
                body,
                language,
            }),
        }
    }

    async fn verify_friendlycaptcha_token(solution: String) -> Result<(), FriendlyCaptchaError> {
        let data = get_memoized(&FRIENDLYCAPTCHA_DATA, || {
            Self::fetch_secret(FRIENDLYCAPTCHA_DATA_NAME)
        })
        .await
        .unwrap();
        let payload = FriendlyCaptchaVerifyPayload {
            solution,
            sitekey: data.sitekey,
            secret: data.secret,
        };
        let response = match Client::new()
            .post(Self::friendlycaptcha_verify_url().as_ref())
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
                return Ok(());
            }
        };
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

    fn friendlycaptcha_verify_url() -> Cow<'static, str> {
        std::env::var("FRIENDLYCAPTCHA_VERIFY_URL")
            .map(Cow::Owned)
            .unwrap_or(FRIENDLYCAPTCHA_VERIFY_URL.into())
    }

    async fn initialise_mailer() -> Result<Arc<AsyncSmtpTransport<Tokio1Executor>>, Error> {
        let smtp_url = Self::smtp_url();
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::from_url(&smtp_url)?
            .authentication(vec![Mechanism::Plain]);

        // Sending credentials over a non-TLS connection is risky, so we only set the credentials
        // when the connection URL is over TLS. If the environment is misconfigured so that
        // the credentials are not sent, the connection will be rejected. This is better than a
        // security breach.
        if smtp_url.starts_with("smtps://") {
            let parsed_credentials: SmtpCredentials =
                Self::fetch_secret(SMTP_CREDENTIALS_NAME).await?;
            builder = builder.credentials(Credentials::new(
                parsed_credentials.username,
                parsed_credentials.password,
            ));
        }

        Ok(Arc::new(builder.build()))
    }

    fn smtp_url() -> Cow<'static, str> {
        std::env::var("SMTP_URL")
            .map(Cow::Owned)
            .unwrap_or(SMTP_URL.into())
    }

    async fn fetch_secret<T: DeserializeOwned>(
        name: &'static str,
    ) -> Result<T, lambda_http::Error> {
        let repository = SecretRepositoryT::open().await;
        repository.get_secret(name).await
    }

    fn create_success_url(language: &str) -> String {
        if language == "en" {
            format!("https://{BASE_HOST}/email-sent.html")
        } else {
            format!("https://{BASE_HOST}/email-sent.{language}.html")
        }
    }
}

async fn get_memoized<T: Clone, F: Future<Output = Result<T, Error>>>(
    mutex: &Mutex<Option<T>>,
    factory: impl FnOnce() -> F,
) -> Result<T, Error> {
    let mut guard = mutex.lock().await;
    match &*guard {
        Some(data) => Ok(data.clone()),
        None => {
            let data: T = factory().await?;
            *guard = Some(data.clone());
            Ok(data)
        }
    }
}

#[derive(Deserialize, Debug)]
struct ContactFormMessage {
    name: Option<String>,
    email: Option<String>,
    subject: Option<String>,
    body: Option<String>,
    language: Option<String>,
    #[serde(rename = "frc-captcha-solution")]
    friendlycaptcha_token: Option<String>,
}

#[derive(Deserialize, Clone)]
struct FriendlyCaptchaData {
    #[serde(rename = "FRIENDLYCAPTCHA_SITEKEY")]
    sitekey: String,
    #[serde(rename = "FRIENDLYCAPTCHA_SECRET")]
    secret: String,
}

#[derive(Serialize)]
struct FriendlyCaptchaVerifyPayload {
    solution: String,
    secret: String,
    sitekey: String,
}

#[derive(Deserialize)]
struct FriendlyCaptchaResponse {
    success: bool,
    #[serde(default)]
    errors: Vec<String>,
}

#[derive(Deserialize)]
struct SmtpCredentials {
    #[serde(rename = "SMTP_USERNAME")]
    username: String,
    #[serde(rename = "SMTP_PASSWORD")]
    password: String,
}

#[derive(Debug)]
enum ContactFormError {
    InternalError {
        description: String,
        subject: String,
        body: String,
        language: String,
    },
    ClientError(String),
}

impl ContactFormError {
    fn log(&self) {
        match self {
            ContactFormError::InternalError { description, .. } => {
                error!("Internal error sending contact form email: {description}");
            }
            ContactFormError::ClientError(description) => {
                error!("Client error sending contact form email: {description}");
            }
        }
    }

    fn to_response(self) -> Response<Body> {
        match self {
            ContactFormError::InternalError {
                subject,
                body,
                language,
                ..
            } => Response::builder()
                .status(500)
                .body(render_error_page(subject.as_str(), body.as_str(), language.as_str()).into())
                .unwrap(),
            ContactFormError::ClientError(description) => Response::builder()
                .status(400)
                .body(format!("Client error: {description}").into())
                .unwrap(),
        }
    }
}

impl std::fmt::Display for ContactFormError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContactFormError::InternalError { description, .. } => {
                write!(f, "Internal error: {description}")
            }
            ContactFormError::ClientError(description) => write!(f, "Client error: {description}"),
        }
    }
}

impl std::error::Error for ContactFormError {}

#[derive(Debug)]
enum FriendlyCaptchaError {
    ClientError(reqwest::Error),
    IncorrectSecret,
    SolutionInvalid,
    SolutionTimeoutOrDuplicate,
    UnrecognizedError(Vec<String>),
}

impl FriendlyCaptchaError {
    fn to_contact_form_error(
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
                description: format!("Incorrect FriendlyCaptcha secret"),
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
        }
    }
}

impl std::error::Error for FriendlyCaptchaError {}

#[derive(Debug)]
enum EnvironmentError {
    MissingSecret(&'static str),
}

impl Display for EnvironmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvironmentError::MissingSecret(key) => write!(f, "Missing secret {key}"),
        }
    }
}

impl std::error::Error for EnvironmentError {}

#[cfg(test)]
mod tests {
    use super::ContactFormMessageHandler;
    use crate::secrets::test_support::{
        FakeSecretRepsitory, FAKE_FRIENDLYCAPTCHA_SECRET, FAKE_FRIENDLYCAPTCHA_SITEKEY,
    };
    use googletest::prelude::*;
    use lambda_http::{http::HeaderValue, Body, Request};
    use lazy_static::lazy_static;
    use serde::Serialize;
    use serial_test::serial;
    use std::time::Duration;
    use test_support::{
        fake_friendlycaptcha::FakeFriendlyCaptcha, fake_smtp::FakeSmtpServer, setup_logging,
    };
    use tokio::time::timeout;

    type ContactFormMessageHandlerForTesting = ContactFormMessageHandler<FakeSecretRepsitory>;

    const CORRECT_CAPTCHA_SOLUTION: &str = "correct captcha solution";

    lazy_static! {
        static ref FAKE_SMTP: FakeSmtpServer = FakeSmtpServer::new();
    }

    #[tokio::test]
    #[serial]
    async fn returns_400_when_captcha_solution_does_not_validate() -> Result<()> {
        init().await;
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET)
                .require_solution(CORRECT_CAPTCHA_SOLUTION);
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary()
            .with_captcha_solution("incorrect captcha solution")
            .into_event();

        let response = ContactFormMessageHandlerForTesting::handle(event)
            .await
            .unwrap();

        verify_that!(response.status().as_u16(), eq(400))
    }

    #[tokio::test]
    #[serial]
    async fn returns_400_when_captcha_solution_is_missing() -> Result<()> {
        init().await;
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET)
                .require_solution(CORRECT_CAPTCHA_SOLUTION);
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary()
            .with_no_captcha_solution()
            .into_event();

        let response = ContactFormMessageHandlerForTesting::handle(event)
            .await
            .unwrap();

        verify_that!(response.status().as_u16(), eq(400))
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn sends_mail_when_friendlycaptcha_fails() {
        init().await;
        let event = EventPayload::arbitrary().into_event();

        let response = ContactFormMessageHandlerForTesting::handle(event)
            .await
            .unwrap();

        expect_that!(response.status().as_u16(), eq(303));
        expect_that!(
            response.body(),
            points_to(matches_pattern!(Body::Text(eq(""))))
        );
        expect_that!(
            timeout(Duration::from_secs(1), FAKE_SMTP.last_mail_content()).await,
            ok(ok(anything()))
        )
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn sends_mail_when_friendlycaptcha_sends_invalid_response() {
        init().await;
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET)
                .return_invalid_response();
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary().into_event();

        let response = ContactFormMessageHandlerForTesting::handle(event)
            .await
            .unwrap();

        expect_that!(response.status().as_u16(), eq(303));
        expect_that!(
            response.body(),
            points_to(matches_pattern!(Body::Text(eq(""))))
        );
        expect_that!(
            timeout(Duration::from_secs(1), FAKE_SMTP.last_mail_content()).await,
            ok(ok(anything()))
        );
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn does_not_send_mail_when_friendlycaptcha_sends_solution_timeout() {
        init().await;
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET)
                .return_solution_timeout();
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary().into_event();

        ContactFormMessageHandlerForTesting::handle(event)
            .await
            .unwrap();

        expect_that!(
            timeout(Duration::from_secs(1), FAKE_SMTP.last_mail_content()).await,
            err(anything())
        );
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn returns_contact_page_when_friendly_captcha_reports_bad_sitekey() {
        init().await;
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new("A different sitekey", FAKE_FRIENDLYCAPTCHA_SECRET);
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary().into_event();

        let response = ContactFormMessageHandlerForTesting::handle(event)
            .await
            .unwrap();

        expect_that!(response.status().as_u16(), eq(500));
        expect_that!(
            response.body(),
            points_to(matches_pattern!(Body::Text(contains_substring(
                "Something went wrong"
            ))))
        );
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn returns_contact_page_when_friendly_captcha_reports_bad_secret() {
        setup_logging();
        init().await;
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, "A different secret");
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary().into_event();

        let response = ContactFormMessageHandlerForTesting::handle(event)
            .await
            .unwrap();

        expect_that!(response.status().as_u16(), eq(500));
        expect_that!(
            response.body(),
            points_to(matches_pattern!(Body::Text(contains_substring(
                "Something went wrong"
            ))))
        );
    }

    #[tokio::test]
    #[serial]
    async fn returns_contact_page_when_connection_to_mail_server_fails() {}

    #[tokio::test]
    #[serial]
    async fn returns_contact_page_when_smtp_fails() {}

    #[tokio::test]
    #[serial]
    async fn returns_contact_page_when_secrets_service_fails() {}

    async fn init() {
        setup_environment();
        FAKE_SMTP.start();
        FAKE_SMTP.flush().await;
    }

    fn setup_environment() {
        FakeSmtpServer::setup_environment();
        FakeFriendlyCaptcha::setup_environment();
    }

    #[derive(Serialize)]
    struct EventPayload {
        name: String,
        email: String,
        subject: String,
        body: String,
        language: String,
        #[serde(rename = "frc-captcha-solution")]
        solution: Option<String>,
    }

    impl EventPayload {
        fn arbitrary() -> Self {
            Self {
                name: "Arbitrary sender".into(),
                email: "email@example.com".into(),
                subject: "Test".into(),
                body: "Test message".into(),
                language: "en".into(),
                solution: Some(CORRECT_CAPTCHA_SOLUTION.into()),
            }
        }

        fn with_no_captcha_solution(self) -> Self {
            Self {
                solution: None,
                ..self
            }
        }

        fn with_captcha_solution(self, solution: impl AsRef<str>) -> Self {
            Self {
                solution: Some(solution.as_ref().into()),
                ..self
            }
        }

        fn into_event(self) -> Request {
            let mut event = Request::new(Body::Text(self.into_json()));
            event
                .headers_mut()
                .append("Content-Type", HeaderValue::from_static("application/json"));
            event
        }

        fn into_json(self) -> String {
            serde_json::to_string(&self).unwrap()
        }
    }
}
