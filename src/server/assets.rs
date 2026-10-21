//! The page itself, compiled into the binary so `otto serve` needs nothing beside it on disk.
//!
//! Every asset goes out `no-cache`: the page changes whenever otto is upgraded, and a browser
//! holding on to the old script talks to the new server in a vocabulary it no longer speaks — a
//! live feed stuck on "Connecting…" because the old page ignores the events the new server sends.

use axum::http::header;
use axum::response::IntoResponse;

pub async fn index() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], include_str!("../../web/index.html"))
}

pub async fn app_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], include_str!("../../web/app.js"))
}

pub async fn style_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], include_str!("../../web/style.css"))
}

pub async fn favicon() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "no-cache")], include_str!("../../web/favicon.svg"))
}

// The two faces the page is set in, carried along so it never reaches out to a font host.
// Both under the SIL Open Font License; the texts sit beside them in web/fonts/.

pub async fn font_mono() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "font/woff2"), (header::CACHE_CONTROL, "no-cache")], include_bytes!("../../web/fonts/jetbrains-mono.woff2").as_slice())
}

pub async fn font_sans() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "font/woff2"), (header::CACHE_CONTROL, "no-cache")], include_bytes!("../../web/fonts/ibm-plex-sans.woff2").as_slice())
}
