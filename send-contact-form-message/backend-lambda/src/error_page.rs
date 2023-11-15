use crate::BASE_HOST;
use serde::Serialize;
use tinytemplate::TinyTemplate;

const SEND_ERROR_TEMPLATE_NAME: &str = "send-error-template-en";
const SEND_ERROR_TEMPLATE_EN: &str = include_str!("../assets/send-error.html");

#[derive(Serialize)]
struct Context {
    site_root: String,
    email: String,
    phone: String,
    subject: String,
    body: String,
}

pub fn render_error_page<'a>(subject: &'a str, body: &'a str, _language: &'a str) -> String {
    let mut tt = TinyTemplate::new();
    tt.add_template(SEND_ERROR_TEMPLATE_NAME, SEND_ERROR_TEMPLATE_EN)
        .unwrap();
    let context = Context {
        site_root: format!("https://{BASE_HOST}"),
        email: "FIXME".into(),
        phone: "FIXME".into(),
        subject: subject.into(),
        body: body.into(),
    };
    tt.render(SEND_ERROR_TEMPLATE_NAME, &context).unwrap()
}
