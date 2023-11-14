use crate::HOST_IP;
use axum::{
    extract::{Json, State},
    http::StatusCode,
    routing::post,
    Router, Server,
};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, sync::Arc};

const FRIENDLYCAPTCHA_PORT: u16 = 5283;
const VERIFY_PATH: &str = "/verify";

pub struct FakeFriendlyCaptcha {
    required_sitekey: Cow<'static, str>,
    required_secret: Cow<'static, str>,
}

#[derive(Deserialize)]
struct VerifyRequestPayload {
    #[allow(unused)]
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
    ) -> Arc<Self> {
        Arc::new(Self {
            required_sitekey: required_sitekey.into(),
            required_secret: required_secret.into(),
        })
    }

    pub async fn serve(self: Arc<Self>) {
        let app = Router::new()
            .route(VERIFY_PATH, post(verify))
            .with_state(self);
        Server::bind(&format!("0.0.0.0:{FRIENDLYCAPTCHA_PORT}").parse().unwrap())
            .serve(app.into_make_service())
            .await
            .unwrap();
    }

    pub fn verify_url() -> String {
        format!("http://{HOST_IP}:{FRIENDLYCAPTCHA_PORT}{VERIFY_PATH}")
    }
}

async fn verify(
    State(state): State<Arc<FakeFriendlyCaptcha>>,
    Json(payload): Json<VerifyRequestPayload>,
) -> (StatusCode, Json<VerifyResponsePayload>) {
    if payload.sitekey != state.required_sitekey {
        (
            StatusCode::BAD_REQUEST,
            Json(VerifyResponsePayload {
                success: false,
                errors: vec!["bad-sitekey".into()],
            }),
        )
    } else if payload.secret != state.required_secret {
        (
            StatusCode::BAD_REQUEST,
            Json(VerifyResponsePayload {
                success: false,
                errors: vec!["bad-secet".into()],
            }),
        )
    } else {
        (
            StatusCode::OK,
            Json(VerifyResponsePayload {
                success: true,
                errors: vec![],
            }),
        )
    }
}
