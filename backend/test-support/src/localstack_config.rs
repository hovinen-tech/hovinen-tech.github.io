use aws_config::{BehaviorVersion, SdkConfig};
use log::info;
use serde::Deserialize;
use std::process::Command;
use testcontainers::{
    core::{Mount, WaitFor},
    runners::AsyncRunner,
    ContainerAsync, GenericImage, ImageExt,
};

pub const LOCALSTACK_PORT: u16 = 4566;

pub struct LocalStackConfig {
    pub aws_host_from_subject: String,
    pub sdk_config: SdkConfig,
    container: Option<ContainerAsync<GenericImage>>,
}

impl LocalStackConfig {
    pub async fn new() -> Self {
        let (aws_endpoint_url_from_test, aws_host_from_subject, container) =
            Self::get_aws_endpoint_url().await;
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

    pub async fn stop(self) {
        if let Some(container) = self.container {
            container.stop().await.expect("Stopping container");
        }
    }

    async fn get_aws_endpoint_url() -> (String, String, Option<ContainerAsync<GenericImage>>) {
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
                Self::run_localstack_returning_endpoint_url().await;
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

    async fn run_localstack_returning_endpoint_url(
    ) -> (String, String, ContainerAsync<GenericImage>) {
        let image = GenericImage::new("localstack/localstack", "2.3.2")
            .with_wait_for(WaitFor::healthcheck())
            .with_network("bridge")
            .with_mount(Mount::bind_mount(
                "/var/run/docker.sock",
                "/var/run/docker.sock",
            ))
            .with_env_var("DEBUG", "1");
        let container = image.start().await.expect("LocalStack started");
        (
            format!(
                "http://localhost:{}",
                container
                    .get_host_port_ipv4(LOCALSTACK_PORT)
                    .await
                    .expect("Port should be present")
            ),
            format!(
                "{}",
                container
                    .get_bridge_ip_address()
                    .await
                    .expect("IP address should be present")
            ),
            container,
        )
    }
}
