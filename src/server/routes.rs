use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::json;

use crate::config::AppConfig;

pub fn v1_router() -> Router<AppConfig> {
    Router::new()
        .route("/models", get(list_models))
        .route("/chat", post(not_implemented))
        .route("/chat/stream", post(not_implemented))
        .route("/ws", get(not_implemented))
}

pub fn health_router() -> Router<AppConfig> {
    Router::new().route("/", get(health_check))
}

async fn health_check() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

async fn list_models(State(config): State<AppConfig>) -> impl IntoResponse {
    let models = if let Some(m) = config.model {
        vec![m]
    } else {
        vec![]
    };
    Json(json!({"models": models}))
}

async fn not_implemented() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "501 Not Implemented: server skeleton only.",
    )
}
