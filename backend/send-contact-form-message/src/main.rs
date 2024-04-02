mod error_page;
mod friendlycaptcha;
mod secrets;

use async_once_cell::OnceCell;
use error_page::render_error_page;
use friendlycaptcha::FriendlyCaptchaVerifier;
use lambda_http::{
    http::StatusCode, run, service_fn, Body, Error, Request, RequestPayloadExt, Response,
};
use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::authentication::{Credentials, Mechanism},
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use secrets::{AwsSecretsManagerSecretRepository, SecretRepository};
use serde::Deserialize;
use std::{borrow::Cow, fmt::Display, sync::OnceLock};
use tracing::error;

const FROM_ADDRESS: &str = "Web contact form <noreply@hovinen.tech>";
const TO_ADDRESS: &str = "Bradford Hovinen <bradford@hovinen.tech>";

const SMTP_URL: &str = "smtps://email-smtp.eu-north-1.amazonaws.com";
const SMTP_CREDENTIALS_NAME: &str = "smtp-ses-credentials";

const BASE_HOST: &str = "hovinen.tech";

static FROM_MAILBOX: OnceLock<Mailbox> = OnceLock::new();
static TO_MAILBOX: OnceLock<Mailbox> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .without_time()
        .init();

    let handler = ContactFormMessageHandler::<AwsSecretsManagerSecretRepository>::new().await;
    run(service_fn(|event| handler.handle(event))).await
}

struct ContactFormMessageHandler<SecretRepositoryT: SecretRepository> {
    secrets_repository: SecretRepositoryT,
    mailer: OnceCell<AsyncSmtpTransport<Tokio1Executor>>,
    friendlycaptcha_verifier: FriendlyCaptchaVerifier<SecretRepositoryT>,
}

impl<SecretRepositoryT: SecretRepository> ContactFormMessageHandler<SecretRepositoryT> {
    async fn new() -> Self
    where
        SecretRepositoryT: Clone,
    {
        let secrets_repository = SecretRepositoryT::open().await;
        Self {
            secrets_repository: secrets_repository.clone(),
            mailer: Default::default(),
            friendlycaptcha_verifier: FriendlyCaptchaVerifier::new(secrets_repository),
        }
    }

    async fn handle(&self, event: Request) -> Result<Response<Body>, Error> {
        let Some(message) = event.payload()? else {
            let error = ContactFormError::InternalError {
                description: "Missing event payload".into(),
                subject: "(Unable to retrieve)".into(),
                body: "(Unable to retrieve)".into(),
                language: "en".into(),
            };
            error.log();
            return Ok(error.into_response());
        };
        match self.process_message(message).await {
            Ok(language) => Ok(Response::builder()
                .status(StatusCode::SEE_OTHER)
                .header("Location", Self::create_success_url(language.as_str()))
                .body("".into())
                .unwrap()),
            Err(error) => {
                error.log();
                Ok(error.into_response())
            }
        }
    }

    async fn process_message(
        &self,
        message: ContactFormMessage,
    ) -> Result<String, ContactFormError> {
        let validated_message = message.validate()?;
        self.verify_captcha(&validated_message).await?;
        let email = self.construct_email_message(&validated_message)?;
        self.send_email(email, &validated_message).await
    }

