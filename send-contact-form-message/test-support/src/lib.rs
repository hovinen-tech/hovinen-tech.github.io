pub mod fake_friendlycaptcha;
pub mod fake_smtp;
pub mod localstack_config;

// Address of services which this test runs itself, as seen by the containers inside Docker. This
// is a fixed IP address for Docker in Linux.
pub const HOST_IP: &str = "172.17.0.1";
