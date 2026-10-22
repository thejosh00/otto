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

pub async fn standard_agents() -> Response {
    blocking(crate::agents::view).await
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
    /// The output it is about, as key or key@version.
    about: Option<String>,
}

pub async fn add_note(Path(id): Path<String>, Json(body): Json<NoteBody>) -> Response {
    blocking(move || {
        let text = crate::core::note_about(&id, body.about.as_deref(), &body.text)?;
        crate::core::note_in_background(&id, &text, body.standing, body.now)
    })
    .await
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
pub struct RenameBody {
    name: String,
}

pub async fn rename(Path(id): Path<String>, Json(body): Json<RenameBody>) -> Response {
    blocking(move || crate::core::rename_run(&id, &body.name)).await
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
pub struct DeleteBody {
    /// Kept unless the page says otherwise, as with `otto delete --yes`.
    data: crate::core::DataChoice,
}

/// The page has already asked.
pub async fn delete(Path(id): Path<String>, Json(body): Json<DeleteBody>) -> Response {
    blocking(move || crate::core::delete_run(&id, body.data, &mut crate::exec::RealExec)).await
}

pub async fn unused() -> Response {
    blocking(|| Ok::<_, OttoError>(crate::cleanup::find())).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupBody {
    /// As `GET /cleanup` lists them.
    paths: Vec<String>,
}

pub async fn cleanup(Json(body): Json<CleanupBody>) -> Response {
    blocking(move || crate::cleanup::remove(&body.paths)).await
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

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct InboxQuery {
    all: bool,
}

pub async fn inbox(Query(q): Query<InboxQuery>) -> Response {
    blocking(move || crate::inbox::inbox(q.all)).await
}

pub async fn health() -> Response {
    blocking(crate::core::health).await
}

pub async fn message(Path((id, message)): Path<(String, String)>) -> Response {
    blocking(move || {
        let id = crate::paths::resolve_run_id(&id)?;
        crate::messages::read(&id, Some(&message))
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDoneBody {
    reply: Option<String>,
    now: bool,
}

pub async fn message_done(Path((id, message)): Path<(String, String)>, Json(body): Json<MessageDoneBody>) -> Response {
    blocking(move || crate::core::message_done(&id, Some(&message), body.reply.as_deref(), body.now)).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputQuery {
    /// Which version; the latest when absent.
    v: Option<u32>,
}

pub async fn output(Path((id, key)): Path<(String, String)>, Query(q): Query<OutputQuery>) -> Response {
    blocking(move || {
        let id = crate::paths::resolve_run_id(&id)?;
        crate::outputs::read(&id, &key, q.v)
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputStateBody {
    state: crate::outputs::OutputState,
    #[serde(default)]
    note: Option<String>,
}

/// The person marks an output: "I sent it" is `done`.
pub async fn output_state(Path((id, key)): Path<(String, String)>, Json(body): Json<OutputStateBody>) -> Response {
    blocking(move || {
        let id = crate::paths::resolve_run_id(&id)?;
        crate::outputs::set_state(&id, &key, body.state, body.note.as_deref(), "person")
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PauseBody {
    /// A timestamp to pause until; with neither this nor `for`, until resumed.
    until: Option<String>,
    /// A length of time: `1h`, `3d`.
    #[serde(rename = "for")]
    for_: Option<String>,
    reason: Option<String>,
}

pub async fn pause(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<PauseBody>) -> Response {
    let offset = state.offset;
    blocking(move || {
        let until = match (body.for_.as_deref().filter(|s| !s.trim().is_empty()), body.until.as_deref().filter(|s| !s.trim().is_empty())) {
            (Some(age), _) => {
                let d = crate::clock::parse_age(age.trim())
                    .filter(|d| d.whole_minutes() > 0)
                    .ok_or_else(|| OttoError::usage(format!("pause for a length of time like 1h or 3d, not \"{age}\"")))?;
                Some(crate::clock::Timestamp::at(crate::clock::now() + d))
            }
            (None, Some(when)) => Some(crate::human::parse_until(when, offset)?),
            (None, None) => None,
        };
        crate::core::pause_run(&id, until, body.reason.filter(|r| !r.trim().is_empty()))
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

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangesQuery {
    all: bool,
}

pub async fn changes(Query(q): Query<ChangesQuery>) -> Response {
    blocking(move || crate::changes::list(q.all)).await
}

pub async fn change(Path((id, n)): Path<(String, u32)>, Query(q): Query<OutputQuery>) -> Response {
    blocking(move || crate::changes::read(&crate::changes::ChangeRef::parse(&format!("{id}/{n}"))?, q.v)).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApproveBody {
    /// `<run>/<number>`, one or more.
    changes: Vec<String>,
    #[serde(default)]
    note: Option<String>,
    /// As pull requests, or pushed, whatever their process says. Neither: as it says.
    #[serde(default)]
    pr: bool,
    #[serde(default)]
    push: bool,
    #[serde(default)]
    merge_when_ready: bool,
}

pub async fn approve_changes(Json(body): Json<ApproveBody>) -> Response {
    let how = crate::human::approve_how(body.pr, body.push, body.merge_when_ready);
    blocking(move || crate::core::approve_changes(&body.changes, how, body.note.as_deref(), false)).await
}

pub async fn merge_change(Path((id, n)): Path<(String, u32)>) -> Response {
    blocking(move || crate::core::merge_change(&format!("{id}/{n}"), false)).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PostBody {
    comment: Option<String>,
}

pub async fn post_replies(Path((id, n)): Path<(String, u32)>, Json(body): Json<PostBody>) -> Response {
    blocking(move || crate::core::post_replies(&format!("{id}/{n}"), body.comment.as_deref(), false)).await
}

pub async fn answer_change(Path((id, n)): Path<(String, u32)>, Json(body): Json<ChangeWordsBody>) -> Response {
    blocking(move || crate::core::answer_change(&format!("{id}/{n}"), body.text.as_deref().unwrap_or(""), false)).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeWordsBody {
    text: Option<String>,
}

pub async fn revise_change(Path((id, n)): Path<(String, u32)>, Json(body): Json<ChangeWordsBody>) -> Response {
    blocking(move || crate::core::revise_change(&format!("{id}/{n}"), body.text.as_deref().unwrap_or(""), false)).await
}

pub async fn drop_change(Path((id, n)): Path<(String, u32)>, Json(body): Json<ChangeWordsBody>) -> Response {
    blocking(move || crate::core::drop_change(&format!("{id}/{n}"), body.text.as_deref(), false)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessesView {
    processes: Vec<ProcessRow>,
    /// Repositories given a process other than `standard`.
    projects: Vec<ProjectRow>,
    /// Repositories changes have come from, for picking one.
    known: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessRow {
    #[serde(flatten)]
    process: crate::config::Process,
    /// `standard` as otto ships it, not redefined in `config.json`.
    builtin: bool,
    projects: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectRow {
    repo: String,
    process: String,
}

/// Every process, who follows each, and the repositories there are to give one.
pub async fn processes() -> Response {
    blocking(|| {
        let config = crate::config::load()?;
        let processes = config
            .processes()
            .into_iter()
            .map(|p| ProcessRow {
                builtin: !config.processes.iter().any(|q| q.name.eq_ignore_ascii_case(&p.name)),
                projects: config.projects_of(&p.name).iter().map(|r| crate::changes::home_relative(r)).collect(),
                process: p,
            })
            .collect();
        let projects = config.projects.iter().map(|(repo, p)| ProjectRow { repo: crate::changes::home_relative(repo), process: p.clone() }).collect();
        let mut known: Vec<String> = crate::changes::list(true)?.changes.into_iter().map(|l| l.repo).collect();
        known.sort();
        known.dedup();
        Ok::<_, OttoError>(ProcessesView { processes, projects, known })
    })
    .await
}

/// Make or change a process, as `otto config process` does.
pub async fn save_process(Json(body): Json<crate::config::Process>) -> Response {
    blocking(move || crate::config::save_process(body)).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct NameBody {
    name: String,
}

pub async fn delete_process(Json(body): Json<NameBody>) -> Response {
    blocking(move || crate::config::delete_process(&body.name).map(|()| serde_json::json!({ "deleted": body.name }))).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectBody {
    repo: String,
    /// `None`, or `standard`: the standard process again.
    process: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectSet {
    repo: String,
    process: crate::config::Process,
}

/// Have a repository follow a process, as `otto config project --process` does.
pub async fn set_project(Json(body): Json<ProjectBody>) -> Response {
    blocking(move || {
        let (repo, process) = crate::config::save_project(&body.repo, body.process.as_deref())?;
        Ok::<_, OttoError>(ProjectSet { repo: crate::changes::home_relative(&repo), process })
    })
    .await
}
