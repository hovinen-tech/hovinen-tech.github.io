use crate::BASE_HOST;
use serde::Serialize;
use serde_json::Value;
use tinytemplate::{error::Error, format, TinyTemplate};

const SEND_ERROR_TEMPLATE_NAME: &str = "send-error-template-en";
const SEND_ERROR_TEMPLATE_EN: &str = include_str!("../assets/send-error.html");

#[derive(Serialize)]
struct Context {
    site_root: String,
    subject: String,
    body: String,
}

pub fn render_error_page<'a>(subject: &'a str, body: &'a str, _language: &'a str) -> String {
    let mut tt = TinyTemplate::new();
    tt.add_formatter("render_paragraphs", render_paragraphs);
    tt.add_template(SEND_ERROR_TEMPLATE_NAME, SEND_ERROR_TEMPLATE_EN)
        .unwrap();
    let context = Context {
        site_root: format!("https://{BASE_HOST}"),
        subject: subject.into(),
        body: body.into(),
    };
    tt.render(SEND_ERROR_TEMPLATE_NAME, &context).unwrap()
}

fn render_paragraphs(value: &Value, output: &mut String) -> Result<(), Error> {
    output.push_str("<p>");
    let mut formatted = String::new();
    format(value, &mut formatted)?;
    output.push_str(&formatted.replace("\n\n", "</p><p>"));
    output.push_str("</p>");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::render_error_page;
    use googletest::prelude::*;

    const MALICIOUS_CONTENT: &str = "<script>doEvil();</script>";

    #[test]
    fn escapes_user_input_in_subject() -> Result<()> {
        let output = render_error_page(MALICIOUS_CONTENT, "A body", "en");

        verify_that!(output, not(contains_substring(MALICIOUS_CONTENT)))
    }

    #[test]
    fn escapes_user_input_in_body() -> Result<()> {
        let output = render_error_page("A subject", MALICIOUS_CONTENT, "en");

        verify_that!(output, not(contains_substring(MALICIOUS_CONTENT)))
    }

    #[test]
    fn renders_paragraphs_in_body() -> Result<()> {
        let output = render_error_page("A subject", "A paragraph\n\nAnother paragraph", "en");

        verify_that!(
            output,
            contains_substring("<p>A paragraph</p><p>Another paragraph</p>")
        )
    }
}
