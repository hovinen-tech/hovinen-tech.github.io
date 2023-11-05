use actix_web::{http::StatusCode, post, web, App, HttpServer, Responder};
use lazy_static::lazy_static;
use lettre::{
    message::{header::ContentType, Mailbox},
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use log::{info, LevelFilter::Info};
use serde::Deserialize;
use simple_logger::SimpleLogger;
use std::{borrow::Cow, sync::Arc};

#[derive(Deserialize, Debug)]
struct ContactFormMessage {
    name: Option<String>,
    email: String,
    subject: String,
    body: String,
}

struct State {
    mailer: Arc<AsyncSmtpTransport<Tokio1Executor>>,
}

lazy_static! {
    static ref FROM_ADDRESS: Mailbox = "Web contact form <noreply@hovinen.tech>".parse().unwrap();
    static ref TO_ADDRESS: Mailbox = "Bradford Hovinen <hovinen@localhost>".parse().unwrap();
}
const SMTP_URL: &'static str = "smtp://localhost";

#[post("/messages")]
async fn send_message(
    message: web::Json<ContactFormMessage>,
    state: web::Data<State>,
) -> impl Responder {
    info!("Got message {message:?}");
    let ContactFormMessage {
        name,
        email,
        subject,
        body,
    } = message.0;
    let reply_to_string = if let Some(name) = name {
        format!("{} <{}>", name, email)
    } else {
        email.clone()
    };
    let Ok(reply_to_email) = reply_to_string.parse() else {
        return (
            Cow::from(format!("Bad email: {}", email)),
            StatusCode::BAD_REQUEST,
        );
    };
    let email = Message::builder()
        .from(FROM_ADDRESS.clone())
        .reply_to(reply_to_email)
        .to(TO_ADDRESS.clone())
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .unwrap();
    match state.mailer.send(email).await {
        Ok(_) => (Cow::from("Message successfully sent"), StatusCode::OK),
        Err(e) => (
            Cow::from(format!("Message sending error: {e:?}")),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    SimpleLogger::new().with_level(Info).init().unwrap();

    let mailer = Arc::new(
        AsyncSmtpTransport::<Tokio1Executor>::from_url(SMTP_URL)
            .unwrap()
            .build(),
    );

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(State {
                mailer: mailer.clone(),
            }))
            .service(send_message)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
