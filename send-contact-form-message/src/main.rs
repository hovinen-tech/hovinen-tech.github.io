use lambda_http::{run, service_fn, Body, Error, Request, RequestPayloadExt, Response};
use lazy_static::lazy_static;
use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{fmt::Display, sync::Arc};
use tokio::sync::Mutex;

lazy_static! {
    static ref MAILER: Mutex<Option<Arc<AsyncSmtpTransport<Tokio1Executor>>>> = Mutex::new(None);
    static ref MCAPTCHA_DATA: Mutex<Option<MCaptchaData>> = Mutex::new(None);
    static ref FROM_ADDRESS: Mailbox = "Web contact form <noreply@hovinen.tech>".parse().unwrap();
    static ref TO_ADDRESS: Mailbox = "Bradford Hovinen <hovinen@hovinen.tech>".parse().unwrap();
}

const SMTP_URL: &'static str = "smtps://email-smtp.eu-north-1.amazonaws.com";
const SMTP_CREDENTIALS_NAME: &'static str = "smtp-ses-credentials";

const MCAPTCHA_DATA_NAME: &'static str = "mcaptcha-data";
const MCAPTCHA_VERIFY_URL: &'static str = "https://demo.mcaptha.org/api/v1/pow/siteverify";

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
    email: String,
    subject: String,
    body: String,
    language: String,
    #[serde(rename = "mcaptcha__token")]
    mcaptcha_token: String,
}

#[derive(Debug)]
enum MessageError {
    BadEmail(String),
    BadMessage(lettre::error::Error),
    SendError(lettre::transport::smtp::Error),
    MissingPayload,
    McaptchaTokenError,
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageError::BadEmail(email) => write!(f, "Bad email: {email}"),
            MessageError::BadMessage(error) => write!(f, "Error building message: {error}"),
            MessageError::SendError(error) => write!(f, "Error sending message: {error}"),
            MessageError::MissingPayload => write!(f, "Event message is missing a payload"),
            MessageError::McaptchaTokenError => write!(f, "mCaptcha token did not validate"),
        }
    }
}

impl std::error::Error for MessageError {}

async fn function_handler(event: Request) -> Result<Response<Body>, Error> {
    let Some(message) = event.payload()? else {
        return Err(Box::new(MessageError::MissingPayload));
    };
    let language = send_message(message).await.map_err(Box::new)?;
    Ok(Response::builder()
        .status(303)
        .header("Location", create_success_url(language.as_str()))
        .body("".into())
        .unwrap())
}

async fn send_message(message: ContactFormMessage) -> Result<String, MessageError> {
    let ContactFormMessage {
        name,
        email,
        subject,
        body,
        language,
        mcaptcha_token,
    } = message;
    verify_mcaptcha_token(mcaptcha_token).await?;
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
    match get_mailer().await.send(email).await {
        Ok(_) => Ok(language),
        Err(e) => Err(MessageError::SendError(e)),
    }
}

#[derive(Deserialize, Clone)]
struct MCaptchaData {
    #[serde(rename = "MCAPTCHA_KEY")]
    key: String,
    #[serde(rename = "MCAPTCHA_SECRET")]
    secret: String,
}

#[derive(Serialize)]
struct MCaptchaVerifyPayload {
    token: String,
    key: String,
    secret: String,
}

#[derive(Deserialize)]
struct MCaptchaResponse {
    valid: bool,
}

async fn verify_mcaptcha_token(token: String) -> Result<(), MessageError> {
    let data = fetch_mcaptcha_data().await.unwrap();
    let payload = MCaptchaVerifyPayload {
        token,
        key: data.key,
        secret: data.secret,
    };
    let response: MCaptchaResponse = Client::new()
        .post(MCAPTCHA_VERIFY_URL)
        .json(&payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    if response.valid {
        Ok(())
    } else {
        Err(MessageError::McaptchaTokenError)
    }
}

async fn fetch_mcaptcha_data<'a>() -> Result<MCaptchaData, Error> {
    let mut guard = MCAPTCHA_DATA.lock().await;
    match &*guard {
        Some(data) => Ok(data.clone()),
        None => {
            let data: MCaptchaData = fetch_secret(MCAPTCHA_DATA_NAME).await?;
            *guard = Some(data.clone());
            Ok(data)
        }
    }
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

async fn get_mailer() -> Arc<AsyncSmtpTransport<Tokio1Executor>> {
    let mut guard = MAILER.lock().await;
    match &*guard {
        Some(mailer) => mailer.clone(),
        None => {
            let mailer = Arc::new(
                initialise_mailer()
                    .await
                    .expect("Could not initialize mailer"),
            );
            *guard = Some(mailer.clone());
            mailer
        }
    }
}

#[derive(Deserialize)]
struct SmtpCredentials {
    #[serde(rename = "SMTP_USERNAME")]
    username: String,
    #[serde(rename = "SMTP_PASSWORD")]
    password: String,
}

async fn initialise_mailer() -> Result<AsyncSmtpTransport<Tokio1Executor>, Error> {
    let parsed_credentials: SmtpCredentials = fetch_secret(SMTP_CREDENTIALS_NAME).await?;

    Ok(AsyncSmtpTransport::<Tokio1Executor>::from_url(SMTP_URL)?
        .credentials(Credentials::new(
            parsed_credentials.username,
            parsed_credentials.password,
        ))
        .build())
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
