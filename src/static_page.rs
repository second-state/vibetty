use axum::{
    body::Body,
    response::{IntoResponse, Response},
};

// 嵌入静态资源
const INDEX_HTML: &str = include_str!("../resources/index.html");
const APP_JS: &str = include_str!("../resources/app.js");
const VOSK_HTML: &str = include_str!("../resources/vosk/index.html");

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
