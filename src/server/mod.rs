//! `otto serve` — the web UI.
//!
//! A view over the run directory, and nothing more: every handler calls the same `core` function
//! the matching CLI command does, and the server holds no state of its own. Stop it and nothing
//! about any run changes; runs keep waking under poke, and `otto ls` still answers. So keeping it
//! running is a convenience, not a correctness matter: `otto service start` has launchd do it (see
//! `launchd`), and nothing breaks when it is down.
//!
//! It binds 127.0.0.1 and nothing else, with no login. A POST here can start `claude` with
//! bypassed permissions, so "only this machine" has to hold against the browser too, not just the
//! network: see [`guard`] for how another website is kept from driving it.

mod assets;
mod handlers;
mod live;

use crate::error::OttoError;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;

pub const DEFAULT_PORT: u16 = 7878;

#[derive(clap::Args, Debug)]
pub struct ServeArgs {
    /// Port on 127.0.0.1 to listen on
    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub port: u16,
    /// Open the page in your browser once it is listening
    #[arg(long)]
    pub open: bool,
}

/// What every handler can see. `offset` is read before the runtime starts any thread — see
/// `clock::local_offset`.
#[derive(Clone)]
pub struct AppState {
    pub port: u16,
    pub offset: time::UtcOffset,
}

pub fn serve(args: ServeArgs) -> Result<(), OttoError> {
    let offset = crate::clock::local_offset();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| OttoError::usage(format!("cannot start the server runtime: {e}")))?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", args.port))
            .await
            .map_err(|e| OttoError::usage(format!("cannot listen on 127.0.0.1:{}: {e}", args.port)))?;
        let port = listener.local_addr()?.port();
        let url = format!("http://127.0.0.1:{port}/");
        println!("otto: serving {} on {url}", crate::paths::otto_home().display());
        println!("  Ctrl-C to stop; runs carry on without it");
        if args.open {
            let _ = std::process::Command::new("open").arg(&url).spawn();
        }
        axum::serve(listener, router(AppState { port, offset }))
            .await
            .map_err(|e| OttoError::usage(format!("the server stopped: {e}")))
    })
}

pub fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    let api = Router::new()
        .route("/meta", get(handlers::meta))
        .route("/usage", get(handlers::usage))
        .route("/config/workdir", post(handlers::set_workdir))
        .route("/runs", get(handlers::list_runs).post(handlers::start_run))
        .route("/runs/{id}", get(handlers::run_detail))
        .route("/runs/{id}/answer", post(handlers::answer))
        .route("/runs/{id}/notes", post(handlers::add_note))
        .route("/runs/{id}/notes/{note}/drop", post(handlers::drop_note))
        .route("/runs/{id}/period", post(handlers::period))
        .route("/runs/{id}/stop", post(handlers::stop))
        .route("/runs/{id}/resume", post(handlers::resume))
        .route("/runs/{id}/wake", post(handlers::wake))
        .route("/runs/{id}/logs", get(handlers::logs))
        .route("/runs/{id}/logs/stream", get(live::logs_stream))
        .route("/runs/{id}/live", get(live::live_stream))
        .route("/agent", get(handlers::agent_status))
        .route("/agent/start", post(handlers::agent_start))
        .route("/agent/stop", post(handlers::agent_stop))
        .route("/poke", post(handlers::poke));
    Router::new()
        .nest("/api", api)
        .route("/", get(assets::index))
        .route("/app.js", get(assets::app_js))
        .route("/style.css", get(assets::style_css))
        .route("/favicon.svg", get(assets::favicon))
        .layer(axum::middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Keep every other website out.
///
/// - **Host** must name this server. A DNS-rebinding page resolves its own hostname to 127.0.0.1
///   and would otherwise be same-origin with us; its requests still carry its own name here.
/// - **Origin**, when a browser sends one on a request that changes anything, must be this page.
/// - **Content-Type** on a POST must be JSON. A plain HTML form cannot send that, and a script
///   on another origin can only send it after a CORS preflight this server never approves.
async fn guard(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    if let Err(why) = check_request(state.port, request.method(), request.headers()) {
        return (StatusCode::FORBIDDEN, Json(ErrorBody { error: why.to_string(), code: 403 })).into_response();
    }
    next.run(request).await
}

fn check_request(port: u16, method: &Method, headers: &HeaderMap) -> Result<(), &'static str> {
    let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    if !ours.iter().any(|o| o == host) {
        return Err("this server only answers to 127.0.0.1 and localhost");
    }
    if method == Method::GET || method == Method::HEAD {
        return Ok(());
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let origin = origin.to_str().unwrap_or("");
        if !ours.iter().any(|o| origin == format!("http://{o}")) {
            return Err("cross-origin requests are refused");
        }
    }
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|c| c.to_str().ok())
        .is_some_and(|c| c.starts_with("application/json"));
    if !json {
        return Err("send JSON (Content-Type: application/json)");
    }
    Ok(())
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
    code: i32,
}

