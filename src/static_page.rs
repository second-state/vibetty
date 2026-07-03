use axum::{
    body::Body,
    response::{IntoResponse, Response},
};

// 嵌入静态资源
const INDEX_HTML: &str = include_str!("../resources/index.html");
const APP_JS: &str = include_str!("../resources/app.js");
const VOSK_HTML: &str = include_str!("../resources/vosk/index.html");
const MQTT_WS_HTML: &str = include_str!("../resources/mqtt_ws.html");

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

pub async fn vosk_handler() -> impl IntoResponse {
    Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(VOSK_HTML))
        .unwrap()
}

/// MQTT-over-WebSocket 调试页(连内置 broker :9001,看屏幕图片 + 发输入)。
pub async fn mqtt_ws_handler() -> impl IntoResponse {
    Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(MQTT_WS_HTML))
        .unwrap()
}
