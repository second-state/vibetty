use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::net::SocketAddr;

// 嵌入静态资源
const INDEX_HTML: &str = include_str!("../resources/index.html");
const APP_JS: &str = include_str!("../resources/app.js");
const SETUP_HTML: &str = include_str!("../resources/setup.html");

pub async fn index_handler() -> impl IntoResponse {
    Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(INDEX_HTML))
        .unwrap()
}

pub async fn app_js_handler() -> impl IntoResponse {
    Response::builder()
        .header("content-type", "application/javascript")
        .body(Body::from(APP_JS))
        .unwrap()
}

pub async fn setup_handler() -> impl IntoResponse {
    Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(SETUP_HTML))
        .unwrap()
}

#[derive(Deserialize)]
pub struct ChangeDirRequest {
    pub path: String,
}

pub async fn change_dir_handler(
    State(state): State<crate::ws::AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<ChangeDirRequest>,
) -> impl IntoResponse {
    // 检查是否来自 localhost
    let ip = addr.ip();
    let is_localhost = ip.is_loopback();

    if !is_localhost {
        log::warn!("Change directory request from non-localhost: {}", ip);
        return (StatusCode::FORBIDDEN, "Only localhost access allowed").into_response();
    }

    log::info!("Change directory request from {}: {}", ip, req.path);

    // 发送 ChangeDir 消息到 cli_tx
    if let Err(e) = state
        .cli_tx
        .send(crate::protocol::ClientMessage::ChangeDir(req.path.clone()))
        .await
    {
        log::error!("Failed to send ChangeDir message: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to send change directory command: {}", e),
        )
            .into_response();
    }

    (StatusCode::OK, format!("Changing to: {}", req.path)).into_response()
}
