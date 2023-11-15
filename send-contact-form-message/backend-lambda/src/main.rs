mod secrets;

use lambda_http::{run, service_fn, Body, Error, Request, RequestPayloadExt, Response};
use lazy_static::lazy_static;
use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::authentication::{Credentials, Mechanism},
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use reqwest::Client;
use secrets::{AwsSecretsManagerSecretRepository, SecretRepository};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{borrow::Cow, fmt::Display, future::Future, marker::PhantomData, sync::Arc};
use tokio::sync::Mutex;
use tracing::warn;

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

#[derive(Debug)]
enum MessageError {
    MissingFieldsInRequest,
    BadEmail(String),
    BadMessage(lettre::error::Error),
    SendError(lettre::transport::smtp::Error),
    MissingPayload,
    FriendlyCaptchaTokenError(Vec<String>),
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageError::MissingFieldsInRequest => write!(f, "Missing fields in request"),
            MessageError::BadEmail(email) => write!(f, "Bad email: {email}"),
            MessageError::BadMessage(error) => write!(f, "Error building message: {error}"),
            MessageError::SendError(error) => write!(f, "Error sending message: {error}"),
            MessageError::MissingPayload => write!(f, "Event message is missing a payload"),
            MessageError::FriendlyCaptchaTokenError(errors) => {
                write!(f, "FriendlyCaptcha token did not validate: {errors:?}")
            }
        }
    }
}

impl std::error::Error for MessageError {}

struct ContactFormMessageHandler<SecretRepositoryT: SecretRepository>(
    PhantomData<SecretRepositoryT>,
);

impl<SecretRepositoryT: SecretRepository> ContactFormMessageHandler<SecretRepositoryT> {
    async fn handle(event: Request) -> Result<Response<Body>, Error> {
        let Some(message) = event.payload()? else {
            return Err(Box::new(MessageError::MissingPayload));
        };
        match Self::send_message(message).await {
            Ok(language) => Ok(Response::builder()
                .status(303)
                .header("Location", Self::create_success_url(language.as_str()))
                .body("".into())
                .unwrap()),
            Err(MessageError::MissingFieldsInRequest) => Ok(Response::builder()
                .status(400)
                .body("Malformed request: missing fields".into())
                .unwrap()),
            Err(MessageError::FriendlyCaptchaTokenError(errors)) => Ok(Response::builder()
                .status(400)
                .body(format!("Captcha verification error: {errors:?}").into())
                .unwrap()),
            // TODO: This can happen without a bug on the contact form, so redirect to an error page.
            Err(MessageError::BadEmail(email)) => Ok(Response::builder()
                .status(400)
                .body(format!("Malformed request: bad email address {email}").into())
                .unwrap()),
            Err(error) => Err(Box::new(error)),
        }
    }

    async fn send_message(message: ContactFormMessage) -> Result<String, MessageError> {
        let ContactFormMessage {
            name,
            email: Some(email),
            subject: Some(subject),
            body: Some(body),
            language: Some(language),
            friendlycaptcha_token: Some(friendlycaptcha_token),
        } = message
        else {
            return Err(MessageError::MissingFieldsInRequest);
        };
        Self::verify_friendlycaptcha_token(friendlycaptcha_token).await?;
        let reply_to_string = if let Some(name) = name {
            format!("{} <{}>", name, email)
        } else {
            email.clone()
        };
        let Ok(reply_to_email) = reply_to_string.parse() else {
            return Err(MessageError::BadEmail(email));
        };
        let email = Message::builder()
            .from(FROM_ADDRESS.clone())
            .reply_to(reply_to_email)
            .to(TO_ADDRESS.clone())
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body)
            .map_err(MessageError::BadMessage)?;
        let mailer = get_memoized(&MAILER, Self::initialise_mailer)
            .await
            .unwrap();
        match mailer.send(email).await {
            Ok(_) => Ok(language),
            Err(e) => Err(MessageError::SendError(e)),
        }
    }

    async fn verify_friendlycaptcha_token(solution: String) -> Result<(), MessageError> {
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
                        panic!("Client error when communicating to FriendlyCaptcha.");
                    }
                }
                warn!("Error verifying FriendlyCaptcha solution: {error}");
                warn!("Letting request pass without verification.");
                return Ok(());
            }
        };
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
        } else {
            Err(MessageError::FriendlyCaptchaTokenError(
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

#[derive(Deserialize)]
struct SmtpCredentials {
    #[serde(rename = "SMTP_USERNAME")]
    username: String,
    #[serde(rename = "SMTP_PASSWORD")]
    password: String,
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
    use test_support::{fake_friendlycaptcha::FakeFriendlyCaptcha, fake_smtp::FakeSmtpServer};
    use tokio::time::timeout;

    type ContactFormMessageHandlerForTesting = ContactFormMessageHandler<FakeSecretRepsitory>;

    const CORRECT_CAPTCHA_SOLUTION: &str = "correct captcha solution";

    lazy_static! {
        static ref FAKE_SMTP: FakeSmtpServer = FakeSmtpServer::new();
    }

    #[tokio::test]
    #[serial]
    async fn returns_400_when_captcha_solution_does_not_validate() -> Result<()> {
        init();
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
        init();
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
        init();
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
            timeout(Duration::from_secs(10), FAKE_SMTP.last_mail_content()).await,
            ok(ok(anything()))
        )
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn sends_mail_when_friendlycaptcha_sends_invalid_response() {
        init();
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
            timeout(Duration::from_secs(10), FAKE_SMTP.last_mail_content()).await,
            ok(ok(anything()))
        );
    }

    fn init() {
        setup_environment();
        FAKE_SMTP.start();
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