/// An `OttoError` as an HTTP answer: the status says which kind, the body says what the CLI would
/// have printed.
pub struct ApiError(pub OttoError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(ErrorBody { error: self.0.message, code: self.0.code })).into_response()
    }
}

/// Run a `core` call off the async threads — every one of them touches files, takes flocks, or
/// waits on a wake to start — and answer with its result as JSON.
pub async fn blocking<T, F>(f: F) -> Response
where
    T: Serialize + Send + 'static,
    F: FnOnce() -> Result<T, OttoError> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(err)) => ApiError(err).into_response(),
        Err(join) => ApiError(OttoError::new(format!("the request failed: {join}"), 4)).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::{open_gate, test_init, OpenGateArgs};
    use axum::body::Body;
    use tower::ServiceExt;

    const PORT: u16 = 7878;

    fn app() -> Router {
        router(AppState { port: PORT, offset: time::UtcOffset::UTC })
    }

    fn get_req(path: &str) -> axum::http::Request<Body> {
        axum::http::Request::get(path).header("host", format!("127.0.0.1:{PORT}")).body(Body::empty()).unwrap()
    }

    fn post_req(path: &str, body: serde_json::Value) -> axum::http::Request<Body> {
        axum::http::Request::post(path)
            .header("host", format!("127.0.0.1:{PORT}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn send(request: axum::http::Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    #[test]
    fn listing_runs_answers_json() {
        let _h = TempHome::new();
        test_init("2026-09-21-web", "a goal").unwrap();
        let (status, body) = rt().block_on(send(get_req("/api/runs")));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["rows"][0]["id"], "2026-09-21-web");
        assert_eq!(body["rows"][0]["short"], "web");
    }

    #[test]
    fn errors_keep_their_meaning_as_status_codes() {
        let _h = TempHome::new();
        test_init("2026-09-21-nogate", "a goal").unwrap();
        let (status, body) = rt().block_on(send(post_req("/api/runs/nogate/answer", serde_json::json!({"choice": "Approve"}))));
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body["error"].as_str().unwrap().contains("no gate open"));

        let (status, _) = rt().block_on(send(get_req("/api/runs/nothing-like-it")));
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn an_answer_from_the_page_closes_the_gate() {
        let _h = TempHome::new();
        test_init("2026-09-21-gate", "a goal").unwrap();
        open_gate(OpenGateArgs {
            id: "2026-09-21-gate".into(),
            slug: "review".into(),
            question: Some("Go?\n\n- Approve\n- Revise".into()),
            question_file: None,
            stdin: false,
            expires_at: None,
            expires_in: None,
        })
        .unwrap();
        let (status, body) =
            rt().block_on(send(post_req("/api/runs/gate/answer", serde_json::json!({"choice": "approve", "noWake": true}))));
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["answer"], "Approve");
        assert!(crate::state::read_run("2026-09-21-gate").unwrap().gate.is_none());
    }

    /// A note to a gated run is recorded, shown on the run, and never wakes it — the gate is
    /// still the question, and a note is not its answer.
    #[test]
    fn a_note_from_the_page_is_recorded_and_can_be_dropped() {
        let _h = TempHome::new();
        test_init("2026-09-21-noted", "a goal").unwrap();
        open_gate(OpenGateArgs {
            id: "2026-09-21-noted".into(),
            slug: "review".into(),
            question: Some("Go?".into()),
            question_file: None,
            stdin: false,
            expires_at: None,
            expires_in: None,
        })
        .unwrap();
        let (status, body) = rt().block_on(send(post_req(
            "/api/runs/noted/notes",
            serde_json::json!({"text": "never touch legacy/", "standing": true, "now": true}),
        )));
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["note"]["id"], "001");
        assert!(body["wake"].is_null());
        assert!(body["notWoken"].as_str().unwrap().contains("gate 001 is open"));

        let (_, detail) = rt().block_on(send(get_req("/api/runs/noted")));
        assert_eq!(detail["notes"][0]["text"], "never touch legacy/\n");
        assert_eq!(detail["notes"][0]["standing"], true);

        let (status, body) = rt().block_on(send(post_req("/api/runs/noted/notes/1/drop", serde_json::json!({}))));
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(crate::state::read_run("2026-09-21-noted").unwrap().notes.is_empty());

        let (status, _) = rt().block_on(send(post_req("/api/runs/noted/notes", serde_json::json!({"text": "  "}))));
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_dry_run_from_the_form_creates_nothing() {
        let h = TempHome::new();
        let workdir = h.path().display().to_string();
        let (status, body) = rt().block_on(send(post_req(
            "/api/runs?dryRun=true",
            serde_json::json!({"goal": "a goal", "slug": "dry", "workdir": workdir}),
        )));
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["planned"]["id"].as_str().unwrap().contains("dry"));
        assert!(crate::paths::all_run_ids().is_empty());
    }

    /// The page can't prompt, so a run with nowhere to run is refused rather than sent to
    /// wherever the server happened to start — until a default is set, which the page can do.
    #[test]
    fn a_run_from_the_form_needs_a_workdir_or_a_default() {
        let h = TempHome::new();
        let form = serde_json::json!({"goal": "a goal", "slug": "nowhere"});
        let (status, body) = rt().block_on(send(post_req("/api/runs?dryRun=true", form.clone())));
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

        let workdir = h.path().display().to_string();
        let (status, body) = rt().block_on(send(post_req("/api/config/workdir", serde_json::json!({"workdir": workdir}))));
        assert_eq!(status, StatusCode::OK, "{body}");
        let (_, meta) = rt().block_on(send(get_req("/api/meta")));
        assert_eq!(meta["workdir"].as_str(), Some(workdir.as_str()));

        let (status, body) = rt().block_on(send(post_req("/api/runs?dryRun=true", form)));
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    #[test]
    fn usage_answers_json_for_any_window() {
        let _h = TempHome::new();
        let (status, body) = rt().block_on(send(get_req("/api/usage?since=7d&by=day")));
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["by"], "day");
        assert_eq!(body["totals"]["wakes"], 0);
        let (status, body) = rt().block_on(send(get_req("/api/usage?since=")));
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["since"].is_null(), "an empty window is all time");
        let (status, _) = rt().block_on(send(get_req("/api/usage?since=lately")));
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_path_in_place_of_an_id_names_no_run() {
        let _h = TempHome::new();
        let (status, _) = rt().block_on(send(get_req("/api/runs/..%2F..")));
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// The reason the guard exists: another site in the same browser must not be able to stop,
    /// wake or start anything.
    #[test]
    fn other_websites_are_refused() {
        let _h = TempHome::new();
        test_init("2026-09-21-safe", "a goal").unwrap();

        let mut foreign = post_req("/api/runs/safe/stop", serde_json::json!({}));
        foreign.headers_mut().insert("origin", "http://evil.test".parse().unwrap());
        assert_eq!(rt().block_on(send(foreign)).0, StatusCode::FORBIDDEN);

        let mut rebound = get_req("/api/runs");
        rebound.headers_mut().insert("host", "evil.test:7878".parse().unwrap());
        assert_eq!(rt().block_on(send(rebound)).0, StatusCode::FORBIDDEN);

        let form = axum::http::Request::post("/api/runs/safe/stop")
            .header("host", format!("127.0.0.1:{PORT}"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("reason=x"))
            .unwrap();
        assert_eq!(rt().block_on(send(form)).0, StatusCode::FORBIDDEN);

        assert_eq!(crate::state::read_run("2026-09-21-safe").unwrap().status, crate::state::Status::Running);

        // The page itself is allowed.
        let mut ours = post_req("/api/runs/safe/stop", serde_json::json!({}));
        ours.headers_mut().insert("origin", format!("http://localhost:{PORT}").parse().unwrap());
        ours.headers_mut().insert("host", format!("localhost:{PORT}").parse().unwrap());
        // Stopping probes tmux for a session; with none running that is a harmless miss.
        let (status, body) = rt().block_on(send(ours));
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}