    async fn verify_captcha<'a>(
        &self,
        message: &ValidatedContactFormMessage<'a>,
    ) -> Result<(), ContactFormError> {
        self.friendlycaptcha_verifier
            .verify_token(message.friendlycaptcha_token)
            .await
            .map_err(|e| {
                e.into_contact_form_error(
                    message.subject.into(),
                    message.body.into(),
                    message.language.into(),
                )
            })?;
        Ok(())
    }

    fn construct_email_message(
        &self,
        message: &ValidatedContactFormMessage,
    ) -> Result<Message, ContactFormError> {
        let reply_to_string = if let Some(name) = message.name {
            format!("{} <{}>", name, message.email)
        } else {
            message.email.into()
        };
        let Ok(reply_to_email) = reply_to_string.parse() else {
            return Err(ContactFormError::ClientError(format!(
                "Invalid email address {}",
                message.email
            )));
        };
        Ok(Message::builder()
            .from(
                FROM_MAILBOX
                    .get_or_init(|| FROM_ADDRESS.parse().unwrap())
                    .clone(),
            )
            .reply_to(reply_to_email)
            .to(TO_MAILBOX
                .get_or_init(|| TO_ADDRESS.parse().unwrap())
                .clone())
            .subject(message.subject)
            .header(ContentType::TEXT_PLAIN)
            .body(message.body.to_string())
            .map_err(|error| ContactFormError::InternalError {
                description: format!("Error building message: {error}"),
                subject: message.subject.into(),
                body: message.body.into(),
                language: message.language.into(),
            })?)
    }

    async fn send_email<'a>(
        &self,
        email: Message,
        validated_message: &ValidatedContactFormMessage<'a>,
    ) -> Result<String, ContactFormError> {
        let mailer = self
            .mailer
            .get_or_try_init(self.initialise_mailer())
            .await
            .map_err(|e| ContactFormError::InternalError {
                description: format!("Unable to connect to SMTP server: {e}"),
                subject: validated_message.subject.into(),
                body: validated_message.body.into(),
                language: validated_message.language.into(),
            })?;
        match mailer.send(email).await {
            Ok(_) => Ok(validated_message.language.into()),
            Err(error) => Err(ContactFormError::InternalError {
                description: format!("Error sending message: {error}"),
                subject: validated_message.subject.into(),
                body: validated_message.body.into(),
                language: validated_message.language.into(),
            }),
        }
    }

    async fn initialise_mailer(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>, Error> {
        let smtp_url = Self::smtp_url();
        println!("initialise_mailer: Connecting to {smtp_url}");
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::from_url(&smtp_url)?
            .authentication(vec![Mechanism::Plain]);

        // Sending credentials over a non-TLS connection is risky, so we only set the credentials
        // when the connection URL is over TLS. If the environment is misconfigured so that
        // the credentials are not sent, the connection will be rejected. This is better than a
        // security breach.
        if smtp_url.starts_with("smtps://") {
            let parsed_credentials: SmtpCredentials = self
                .secrets_repository
                .get_secret(SMTP_CREDENTIALS_NAME)
                .await?;
            builder = builder.credentials(Credentials::new(
                parsed_credentials.username,
                parsed_credentials.password,
            ));
        }

        Ok(builder.build())
    }

    fn smtp_url() -> Cow<'static, str> {
        std::env::var("SMTP_URL")
            .map(Cow::Owned)
            .unwrap_or(SMTP_URL.into())
    }

    fn create_success_url(language: &str) -> String {
        if language == "en" {
            format!("https://{BASE_HOST}/email-sent.html")
        } else {
            format!("https://{BASE_HOST}/email-sent.{language}.html")
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

impl ContactFormMessage {
    fn validate(&self) -> Result<ValidatedContactFormMessage, ContactFormError> {
        let ContactFormMessage {
            name,
            email: Some(email),
            subject: Some(subject),
            body: Some(body),
            language: Some(language),
            friendlycaptcha_token: Some(friendlycaptcha_token),
        } = self
        else {
            return Err(ContactFormError::ClientError(
                "Missing fields in request".into(),
            ));
        };

        Ok(ValidatedContactFormMessage {
            name: name.as_ref().map(|s| s.as_str()),
            email,
            subject,
            body,
            language,
            friendlycaptcha_token,
        })
    }
}

struct ValidatedContactFormMessage<'a> {
    name: Option<&'a str>,
    email: &'a str,
    subject: &'a str,
    body: &'a str,
    language: &'a str,
    friendlycaptcha_token: &'a str,
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

    fn into_response(self) -> Response<Body> {
        match self {
            ContactFormError::InternalError {
                subject,
                body,
                language,
                ..
            } => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("Content-Type", "text/html; charset=utf-8")
                .body(render_error_page(subject.as_str(), body.as_str(), language.as_str()).into())
                .unwrap(),
            ContactFormError::ClientError(description) => Response::builder()
                .status(StatusCode::BAD_REQUEST)
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
    use crate::{
        friendlycaptcha::FRIENDLYCAPTCHA_DATA_NAME,
        secrets::test_support::{
            FakeSecretRepsitory, FAKE_FRIENDLYCAPTCHA_SECRET, FAKE_FRIENDLYCAPTCHA_SITEKEY,
        },
        SMTP_CREDENTIALS_NAME,
    };
    use googletest::prelude::*;
    use lambda_http::{http::HeaderValue, Body, Request};
    use serde::Serialize;
    use serial_test::serial;
    use std::{sync::OnceLock, time::Duration};
    use test_support::{
        fake_friendlycaptcha::FakeFriendlyCaptcha,
        fake_smtp::{start_poisoned_smtp_server, FakeSmtpServer, POISONED_SMTP_PORT, SMTP_PORT},
        setup_logging,
    };
    use tokio::time::timeout;

    type ContactFormMessageHandlerForTesting = ContactFormMessageHandler<FakeSecretRepsitory>;

    const CORRECT_CAPTCHA_SOLUTION: &str = "correct captcha solution";

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
        let subject = ContactFormMessageHandlerForTesting::new().await;

        let response = subject.handle(event).await.unwrap();

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
        let subject = ContactFormMessageHandlerForTesting::new().await;

        let response = subject.handle(event).await.unwrap();

        verify_that!(response.status().as_u16(), eq(400))
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn sends_mail_when_friendlycaptcha_fails() {
        init().await;
        let event = EventPayload::arbitrary().into_event();
        let subject = ContactFormMessageHandlerForTesting::new().await;

        let response = subject.handle(event).await.unwrap();

        expect_that!(response.status().as_u16(), eq(303));
        expect_that!(
            response.body(),
            points_to(matches_pattern!(Body::Text(eq(""))))
        );
        expect_that!(
            timeout(Duration::from_secs(1), fake_smtp().last_mail_content()).await,
            ok(ok(anything()))
        )
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn returns_400_when_captcha_solution_is_wrong_on_second_attempt() -> Result<()> {
        init().await;
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET)
                .require_solution(CORRECT_CAPTCHA_SOLUTION);
        tokio::spawn(fake_friendlycaptcha.serve());
        let mut subject = ContactFormMessageHandlerForTesting::new().await;
        let event = EventPayload::arbitrary()
            .with_captcha_solution("incorrect captcha solution")
            .into_event();
        subject
            .secrets_repository
            .remove_secret(FRIENDLYCAPTCHA_DATA_NAME);
        subject.handle(event).await.unwrap();
        let event = EventPayload::arbitrary()
            .with_captcha_solution("incorrect captcha solution")
            .into_event();
        subject.secrets_repository.add_secret(
            FRIENDLYCAPTCHA_DATA_NAME,
            format!(
                r#"{{
                    "FRIENDLYCAPTCHA_SITEKEY": "{FAKE_FRIENDLYCAPTCHA_SITEKEY}",
                    "FRIENDLYCAPTCHA_SECRET": "{FAKE_FRIENDLYCAPTCHA_SECRET}"
                }}"#
            ),
        );

        let response = subject.handle(event).await.unwrap();

        verify_that!(response.status().as_u16(), eq(400))
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
        let subject = ContactFormMessageHandlerForTesting::new().await;

        let response = subject.handle(event).await.unwrap();

        expect_that!(response.status().as_u16(), eq(303));
        expect_that!(
            response.body(),
            points_to(matches_pattern!(Body::Text(eq(""))))
        );
        expect_that!(
            timeout(Duration::from_secs(1), fake_smtp().last_mail_content()).await,
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
        let subject = ContactFormMessageHandlerForTesting::new().await;

        subject.handle(event).await.unwrap();

        expect_that!(
            timeout(Duration::from_secs(1), fake_smtp().last_mail_content()).await,
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
        let subject = ContactFormMessageHandlerForTesting::new().await;

        let response = subject.handle(event).await.unwrap();

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
        init().await;
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, "A different secret");
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary().into_event();
        let subject = ContactFormMessageHandlerForTesting::new().await;

        let response = subject.handle(event).await.unwrap();

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
    async fn returns_contact_page_when_connection_to_mail_server_fails() {
        init().await;
        let _env = TemporaryEnv::new("SMTP_URL", "smtp://nonexistent.host.internal");
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET);
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary().into_event();
        let subject = ContactFormMessageHandlerForTesting::new().await;

        let response = subject.handle(event).await.unwrap();

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
    async fn returns_contact_page_when_smtp_fails() {
        init().await;
        start_poisoned_smtp_server();
        let _env = TemporaryEnv::new("SMTP_URL", format!("smtp://localhost:{POISONED_SMTP_PORT}"));
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET);
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary().into_event();
        let subject = ContactFormMessageHandlerForTesting::new().await;

        let response = subject.handle(event).await.unwrap();

        expect_that!(response.status().as_u16(), eq(500));
        expect_that!(
            response.body(),
            points_to(matches_pattern!(Body::Text(contains_substring(
                "Something went wrong"
            ))))
        );
        expect_that!(
            response.headers().get("Content-Type"),
            some(eq("text/html; charset=utf-8"))
        );
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn send_mail_when_secrets_service_fails_for_friendlycaptcha() {
        setup_logging();
        init().await;
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET);
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary().into_event();
        let mut subject = ContactFormMessageHandlerForTesting::new().await;
        subject
            .secrets_repository
            .remove_secret(FRIENDLYCAPTCHA_DATA_NAME);

        let response = subject.handle(event).await.unwrap();

        expect_that!(response.status().as_u16(), eq(303));
        expect_that!(
            response.body(),
            points_to(matches_pattern!(Body::Text(eq(""))))
        );
        expect_that!(
            timeout(Duration::from_secs(1), fake_smtp().last_mail_content()).await,
            ok(ok(anything()))
        )
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn returns_contact_page_when_secrets_service_fails_for_smtp() {
        init().await;
        // Credentials are only retrieved if using smtps
        let _env = TemporaryEnv::new("SMTP_URL", format!("smtps://localhost:{SMTP_PORT}"));
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET);
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary().into_event();
        let mut subject = ContactFormMessageHandlerForTesting::new().await;
        subject
            .secrets_repository
            .remove_secret(SMTP_CREDENTIALS_NAME);

        let response = subject.handle(event).await.unwrap();

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
    async fn sends_mail_when_second_attempt_to_obtain_smtp_secrets_succeeds() {
        init().await;
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET);
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary().into_event();
        let mut subject = ContactFormMessageHandlerForTesting::new().await;
        {
            // Credentials are only retrieved if using smtps
            let _env = TemporaryEnv::new("SMTP_URL", format!("smtps://localhost:{SMTP_PORT}"));
            let event = EventPayload::arbitrary().into_event();
            subject
                .secrets_repository
                .remove_secret(SMTP_CREDENTIALS_NAME);
            subject.handle(event).await.unwrap();
        }

        subject.handle(event).await.unwrap();

        expect_that!(
            timeout(Duration::from_secs(1), fake_smtp().last_mail_content()).await,
            ok(ok(anything()))
        )
    }

    #[googletest::test]
    #[tokio::test]
    #[serial]
    async fn renders_message_content_and_subject_in_error_page() {
        init().await;
        start_poisoned_smtp_server();
        let _env = TemporaryEnv::new("SMTP_URL", format!("smtp://localhost:{POISONED_SMTP_PORT}"));
        let fake_friendlycaptcha =
            FakeFriendlyCaptcha::new(FAKE_FRIENDLYCAPTCHA_SITEKEY, FAKE_FRIENDLYCAPTCHA_SECRET);
        tokio::spawn(fake_friendlycaptcha.serve());
        let event = EventPayload::arbitrary()
            .with_subject("Message subject")
            .with_body("Message body")
            .into_event();
        let subject = ContactFormMessageHandlerForTesting::new().await;

        let response = subject.handle(event).await.unwrap();

        expect_that!(response.status().as_u16(), eq(500));
        expect_that!(
            response.body(),
            points_to(matches_pattern!(Body::Text(
                contains_substring("Message subject").and(contains_substring("Message body"))
            )))
        );
    }

    async fn init() {
        setup_environment();
        fake_smtp().start();
        fake_smtp().flush().await;
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

        fn with_subject(self, subject: impl AsRef<str>) -> Self {
            Self {
                subject: subject.as_ref().into(),
                ..self
            }
        }

        fn with_body(self, body: impl AsRef<str>) -> Self {
            Self {
                body: body.as_ref().into(),
                ..self
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

    struct TemporaryEnv(&'static str, Option<String>);

    impl TemporaryEnv {
        fn new(key: &'static str, value: impl AsRef<str>) -> Self {
            let old_value = std::env::var(key).ok();
            std::env::set_var(key, value.as_ref());
            Self(key, old_value)
        }
    }

    impl Drop for TemporaryEnv {
        fn drop(&mut self) {
            if let Some(value) = self.1.as_ref() {
                std::env::set_var(self.0, value);
            } else {
                std::env::remove_var(self.0);
            }
        }
    }

    fn fake_smtp() -> &'static FakeSmtpServer {
        static FAKE_SMTP: OnceLock<FakeSmtpServer> = OnceLock::new();
        FAKE_SMTP.get_or_init(|| FakeSmtpServer::new())
    }
}
