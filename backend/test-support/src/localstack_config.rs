use std::process::Command;

use aws_config::{BehaviorVersion, SdkConfig};
use lazy_static::lazy_static;
use log::info;
use serde::Deserialize;
use testcontainers::{clients::Cli, core::WaitFor, Container, GenericImage, RunnableImage};

pub const LOCALSTACK_PORT: u16 = 4566;

lazy_static! {
    static ref DOCKER: Cli = Cli::default();
}

pub struct LocalStackConfig {
    pub aws_host_from_subject: String,
    pub sdk_config: SdkConfig,
    container: Option<Container<'static, GenericImage>>,
}

impl LocalStackConfig {
    pub async fn new() -> Self {
        let (aws_endpoint_url_from_test, aws_host_from_subject, container) =
            Self::get_aws_endpoint_url();
        info!("Using AWS endpoint {aws_endpoint_url_from_test}");
        info!("Host from within system under test {aws_host_from_subject}");
        let sdk_config = aws_config::defaults(BehaviorVersion::latest())
            .endpoint_url(&aws_endpoint_url_from_test)
            .load()
            .await;
        Self {
            aws_host_from_subject,
            sdk_config,
            container,
        }
    }

    fn get_aws_endpoint_url() -> (String, String, Option<Container<'static, GenericImage>>) {
        if std::env::var("USE_RUNNING_LOCALSTACK").is_ok() {
            info!("Using already running LocalStack due to environment variable USE_RUNNING_LOCALSTACK");
            let localstack_container_ip = Self::get_container_ip_from_running_localstack();
            (
                format!("http://{localstack_container_ip}:{LOCALSTACK_PORT}"),
                localstack_container_ip,
                None,
            )
        } else {
            info!("Starting own LocalStack instance");
            let (aws_endpoint_url_from_test, aws_host_from_subject, container) =
                Self::run_localstack_returning_endpoint_url();
            (
                aws_endpoint_url_from_test,
                aws_host_from_subject,
                Some(container),
            )
        }
    }

    fn get_container_ip_from_running_localstack() -> String {
        #[derive(Deserialize)]
        struct LocalStackStatus {
            container_ip: String,
        }
        let localstack_status: LocalStackStatus = serde_json::from_slice(
            &Command::new("localstack")
                .args(["status", "docker", "-f", "json"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        localstack_status.container_ip
    }

    fn run_localstack_returning_endpoint_url() -> (String, String, Container<'static, GenericImage>)
    {
        let container = DOCKER.run(
            RunnableImage::from(
                GenericImage::new("localstack/localstack", "2.3.2")
                    .with_volume("/var/run/docker.sock", "/var/run/docker.sock")
                    .with_wait_for(WaitFor::Healthcheck),
            )
            .with_network("bridge")
            .with_env_var(("DEBUG", "1")),
        );
        (
            format!(
                "http://localhost:{}",
                container.get_host_port_ipv4(LOCALSTACK_PORT)
            ),
            format!("{}", container.get_bridge_ip_address()),
            container,
        )
    }
}

impl Drop for LocalStackConfig {
    fn drop(&mut self) {
        if let Some(c) = self.container.as_ref() {
            c.stop()
        }
    }
}
