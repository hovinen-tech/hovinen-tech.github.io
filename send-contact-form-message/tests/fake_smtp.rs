use mailin_embedded::{Handler, Server, SslConfig};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot::{self, Receiver, Sender};
use tracing::debug;

pub const SMTP_PORT: &str = "4567";

#[derive(Clone)]
struct SmtpHandler(Vec<u8>, Arc<Mutex<Option<Sender<String>>>>);

impl Handler for SmtpHandler {
    fn data(&mut self, buf: &[u8]) -> std::io::Result<()> {
        debug!("Got data:\n{}", String::from_utf8_lossy(buf));
        self.0.extend(buf);
        Ok(())
    }

    fn data_end(&mut self) -> mailin_embedded::Response {
        self.1
            .lock()
            .unwrap()
            .take()
            .unwrap()
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

pub fn setup_smtp() -> Receiver<String> {
    let (tx, rx) = oneshot::channel();
    let handler = SmtpHandler(Vec::new(), Arc::new(Mutex::new(Some(tx))));
    let mut server = Server::new(handler);
    server
        .with_name("hovinen.tech")
        .with_ssl(SslConfig::None)
        .unwrap()
        .with_addr(format!("0.0.0.0:{SMTP_PORT}"))
        .unwrap();
    std::thread::spawn(|| {
        let _ = server.serve();
    });
    rx
}
