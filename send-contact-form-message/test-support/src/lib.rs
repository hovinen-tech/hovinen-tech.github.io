pub mod fake_friendlycaptcha;
pub mod fake_smtp;
pub mod localstack_config;
pub mod secrets;

use log::LevelFilter;
use regex::Regex;
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::borrow::Cow;

// Address of services which this test runs itself, as seen by the containers inside Docker. This
// is a fixed IP address for Docker in Linux.
pub const HOST_IP: &str = "172.17.0.1";

pub fn clean_payload(raw: &str) -> Cow<str> {
    let line_break = Regex::new("\n +").unwrap();
    line_break.replace_all(raw, "")
}

pub fn setup_logging() {
    CombinedLogger::init(vec![TermLogger::new(
        LevelFilter::Debug,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    )])
    .unwrap();
}
