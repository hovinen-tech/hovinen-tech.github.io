use lambda_http::{run, service_fn, Body, Error, Request, RequestPayloadExt, Response};
use lazy_static::lazy_static;
use lettre::{
    message::{header::ContentType, Mailbox},
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use serde::Deserialize;
use std::{fmt::Display, sync::Arc};
use tokio::sync::Mutex;
use tracing::info;

lazy_static! {
    static ref MAILER: Mutex<Option<Arc<AsyncSmtpTransport<Tokio1Executor>>>> = Mutex::new(None);
    static ref FROM_ADDRESS: Mailbox = "Web contact form <noreply@hovinen.tech>".parse().unwrap();
    static ref TO_ADDRESS: Mailbox = "Bradford Hovinen <hovinen@localhost>".parse().unwrap();
}

const SMTP_HOST: &'static str = "";
const SMTP_USERNAME_KEY: &'static str = "SMTP_USERNAME";
const SMTP_PASSWORD_KEY: &'static str = "SMTP_PASSWORD";

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
}

#[derive(Debug)]
enum MessageError {
    BadEmail(String),
    BadMessage(lettre::error::Error),
    SendError(lettre::transport::smtp::Error),
    MissingPayload,
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageError::BadEmail(email) => write!(f, "Bad email: {email}"),
            MessageError::BadMessage(error) => write!(f, "Error building message: {error}"),
            MessageError::SendError(error) => write!(f, "Error sending message: {error}"),
            MessageError::MissingPayload => write!(f, "Event message is missing a payload"),
        }
    }
}

impl std::error::Error for MessageError {}

async fn function_handler(event: Request) -> Result<Response<Body>, Error> {
    info!("event body: {:?}", event.body());
    info!("event headers: {:?}", event.headers());
    let Some(message) = event.payload()? else {
        return Err(Box::new(MessageError::MissingPayload));
    };
    send_message(message).await.map_err(Box::new)?;
    Ok(Response::builder().status(200).body("".into()).unwrap())
}

async fn send_message(message: ContactFormMessage) -> Result<(), MessageError> {
    let ContactFormMessage {
        name,
        email,
        subject,
        body,
    } = message;
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
        Ok(_) => Ok(()),
        Err(e) => Err(MessageError::SendError(e)),
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

async fn initialise_mailer() -> Result<AsyncSmtpTransport<Tokio1Executor>, Error> {
    let config = aws_config::load_from_env().await;
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);

    let smtp_username_secret = secrets_client
        .get_secret_value()
        .secret_id(SMTP_USERNAME_KEY)
        .send()
        .await?;
    let Some(smtp_username) = smtp_username_secret.secret_string() else {
        return Err(Box::new(EnvironmentError::MissingSecret(SMTP_USERNAME_KEY)));
    };

    let smtp_password_secret = secrets_client
        .get_secret_value()
        .secret_id(SMTP_PASSWORD_KEY)
        .send()
        .await?;
    let Some(smtp_password) = smtp_password_secret.secret_string() else {
        return Err(Box::new(EnvironmentError::MissingSecret(SMTP_PASSWORD_KEY)));
    };

    Ok(AsyncSmtpTransport::<Tokio1Executor>::from_url(
        create_smtp_url(smtp_username, smtp_password).as_str(),
    )?
    .build())
}

fn create_smtp_url(username: &str, password: &str) -> String {
    format!("smtps://{username}:{password}@{SMTP_HOST}")
}
