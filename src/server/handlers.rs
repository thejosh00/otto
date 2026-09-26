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
    /// Every launcher the new-run form can offer, `claude` first.
    launchers: Vec<String>,
    /// The default working directory as configured (`~/work`), or `None` — the form then asks.
    workdir: Option<String>,
}

pub async fn meta() -> Response {
    blocking(|| {
        let config = crate::config::load()?;
        let launchers = config.launchers().into_iter().map(|l| l.name).collect();
        Ok(Meta {
            version: env!("CARGO_PKG_VERSION"),
            home: crate::paths::otto_home().display().to_string(),
            launchers,
            workdir: config.workdir,
        })
    })
    .await
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
        // The terminal asks for a default; the page has to be told one. Refused here rather
        // than letting the run fall back to wherever the server was started.
        if init.workdir.is_none() && crate::config::load()?.workdir.is_none() {
            return Err(OttoError::usage(
                "no working directory: give one for this run, or set a default (`otto config workdir <dir>`)",
            ));
        }
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
pub struct NoteBody {
    text: String,
    standing: bool,
    now: bool,
}

pub async fn add_note(Path(id): Path<String>, Json(body): Json<NoteBody>) -> Response {
    blocking(move || crate::core::note_in_background(&id, &body.text, body.standing, body.now)).await
}

pub async fn drop_note(Path((id, note)): Path<(String, String)>) -> Response {
    blocking(move || crate::core::drop_note(&id, &note)).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkdirBody {
    workdir: String,
}

#[derive(Serialize)]
struct WorkdirSaved {
    workdir: String,
}

/// Set the default working directory, as `otto config workdir` does.
pub async fn set_workdir(Json(body): Json<WorkdirBody>) -> Response {
    blocking(move || {
        let resolved = crate::config::save_workdir(&body.workdir)?;
        Ok(WorkdirSaved { workdir: resolved.display().to_string() })
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PeriodBody {
    /// A duration, as `otto period` takes it: `30m`, `4h`, `1d`.
    period: String,
}

pub async fn period(Path(id): Path<String>, Json(body): Json<PeriodBody>) -> Response {
    blocking(move || {
        let minutes = crate::clock::parse_age(body.period.trim())
            .map(|d| d.whole_minutes())
            .filter(|m| *m >= 1)
            .ok_or_else(|| OttoError::usage(format!("a period is a duration like 30m, 4h or 1d, not \"{}\"", body.period)))?;
        crate::core::set_period(&id, minutes as u64)
    })
    .await
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
pub struct ResumeBody {
    reason: Option<String>,
    no_wake: bool,
}

pub async fn resume(Path(id): Path<String>, Json(body): Json<ResumeBody>) -> Response {
    blocking(move || {
        let reason = body.reason.filter(|r| !r.trim().is_empty());
        crate::core::resume_run(&id, reason, body.no_wake)
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

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageQuery {
    /// An age, date or timestamp; absent is all time.
    since: Option<String>,
    by: crate::core::UsageBy,
}

pub async fn usage(State(state): State<Arc<AppState>>, Query(q): Query<UsageQuery>) -> Response {
    let offset = state.offset;
    blocking(move || crate::core::usage(q.since.as_deref().filter(|s| !s.is_empty()), q.by, offset)).await
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
