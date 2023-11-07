use lambda_http::{run, service_fn, Body, Error, Request, Response};
use lazy_static::lazy_static;
use lettre::{
    message::{header::ContentType, Mailbox},
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use serde::Deserialize;
use std::sync::Arc;

lazy_static! {
    static ref MAILER: Arc<AsyncSmtpTransport<Tokio1Executor>> = Arc::new(
        AsyncSmtpTransport::<Tokio1Executor>::from_url(SMTP_URL)
            .unwrap()
            .build()
    );
    static ref FROM_ADDRESS: Mailbox = "Web contact form <noreply@hovinen.tech>".parse().unwrap();
    static ref TO_ADDRESS: Mailbox = "Bradford Hovinen <hovinen@localhost>".parse().unwrap();
}

const SMTP_URL: &'static str = "smtp://localhost";

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
    MissingBody,
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageError::BadEmail(email) => write!(f, "Bad email: {email}"),
            MessageError::BadMessage(error) => write!(f, "Error building message: {error}"),
            MessageError::SendError(error) => write!(f, "Error sending message: {error}"),
            MessageError::MissingBody => write!(f, "Event message is missing a body"),
        }
    }
}

impl std::error::Error for MessageError {}

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
    match MAILER.send(email).await {
        Ok(_) => Ok(()),
        Err(e) => Err(MessageError::SendError(e)),
    }
}

async fn function_handler(event: Request) -> Result<Response<Body>, Error> {
    let message = if let Body::Text(body) = event.body() {
        serde_json::from_str(body)?
    } else {
        return Err(Box::new(MessageError::MissingBody));
    };
    send_message(message).await.map_err(Box::new)?;
    Ok(Response::builder().status(200).body("".into()).unwrap())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .without_time()
        .init();

    run(service_fn(function_handler)).await
}
