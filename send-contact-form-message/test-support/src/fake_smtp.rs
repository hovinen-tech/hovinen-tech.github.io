use log::debug;
use mailin_embedded::{Handler, Server, SslConfig};
use std::{net::IpAddr, sync::Arc, time::Duration};
use tokio::{
    sync::watch::{self, error::RecvError, Receiver, Sender},
    time::timeout,
};

pub const SMTP_PORT: u16 = 4567;
pub const POISONED_SMTP_PORT: u16 = 4568;

#[derive(Clone)]
struct SmtpHandler(Vec<u8>, Arc<Sender<String>>);

impl Handler for SmtpHandler {
    fn data(&mut self, buf: &[u8]) -> std::io::Result<()> {
        debug!("Got data:\n{}", String::from_utf8_lossy(buf));
        self.0.extend(buf);
        Ok(())
    }

    fn data_end(&mut self) -> mailin_embedded::Response {
        self.1
            .send(String::from_utf8(self.0.drain(..).collect()).unwrap())
            .unwrap();
        mailin_embedded::response::OK
    }

    fn auth_plain(
        &mut self,
        authorization_id: &str,
        authentication_id: &str,
        password: &str,
    ) -> mailin_embedded::Response {
        debug!("Got authentication data {authorization_id}, {authentication_id}, {password}");
        mailin_embedded::response::AUTH_OK
    }
}

pub struct FakeSmtpServer(
    std::sync::Mutex<Option<Server<SmtpHandler>>>,
    tokio::sync::Mutex<Receiver<String>>,
);

impl FakeSmtpServer {
    pub fn new() -> Self {
        let (sender, receiver) = watch::channel("".into());
        let handler = SmtpHandler(Vec::new(), Arc::new(sender));
        let mut server = Server::new(handler);
        server
            .with_name("hovinen.tech")
            .with_ssl(SslConfig::None)
            .unwrap()
            .with_addr(format!("0.0.0.0:{SMTP_PORT}"))
            .unwrap();
        Self(
            std::sync::Mutex::new(Some(server)),
            tokio::sync::Mutex::new(receiver),
        )
    }

    pub fn start(&self) {
        let mut guard = self.0.lock().unwrap();
        if let Some(server) = guard.take() {
            std::thread::spawn(move || {
                let _ = server.serve();
            });
        }
    }

    pub async fn last_mail_content(&self) -> Result<String, RecvError> {
        let mut receiver = self.1.lock().await;
        receiver.changed().await?;
        let content = receiver.borrow_and_update().clone();
        drop(receiver);
        Ok(content)
    }

    pub async fn flush(&self) {
        let mut receiver = self.1.lock().await;
        let _ = timeout(Duration::from_millis(100), receiver.changed()).await;
    }

    pub fn setup_environment() {
        std::env::set_var("SMTP_URL", format!("smtp://localhost:{SMTP_PORT}"));
    }
}

impl Default for FakeSmtpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
struct PoisonedSmtpHandler;

impl Handler for PoisonedSmtpHandler {
    fn helo(&mut self, _ip: IpAddr, _domain: &str) -> mailin_embedded::Response {
        mailin_embedded::response::INTERNAL_ERROR
    }

    fn mail(&mut self, _ip: IpAddr, _domain: &str, _from: &str) -> mailin_embedded::Response {
        mailin_embedded::response::INTERNAL_ERROR
    }
}

pub fn start_poisoned_smtp_server() {
    let handler = PoisonedSmtpHandler;
    let mut server = Server::new(handler);
    server
        .with_name("hovinen.tech")
        .with_ssl(SslConfig::None)
        .unwrap()
        .with_addr(format!("0.0.0.0:{POISONED_SMTP_PORT}"))
        .unwrap();
    std::thread::spawn(move || {
        let _ = server.serve();
    });
}
