use axum::{
    body::Body,
    response::{IntoResponse, Response},
};

// 嵌入静态资源
const MQTT_WS_HTML: &str = include_str!("../resources/mqtt_ws.html");

/// MQTT-over-WebSocket 调试页(连内置 broker :9001,看屏幕图片 + 发输入)。
pub async fn mqtt_ws_handler() -> impl IntoResponse {
    Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(MQTT_WS_HTML))
        .unwrap()
}
