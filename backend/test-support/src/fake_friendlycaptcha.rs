use crate::HOST_IP;
use axum::{
    body::{Bytes, Full},
    extract::{Json, State},
    http::StatusCode,
    response::Response,
    routing::post,
    Router, Server,
};
use hyper::header;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::borrow::Cow;

const FRIENDLYCAPTCHA_PORT: u16 = 5283;
const VERIFY_PATH: &str = "/verify";

#[derive(Clone)]
pub struct FakeFriendlyCaptcha {
    required_sitekey: Cow<'static, str>,
    required_secret: Cow<'static, str>,
    required_solution: Option<String>,
    return_invalid_response: bool,
    return_solution_timeout: bool,
}

#[derive(Deserialize)]
struct VerifyRequestPayload {
    solution: String,
    secret: String,
    sitekey: String,
}

#[derive(Serialize)]
struct VerifyResponsePayload {
    success: bool,
    #[serde(default)]
    errors: Vec<String>,
}

impl FakeFriendlyCaptcha {
    pub fn new(
        required_sitekey: impl Into<Cow<'static, str>>,
        required_secret: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            required_sitekey: required_sitekey.into(),
            required_secret: required_secret.into(),
            required_solution: None,
            return_invalid_response: false,
            return_solution_timeout: false,
        }
    }

    pub fn setup_environment() {
        std::env::set_var(
            "FRIENDLYCAPTCHA_VERIFY_URL",
            format!("http://localhost:{FRIENDLYCAPTCHA_PORT}{VERIFY_PATH}"),
        );
    }

    pub async fn serve(self) {
        let app = Router::new()
            .route(VERIFY_PATH, post(verify))
            .with_state(self);
        Server::bind(&format!("0.0.0.0:{FRIENDLYCAPTCHA_PORT}").parse().unwrap())
            .serve(app.into_make_service())
            .await
            .unwrap();
    }

    pub fn require_solution(self, required_solution: impl AsRef<str>) -> Self {
        Self {
            required_solution: Some(required_solution.as_ref().into()),
            ..self
        }
    }

    pub fn return_invalid_response(self) -> Self {
        Self {
            return_invalid_response: true,
            ..self
        }
    }

    pub fn return_solution_timeout(self) -> Self {
        Self {
            return_solution_timeout: true,
            ..self
        }
    }

    pub fn verify_url() -> String {
        format!("http://{HOST_IP}:{FRIENDLYCAPTCHA_PORT}{VERIFY_PATH}")
    }
}

async fn verify(
    State(state): State<FakeFriendlyCaptcha>,
    Json(payload): Json<VerifyRequestPayload>,
) -> Response<Full<Bytes>> {
    if state.return_invalid_response {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Full::from("Invalid response"))
            .unwrap()
    } else if state.return_solution_timeout {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::from(
                json!(VerifyResponsePayload {
                    success: false,
                    errors: vec!["solution_timeout_or_duplicate".into()],
                })
                .to_string(),
            ))
            .unwrap()
    } else if payload.sitekey != state.required_sitekey {
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::from(
                json!(VerifyResponsePayload {
                    success: false,
                    errors: vec!["bad_request".into()],
                })
                .to_string(),
            ))
            .unwrap()
    } else if payload.secret != state.required_secret {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::from(
                json!(VerifyResponsePayload {
                    success: false,
                    errors: vec!["secret_invalid".into()],
                })
                .to_string(),
            ))
            .unwrap()
    } else if state.required_solution.is_some() && Some(payload.solution) != state.required_solution
    {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::from(
                json!(VerifyResponsePayload {
                    success: false,
                    errors: vec!["solution_invalid".into()],
                })
                .to_string(),
            ))
            .unwrap()
    } else {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::from(
                json!(VerifyResponsePayload {
                    success: true,
                    errors: vec![],
                })
                .to_string(),
            ))
            .unwrap()
    }
}
