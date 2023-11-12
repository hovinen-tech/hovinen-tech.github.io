use lambda_http::{run, service_fn, Body, Error, Request, RequestPayloadExt, Response};
use lazy_static::lazy_static;
use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{borrow::Cow, fmt::Display, future::Future, sync::Arc};
use tokio::sync::Mutex;
use tracing::warn;

lazy_static! {
    static ref MAILER: Mutex<Option<Arc<AsyncSmtpTransport<Tokio1Executor>>>> = Mutex::new(None);
    static ref FRIENDLYCAPTCHA_DATA: Mutex<Option<FriendlyCaptchaData>> = Mutex::new(None);
    static ref FROM_ADDRESS: Mailbox = "Web contact form <noreply@hovinen.tech>".parse().unwrap();
    static ref TO_ADDRESS: Mailbox = "Bradford Hovinen <hovinen@hovinen.tech>".parse().unwrap();
}

const SMTP_HOST: &'static str = "email-smtp.eu-north-1.amazonaws.com";
const SMTP_CREDENTIALS_NAME: &'static str = "smtp-ses-credentials";

const FRIENDLYCAPTCHA_DATA_NAME: &'static str = "friendlycaptcha-data";
const FRIENDLYCAPTCHA_VERIFY_URL: &'static str =
    "https://api.friendlycaptcha.com/api/v1/siteverify";

const BASE_HOST: &'static str = "hovinen-tech.github.io";

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .without_time()
        .init();

    run(service_fn(function_handler)).await
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

async fn function_handler(event: Request) -> Result<Response<Body>, Error> {
    let Some(message) = event.payload()? else {
        return Err(Box::new(MessageError::MissingPayload));
    };
    match send_message(message).await {
        Ok(language) => Ok(Response::builder()
            .status(303)
            .header("Location", create_success_url(language.as_str()))
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
    verify_friendlycaptcha_token(friendlycaptcha_token).await?;
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
    let mailer = get_memoized(&MAILER, || initialise_mailer()).await.unwrap();
    match mailer.send(email).await {
        Ok(_) => Ok(language),
        Err(e) => Err(MessageError::SendError(e)),
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

async fn verify_friendlycaptcha_token(solution: String) -> Result<(), MessageError> {
    let data = get_memoized(&FRIENDLYCAPTCHA_DATA, || {
        fetch_secret(FRIENDLYCAPTCHA_DATA_NAME)
    })
    .await
    .unwrap();
    let payload = FriendlyCaptchaVerifyPayload {
        solution,
        sitekey: data.sitekey,
        secret: data.secret,
    };
    let response = match Client::new()
        .post(friendlycaptcha_verify_url().as_ref())
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

async fn initialise_mailer() -> Result<Arc<AsyncSmtpTransport<Tokio1Executor>>, Error> {
    let parsed_credentials: SmtpCredentials = fetch_secret(SMTP_CREDENTIALS_NAME).await?;

    Ok(Arc::new(
        AsyncSmtpTransport::<Tokio1Executor>::from_url(&smtp_url())?
            .credentials(Credentials::new(
                parsed_credentials.username,
                parsed_credentials.password,
            ))
            .build(),
    ))
}

fn smtp_url() -> String {
    let host = std::env::var("SMTP_HOST")
        .map(Cow::Owned)
        .unwrap_or(SMTP_HOST.into());
    let port = std::env::var("SMTP_PORT")
        .map(|v| Cow::Owned(format!(":{v}")))
        .unwrap_or("".into());
    format!("smtps://{host}{port}")
}

async fn fetch_secret<T: DeserializeOwned>(name: &'static str) -> Result<T, Error> {
    let config = aws_config::from_env().region("eu-north-1").load().await;
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);

    let secret = secrets_client
        .get_secret_value()
        .secret_id(name)
        .send()
        .await?;
    let Some(secret_value) = secret.secret_string() else {
        return Err(Box::new(EnvironmentError::MissingSecret(name)));
    };
    Ok(serde_json::from_str(secret_value)?)
}

fn create_success_url(language: &str) -> String {
    if language == "en" {
        format!("https://{BASE_HOST}/email-sent.html")
    } else {
        format!("https://{BASE_HOST}/email-sent.{language}.html")
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
