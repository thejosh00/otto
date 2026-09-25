//! One handler per action, each a thin JSON wrapper over the `core` call the matching CLI
//! command makes.

use super::{blocking, AppState};
use crate::core::{Caller, LogQuery};
use crate::error::OttoError;
use crate::state::commands::InitArgs;
use crate::state::Detach;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    version: &'static str,
    home: String,
}

pub async fn meta() -> Response {
    blocking(|| Ok(Meta { version: env!("CARGO_PKG_VERSION"), home: crate::paths::otto_home().display().to_string() })).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ListQuery {
    all: bool,
}

pub async fn list_runs(Query(q): Query<ListQuery>) -> Response {
    blocking(move || crate::core::list_runs(q.all)).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct StartQuery {
    dry_run: bool,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Started {
    planned: Option<crate::core::Planned>,
    id: Option<String>,
    wake: Option<crate::core::WakeStart>,
    /// The run exists, but its first wake did not start; say so rather than hide a run that was
    /// created.
    wake_error: Option<String>,
}

pub async fn start_run(Query(q): Query<StartQuery>, Json(init): Json<InitArgs>) -> Response {
    blocking(move || {
        if q.dry_run {
            return Ok(Started { planned: Some(crate::core::plan_run(&init)?), ..Default::default() });
        }
        let detach = init.detach;
        let id = crate::core::create_run(init)?;
        let mut started = Started { id: Some(id.clone()), ..Default::default() };
        match crate::core::start_wake(&id, Caller::Background.strategy(detach), None) {
            Ok(wake) => started.wake = Some(wake),
            Err(err) => started.wake_error = Some(err.message),
        }
        Ok(started)
    })
    .await
}

/// `_` means "the run you'd mean with no id", as `otto show` with none does.
pub async fn run_detail(Path(id): Path<String>) -> Response {
    blocking(move || crate::core::run_detail(if id == "_" { None } else { Some(&id) })).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AnswerBody {
    choice: Option<String>,
    text: Option<String>,
    no_wake: bool,
}

pub async fn answer(Path(id): Path<String>, Json(body): Json<AnswerBody>) -> Response {
    blocking(move || crate::core::answer_in_background(&id, body.choice.as_deref(), body.text.as_deref(), body.no_wake)).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct StopBody {
    reason: Option<String>,
    failed: bool,
}

pub async fn stop(Path(id): Path<String>, Json(body): Json<StopBody>) -> Response {
    blocking(move || {
        let reason = body.reason.filter(|r| !r.trim().is_empty());
        crate::core::stop_run(&id, reason, body.failed, &mut crate::exec::RealExec)
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeBody {
    detach: Option<Detach>,
}

pub async fn wake(Path(id): Path<String>, Json(body): Json<WakeBody>) -> Response {
    blocking(move || crate::core::wake_in_background(&id, body.detach)).await
}

/// The query string both log endpoints take. `event` is comma-separated, like `--event`.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct LogParams {
    n: Option<usize>,
    since: Option<String>,
    event: Option<String>,
    decisions: bool,
}

impl LogParams {
    pub fn query(&self) -> LogQuery {
        LogQuery {
            lines: self.n,
            since: self.since.clone().filter(|s| !s.trim().is_empty()),
            events: self.event.as_deref().unwrap_or("").split(',').map(str::to_string).collect(),
            decisions: self.decisions,
        }
    }
}

#[derive(Serialize)]
struct Logs {
    header: String,
    lines: Vec<crate::core::LogLine>,
}

pub async fn logs(State(state): State<Arc<AppState>>, Path(id): Path<String>, Query(params): Query<LogParams>) -> Response {
    let offset = state.offset;
    blocking(move || {
        let page = crate::core::read_logs(&id, &params.query(), offset)?;
        Ok(Logs { header: page.header, lines: page.lines })
    })
    .await
}

pub async fn agent_status() -> Response {
    blocking(crate::launchd::agent_status).await
}

pub async fn agent_start() -> Response {
    blocking(crate::launchd::start_agent).await
}

pub async fn agent_stop() -> Response {
    blocking(crate::launchd::stop_agent).await
}

pub async fn poke() -> Response {
    blocking(|| Ok::<_, OttoError>(crate::poke::poke_pass(&crate::poke::PokeArgs::default()))).await
}
