//! The page itself, compiled into the binary so `otto serve` needs nothing beside it on disk.

use axum::http::header;
use axum::response::IntoResponse;

pub async fn index() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], include_str!("../../web/index.html"))
}

pub async fn app_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("../../web/app.js"))
}

pub async fn style_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], include_str!("../../web/style.css"))
}

pub async fn favicon() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/svg+xml")], include_str!("../../web/favicon.svg"))
}
