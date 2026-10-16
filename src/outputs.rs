//! Outputs: what a run made that a person would want to find again — a day's report, a design
//! document as it evolves, an explanation and the reply drafted for it, the tickets it filed.
//!
//! A message is a notification: it waits in the inbox until the person is done with it, and then it
//! is gone from view. That was the only home a run's work had, so last Tuesday's report lived in the
//! inbox's done pile and a design document reviewed twice was two unrelated messages. An output is
//! the work itself, kept: published with `otto state publish`, listed on the run's page under "Its
//! work", and pointed at by a message (`--output`) when the person should be told it is there.
//!
//! One model covers every shape of work, so nobody declares an agent's type:
//!
//! - an output has a **key**, which says what it is. Publishing a new key adds a thing — a daily
//!   report is a new key each day. Publishing a key again adds a **version** of the same thing — a
//!   design revised after review is `design` v2.
//! - a run may declare its **stages** (`otto state stages`), and tag each output with one — research,
//!   design, plan — for work that moves through steps.
//! - an output may carry a **draft** (text for the person to send on) and a **state** (waiting on
//!   them, done, or not needed) — one per question in a channel, each with its reply.
//!
//! The page picks its layout from which of those are present. A question can be **about** a version
//! (`open-gate --about design`), and the answer is kept on that version as its review.
//!
//! On disk: `outputs/index.json` is everything but the content; `outputs/<key>/v<n>.<ext>` is each
//! version's text, verbatim. Work that lives elsewhere — a ticket, a merged PR — is a version with a
//! `link` instead of a file. `artifacts/` stays the run's scratch space: an output is published from
//! there on purpose, so the page shows the work and not the workings.

use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::event::Event;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const OUTPUTS_DIR: &str = "outputs";
pub const INDEX_FILE: &str = "outputs/index.json";
/// A title is a line in a list.
pub const TITLE_MAX_CHARS: usize = 120;
/// A key names a folder, and is typed back: `2026-10-16-refund-policy-qa`, `design`.
pub const KEY_MAX_CHARS: usize = 80;
/// A version is a document a person reads, not a data dump.
pub const FILE_MAX_BYTES: usize = 512 * 1024;
/// A draft is a reply someone will paste.
pub const DRAFT_MAX_BYTES: usize = 8 * 1024;
pub const ABOUT_MAX_CHARS: usize = 400;
pub const STAGES_MAX: usize = 12;
pub const STAGE_MAX_CHARS: usize = 40;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    /// The steps the work moves through, in order, when it declared them (`otto state stages`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<String>,
    /// In the order first published.
    #[serde(default)]
    pub outputs: Vec<Output>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub key: String,
    /// The latest version's title.
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// What it responds to — a thread, a ticket — usually a link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<OutputState>,
    /// Why it is in that state, in a few words: "Sam answered it in the thread".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_note: Option<String>,
    /// `person` or `wake`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_at: Option<Timestamp>,
    pub versions: Vec<Version>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub n: u32,
    pub at: Timestamp,
    /// The wake that published it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<u32>,
    /// Its text, relative to the run directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Or where it lives, for work kept elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Text for the person to send on, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<u32>,
    /// What the person answered to a question about this version.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<Review>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub at: Timestamp,
    pub gate: String,
    pub answer: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum OutputState {
    /// It waits on the person: a draft to send, something to act on
    Waiting,
    /// The person has dealt with it — sent the draft, acted on it
    Done,
    /// Not needed any more — someone else answered, it was overtaken
    Dropped,
}

impl OutputState {
    pub fn as_str(self) -> &'static str {
        match self {
            OutputState::Waiting => "waiting",
            OutputState::Done => "done",
            OutputState::Dropped => "dropped",
        }
    }
}

impl Output {
    pub fn latest(&self) -> &Version {
        self.versions.last().expect("an output has at least one version")
    }

    pub fn version(&self, n: u32) -> Option<&Version> {
        self.versions.iter().find(|v| v.n == n)
    }
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

pub fn read_index(run_dir: &Path) -> Result<Index, OttoError> {
    match std::fs::read_to_string(run_dir.join(INDEX_FILE)) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| OttoError::conflict(format!("{INDEX_FILE} is not valid: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::default()),
        Err(e) => Err(e.into()),
    }
}

/// For views: an unreadable index shows as no outputs rather than failing the whole page.
pub fn index_or_empty(run_dir: &Path) -> Index {
    read_index(run_dir).unwrap_or_default()
}

fn write_index(run_dir: &Path, index: &Index) -> Result<(), OttoError> {
    std::fs::create_dir_all(run_dir.join(OUTPUTS_DIR))?;
    crate::state::write_atomic(&run_dir.join(INDEX_FILE), &(serde_json::to_string_pretty(index)? + "\n"))
}

/// A key as stored: lower case, `[a-z0-9._-]`, starting with a letter or digit. Upper case is
/// folded rather than refused, so `PROJ-4412` works and can't collide with `proj-4412` on a
/// case-insensitive disk.
pub fn normalise_key(key: &str) -> Result<String, OttoError> {
    let key = key.trim().to_lowercase();
    let ok = !key.is_empty()
        && key.chars().count() <= KEY_MAX_CHARS
        && key.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !key.contains("..");
    if ok {
        Ok(key)
    } else {
        Err(OttoError::usage(format!(
            "--key {key:?}: say what it is in letters, digits, '-', '_' or '.', starting with a letter or digit, \
             at most {KEY_MAX_CHARS} characters — `design`, `2026-10-16-refund-policy-qa`"
        )))
    }
}

fn check_title(title: &str) -> Result<String, OttoError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(OttoError::usage("an output needs a title — what it is, in a few words, for a list"));
    }
    if title.contains('\n') {
        return Err(OttoError::usage("an output's title is one line"));
    }
    if title.chars().count() > TITLE_MAX_CHARS {
        return Err(OttoError::usage(format!(
            "an output's title is at most {TITLE_MAX_CHARS} characters (this one is {})",
            title.chars().count()
        )));
    }
    Ok(title.to_string())
}

fn check_link(link: &str) -> Result<String, OttoError> {
    let link = link.trim();
    if (link.starts_with("https://") || link.starts_with("http://")) && !link.contains(char::is_whitespace) {
        Ok(link.to_string())
    } else {
        Err(OttoError::usage(format!("--link {link:?}: an http(s) address")))
    }
}

/// `stage` as the run declared it, matched without regard to case.
fn find_stage(index: &Index, stage: &str) -> Result<String, OttoError> {
    if index.stages.is_empty() {
        return Err(OttoError::usage(
            "--stage: this run has declared no stages — `otto state stages <id> <first> <second> …` first",
        ));
    }
    index.stages.iter().find(|s| s.eq_ignore_ascii_case(stage.trim())).cloned().ok_or_else(|| {
        OttoError::usage(format!("--stage {stage:?} is not one of this run's stages: {}", index.stages.join(", ")))
    })
}

fn words(text: &str) -> u32 {
    text.split_whitespace().count().min(u32::MAX as usize) as u32
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

/// What a wake hands `publish`, already read from wherever the CLI found it.
#[derive(Default)]
pub struct Publish<'a> {
    pub key: &'a str,
    pub title: &'a str,
    /// The text of this version, and the extension to keep it under (`md`, `txt`, `csv`).
    pub text: Option<(&'a str, &'a str)>,
    pub link: Option<&'a str>,
    pub stage: Option<&'a str>,
    pub about: Option<&'a str>,
    pub draft: Option<&'a str>,
    pub state: Option<OutputState>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Published {
    pub key: String,
    pub version: u32,
    /// Where the text went, relative to the run directory, when there is text.
    pub file: Option<String>,
    /// Nothing differed from the latest version, so no new version was made.
    pub unchanged: bool,
}

/// Publish a version of `key`: a new output when the key is new, its next version otherwise. A
/// version identical to the latest — same text, link and draft — is not a new version: a wake
/// that republishes what it already published changes nothing.
pub fn publish(run_id: &str, p: &Publish) -> Result<Published, OttoError> {
    let key = normalise_key(p.key)?;
    let title = check_title(p.title)?;
    let link = p.link.map(check_link).transpose()?;
    let text = match p.text {
        Some((text, ext)) => {
            let text = text.trim_end();
            if text.trim().is_empty() {
                return Err(OttoError::usage("the file is empty — an output is something a person reads"));
            }
            if text.len() > FILE_MAX_BYTES {
                return Err(OttoError::usage(format!(
                    "an output is at most {FILE_MAX_BYTES} bytes (this one is {}) — publish the part a person reads",
                    text.len()
                )));
            }
            let ext = ext.trim_start_matches('.').to_lowercase();
            let ext = if !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()) { ext } else { "md".into() };
            Some((text.to_string(), ext))
        }
        None => None,
    };
    if text.is_some() == link.is_some() {
        return Err(OttoError::usage("give the output either a file (--file, --stdin) or a --link, not both and not neither"));
    }
    let draft = p.draft.map(str::trim_end).filter(|d| !d.trim().is_empty()).map(str::to_string);
    if draft.as_ref().is_some_and(|d| d.len() > DRAFT_MAX_BYTES) {
        return Err(OttoError::usage(format!("a draft is at most {DRAFT_MAX_BYTES} bytes")));
    }
    let about = p.about.map(str::trim).filter(|a| !a.is_empty()).map(str::to_string);
    if about.as_ref().is_some_and(|a| a.chars().count() > ABOUT_MAX_CHARS || a.contains('\n')) {
        return Err(OttoError::usage(format!("--about is one line of at most {ABOUT_MAX_CHARS} characters — a link, or what it answers")));
    }

    crate::state::transaction(run_id, |path, state| {
        let mut index = read_index(path)?;
        let stage = p.stage.map(|s| find_stage(&index, s)).transpose()?;
        let at = Timestamp::now();
        let wake = state.wake.as_ref().filter(|w| w.outcome.is_none()).map(|w| w.n);
        let existing = index.outputs.iter().position(|o| o.key == key);

        if let Some(i) = existing {
            let latest = index.outputs[i].latest().clone();
            let same_text = match (&text, &latest.file) {
                (Some((t, _)), Some(file)) => std::fs::read_to_string(path.join(file)).is_ok_and(|old| old.trim_end() == t),
                (None, None) => true,
                _ => false,
            };
            let unchanged = same_text && latest.link == link && latest.draft == draft;
            if unchanged {
                // Only the bookkeeping moves: a new title, stage or state needs no new version.
                let output = &mut index.outputs[i];
                output.title = title.clone();
                if stage.is_some() {
                    output.stage = stage.clone();
                }
                if about.is_some() {
                    output.about = about.clone();
                }
                if let Some(s) = p.state {
                    set_state_on(output, s, None, "wake", at);
                }
                write_index(path, &index)?;
                return Ok(Published { key, version: latest.n, file: latest.file, unchanged: true });
            }
        }

        let n = existing.map(|i| index.outputs[i].latest().n + 1).unwrap_or(1);
        let file = match &text {
            Some((t, ext)) => {
                let rel = format!("{OUTPUTS_DIR}/{key}/v{n}.{ext}");
                std::fs::create_dir_all(path.join(OUTPUTS_DIR).join(&key))?;
                crate::state::write_atomic(&path.join(&rel), &format!("{t}\n"))?;
                Some(rel)
            }
            None => None,
        };
        let version = Version {
            n,
            at,
            wake,
            file: file.clone(),
            link: link.clone(),
            draft: draft.clone(),
            words: text.as_ref().map(|(t, _)| words(t)),
            reviews: Vec::new(),
        };
        let output = match existing {
            Some(i) => {
                let output = &mut index.outputs[i];
                output.title = title.clone();
                output.versions.push(version);
                output
            }
            None => {
                index.outputs.push(Output {
                    key: key.clone(),
                    title: title.clone(),
                    stage: None,
                    about: None,
                    state: None,
                    state_note: None,
                    state_by: None,
                    state_at: None,
                    versions: vec![version],
                });
                index.outputs.last_mut().expect("just pushed")
            }
        };
        if stage.is_some() {
            output.stage = stage.clone();
        }
        if about.is_some() {
            output.about = about;
        }
        // A new draft waits on the person unless the wake says otherwise.
        match p.state {
            Some(s) => set_state_on(output, s, None, "wake", at),
            None if draft.is_some() => set_state_on(output, OutputState::Waiting, None, "wake", at),
            None => {}
        }
        write_index(path, &index)?;
        crate::event::record(
            path,
            &Event::OutputPublished { output: key.clone(), version: n, title, stage, file: file.clone(), link },
        )?;
        Ok(Published { key, version: n, file, unchanged: false })
    })
}

fn set_state_on(output: &mut Output, state: OutputState, note: Option<String>, by: &str, at: Timestamp) {
    output.state = Some(state);
    output.state_note = note;
    output.state_by = Some(by.to_string());
    output.state_at = Some(at);
}

/// Declare the steps the run's work moves through, in order. Every output already tagged with a
/// stage must keep a stage that exists.
pub fn set_stages(run_id: &str, stages: &[String]) -> Result<Vec<String>, OttoError> {
    let stages: Vec<String> = stages.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if stages.is_empty() {
        return Err(OttoError::usage("name at least one stage, in order: research design plan"));
    }
    if stages.len() > STAGES_MAX {
        return Err(OttoError::usage(format!("at most {STAGES_MAX} stages")));
    }
    for (i, s) in stages.iter().enumerate() {
        if s.chars().count() > STAGE_MAX_CHARS || s.contains('\n') {
            return Err(OttoError::usage(format!("a stage is a few words, at most {STAGE_MAX_CHARS} characters: {s:?}")));
        }
        if stages[..i].iter().any(|t| t.eq_ignore_ascii_case(s)) {
            return Err(OttoError::usage(format!("stage {s:?} is named twice")));
        }
    }
    crate::state::transaction(run_id, |path, _state| {
        let mut index = read_index(path)?;
        if let Some(orphan) = index
            .outputs
            .iter()
            .find(|o| o.stage.as_ref().is_some_and(|st| !stages.iter().any(|s| s.eq_ignore_ascii_case(st))))
        {
            return Err(OttoError::conflict(format!(
                "output {} is in stage {:?}, which the new list drops — keep it, or republish that output in another stage first",
                orphan.key,
                orphan.stage.as_deref().unwrap_or("")
            )));
        }
        // Outputs keep the stage's spelling as now declared.
        for output in &mut index.outputs {
            if let Some(st) = output.stage.as_mut() {
                if let Some(s) = stages.iter().find(|s| s.eq_ignore_ascii_case(st)) {
                    *st = s.clone();
                }
            }
        }
        index.stages = stages.clone();
        write_index(path, &index)?;
        crate::event::record(path, &Event::StagesSet { stages: stages.clone() })?;
        Ok(stages.clone())
    })
}

/// Mark an output waiting, done or not needed — by the person ("I sent it") or by a wake (someone
/// else answered the question its draft was for).
pub fn set_state(run_id: &str, key: &str, state: OutputState, note: Option<&str>, by: &str) -> Result<Output, OttoError> {
    let key = normalise_key(key)?;
    let note = note.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    crate::state::transaction(run_id, |path, run| {
        let mut index = read_index(path)?;
        let output = index
            .outputs
            .iter_mut()
            .find(|o| o.key == key)
            .ok_or_else(|| OttoError::not_found(format!("{} has no output {key}", run.label())))?;
        set_state_on(output, state, note.clone(), by, Timestamp::now());
        let out = output.clone();
        write_index(path, &index)?;
        crate::event::record(path, &Event::OutputState { output: key.clone(), state, by: by.to_string(), note })?;
        Ok(out)
    })
}

// ---------------------------------------------------------------------------
// Questions about a version, and notes about one
// ---------------------------------------------------------------------------

/// `design` or `design@2` → `design@2`, checked against what exists: a question is about the
/// version that was current when it was asked, even after a later one lands.
pub fn resolve_about(run_dir: &Path, about: &str) -> Result<String, OttoError> {
    let (key, n) = match about.trim().split_once('@') {
        Some((key, n)) => (key, Some(n.trim_start_matches('v').parse::<u32>().map_err(|_| OttoError::usage(format!("--about {about:?}: key@version, like design@2")))?)),
        None => (about.trim(), None),
    };
    let key = normalise_key(key)?;
    let index = read_index(run_dir)?;
    let output = index
        .outputs
        .iter()
        .find(|o| o.key == key)
        .ok_or_else(|| OttoError::usage(format!("--about {key}: no output has that key — publish it first")))?;
    let n = match n {
        Some(n) => output.version(n).map(|v| v.n).ok_or_else(|| OttoError::usage(format!("--about: {key} has no version {n}")))?,
        None => output.latest().n,
    };
    Ok(format!("{key}@{n}"))
}

fn split_about(about: &str) -> Option<(&str, u32)> {
    let (key, n) = about.split_once('@')?;
    Some((key, n.parse().ok()?))
}

/// Keep the person's answer on the version the question was about. Called with the run's lock
/// held, from closing the gate; a missing output is no reason to fail the answer, so it is skipped.
pub fn record_review(run_dir: &Path, about: &str, gate: &str, answer: &str, at: Timestamp) -> Result<(), OttoError> {
    let Some((key, n)) = split_about(about) else { return Ok(()) };
    let mut index = read_index(run_dir)?;
    let Some(version) = index.outputs.iter_mut().find(|o| o.key == key).and_then(|o| o.versions.iter_mut().find(|v| v.n == n)) else {
        return Ok(());
    };
    version.reviews.push(Review { at, gate: gate.to_string(), answer: answer.to_string() });
    write_index(run_dir, &index)
}

/// The opening line of a note about an output, so the wake knows which one, and the page can say so.
pub const NOTE_ABOUT_PREFIX: &str = "About output ";

/// A note's text when it is about `key@n`: the line naming the output, then the person's words.
pub fn note_text(run_dir: &Path, about: &str, text: &str) -> Result<String, OttoError> {
    let about = resolve_about(run_dir, about)?;
    let (key, _) = split_about(&about).expect("resolve_about returns key@n");
    let title = read_index(run_dir)?.outputs.into_iter().find(|o| o.key == key).map(|o| o.title).unwrap_or_default();
    Ok(format!("{NOTE_ABOUT_PREFIX}{about}, \"{title}\":\n\n{}", text.trim()))
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// One output, as a list shows it: everything but the text.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputLine {
    #[serde(flatten)]
    pub output: Output,
    pub first_at: Timestamp,
    pub latest_at: Timestamp,
    /// The open question is about one of its versions.
    pub asking: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageView {
    pub name: String,
    /// `done`, `current` or `later`.
    pub status: &'static str,
    pub outputs: usize,
}

/// A run's work as its page shows it.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputsView {
    pub stages: Vec<StageView>,
    /// Newest change first.
    pub outputs: Vec<OutputLine>,
}

/// The run's outputs, newest change first, and where its stages stand: the current stage is the
/// one the open question is about, else the one the latest output was published in.
pub fn view(run_dir: &Path, state: &crate::state::RunState) -> OutputsView {
    let index = index_or_empty(run_dir);
    let asking_key = state.gate.as_ref().and_then(|g| g.about.as_deref()).and_then(split_about).map(|(k, _)| k.to_string());
    let mut outputs: Vec<(usize, OutputLine)> = index
        .outputs
        .iter()
        .filter(|o| !o.versions.is_empty())
        .map(|o| OutputLine {
            first_at: o.versions[0].at,
            latest_at: o.latest().at,
            asking: asking_key.as_deref() == Some(o.key.as_str()),
            output: o.clone(),
        })
        .enumerate()
        .collect();
    // Newest change first; within one second, the one published later.
    outputs.sort_by(|(i, a), (j, b)| b.latest_at.cmp(&a.latest_at).then(j.cmp(i)));
    let outputs: Vec<OutputLine> = outputs.into_iter().map(|(_, o)| o).collect();
    let current_stage = asking_key
        .as_deref()
        .and_then(|k| outputs.iter().find(|o| o.output.key == k))
        .and_then(|o| o.output.stage.clone())
        .or_else(|| outputs.iter().find_map(|o| o.output.stage.clone()));
    let current = current_stage.and_then(|s| index.stages.iter().position(|t| *t == s)).unwrap_or(0);
    let stages = index
        .stages
        .iter()
        .enumerate()
        .map(|(i, name)| StageView {
            name: name.clone(),
            status: match i.cmp(&current) {
                std::cmp::Ordering::Less => "done",
                std::cmp::Ordering::Equal => "current",
                std::cmp::Ordering::Greater => "later",
            },
            outputs: outputs.iter().filter(|o| o.output.stage.as_deref() == Some(name)).count(),
        })
        .collect();
    OutputsView { stages, outputs }
}

/// One version of one output, with its text.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionView {
    pub output: Output,
    pub version: Version,
    /// `None` for a link, or a file gone missing.
    pub text: Option<String>,
}

/// `key`'s version `n`, or its latest.
pub fn read(run_id: &str, key: &str, n: Option<u32>) -> Result<VersionView, OttoError> {
    let state = crate::state::read_run(run_id)?;
    let dir = crate::paths::run_dir(&state.id)?;
    let key = normalise_key(key)?;
    let output = read_index(&dir)?
        .outputs
        .into_iter()
        .find(|o| o.key == key)
        .ok_or_else(|| OttoError::not_found(format!("{} has no output {key}", state.label())))?;
    let version = match n {
        Some(n) => output.version(n).cloned().ok_or_else(|| OttoError::not_found(format!("{key} has no version {n}")))?,
        None => output.latest().clone(),
    };
    let text = version.file.as_ref().and_then(|f| std::fs::read_to_string(dir.join(f)).ok());
    Ok(VersionView { output, version, text })
}

/// What a message points at, as the inbox shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputRef {
    pub key: String,
    pub version: u32,
    pub title: String,
    pub at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<OutputState>,
    pub versions: u32,
}

pub fn reference(run_dir: &Path, about: &str) -> Option<OutputRef> {
    let (key, n) = split_about(about)?;
    let output = read_index(run_dir).ok()?.outputs.into_iter().find(|o| o.key == key)?;
    let version = output.version(n)?.clone();
    Some(OutputRef {
        key: output.key.clone(),
        version: n,
        title: output.title.clone(),
        at: version.at,
        words: version.words,
        link: version.link,
        draft: version.draft,
        state: output.state,
        versions: output.latest().n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;

    fn doc<'a>(key: &'a str, title: &'a str, text: &'a str) -> Publish<'a> {
        Publish { key, title, text: Some((text, "md")), ..Default::default() }
    }

    fn journal(id: &str) -> String {
        std::fs::read_to_string(crate::paths::run_dir(id).unwrap().join("journal.jsonl")).unwrap()
    }

    #[test]
    fn a_new_key_is_a_new_output_and_the_same_key_a_new_version() {
        let _h = TempHome::new();
        test_init("o-ver", "a goal").unwrap();
        let first = publish("o-ver", &doc("2026-10-16-refund-policy-qa", "refund-policy-qa: a stale index", "It isn't the model.")).unwrap();
        assert_eq!((first.version, first.unchanged), (1, false));
        assert_eq!(first.file.as_deref(), Some("outputs/2026-10-16-refund-policy-qa/v1.md"));
        publish("o-ver", &doc("2026-10-15-tool-call-json", "tool-call-json: the grader", "The grader is wrong.")).unwrap();

        let dir = crate::paths::run_dir("o-ver").unwrap();
        assert_eq!(read_index(&dir).unwrap().outputs.len(), 2, "a key a day is a series");

        let again = publish("o-ver", &doc("2026-10-16-refund-policy-qa", "refund-policy-qa: a stale index", "It isn't the model.")).unwrap();
        assert!(again.unchanged, "the same text again is not a new version");
        let v2 = publish("o-ver", &doc("2026-10-16-refund-policy-qa", "refund-policy-qa: a stale index, fixed", "It isn't the model.\n\nFixed now.")).unwrap();
        assert_eq!(v2.version, 2);
        let out = read("o-ver", "2026-10-16-REFUND-policy-qa", None).unwrap();
        assert_eq!(out.output.title, "refund-policy-qa: a stale index, fixed", "the latest title, and the key is case-blind");
        assert_eq!(out.text.as_deref(), Some("It isn't the model.\n\nFixed now.\n"));
        assert_eq!(read("o-ver", "2026-10-16-refund-policy-qa", Some(1)).unwrap().text.as_deref(), Some("It isn't the model.\n"));
        let j = journal("o-ver");
        assert_eq!(j.matches("\"output-published\"").count(), 3, "the unchanged publish is not journaled: {j}");
    }

    #[test]
    fn bad_outputs_are_refused_whole() {
        let _h = TempHome::new();
        test_init("o-bad", "a goal").unwrap();
        for key in ["", "../x", "-lead", "has space", &"k".repeat(KEY_MAX_CHARS + 1)] {
            assert!(publish("o-bad", &doc(key, "t", "x")).is_err(), "{key:?}");
        }
        assert!(publish("o-bad", &doc("k", "", "x")).is_err());
        assert!(publish("o-bad", &doc("k", "a\nb", "x")).is_err());
        assert!(publish("o-bad", &doc("k", "t", "  \n")).is_err());
        assert!(publish("o-bad", &Publish { key: "k", title: "t", ..Default::default() }).is_err(), "neither file nor link");
        let both = Publish { link: Some("https://x"), ..doc("k", "t", "x") };
        assert!(publish("o-bad", &both).is_err());
        let not_a_link = Publish { key: "k", title: "t", link: Some("PROJ-1"), ..Default::default() };
        assert!(publish("o-bad", &not_a_link).is_err());
        let stage = Publish { stage: Some("design"), ..doc("k", "t", "x") };
        assert!(publish("o-bad", &stage).unwrap_err().to_string().contains("otto state stages"), "no stages declared");
        assert!(read_index(&crate::paths::run_dir("o-bad").unwrap()).unwrap().outputs.is_empty(), "nothing half-made");
    }

    #[test]
    fn stages_tag_outputs_and_say_where_the_work_is() {
        let _h = TempHome::new();
        test_init("o-stage", "a goal").unwrap();
        set_stages("o-stage", &["Research".into(), "Design".into(), "Plan".into()]).unwrap();
        assert!(set_stages("o-stage", &["a".into(), "A".into()]).is_err(), "named twice");
        publish("o-stage", &Publish { stage: Some("research"), ..doc("research", "What it touches", "Three places.") }).unwrap();
        let wrong = Publish { stage: Some("deploy"), ..doc("x", "t", "x") };
        assert!(publish("o-stage", &wrong).unwrap_err().to_string().contains("Research, Design, Plan"));
        publish("o-stage", &Publish { stage: Some("Design"), ..doc("design", "Design: tiers", "v1") }).unwrap();

        let dir = crate::paths::run_dir("o-stage").unwrap();
        let state = crate::state::read_run("o-stage").unwrap();
        let v = view(&dir, &state);
        let statuses: Vec<(&str, &str)> = v.stages.iter().map(|s| (s.name.as_str(), s.status)).collect();
        assert_eq!(statuses, vec![("Research", "done"), ("Design", "current"), ("Plan", "later")]);
        assert_eq!(v.outputs[0].output.stage.as_deref(), Some("Design"), "stored as declared");
        assert!(set_stages("o-stage", &["Design".into()]).is_err(), "research still has an output");
        assert!(journal("o-stage").contains("\"stages-set\""));
    }

    #[test]
    fn a_question_about_a_version_keeps_its_answer_there() {
        let _h = TempHome::new();
        test_init("o-rev", "a goal").unwrap();
        publish("o-rev", &doc("design", "Design", "v1 text")).unwrap();
        let dir = crate::paths::run_dir("o-rev").unwrap();
        assert_eq!(resolve_about(&dir, "design").unwrap(), "design@1");
        assert!(resolve_about(&dir, "design@3").is_err());
        assert!(resolve_about(&dir, "nope").is_err());
        crate::state::ops::open_gate_about("o-rev", "design-review", "Is the design right?", None, Some("design")).unwrap();
        let state = crate::state::read_run("o-rev").unwrap();
        assert_eq!(state.gate.as_ref().unwrap().about.as_deref(), Some("design@1"));
        assert!(view(&dir, &state).outputs[0].asking);

        crate::state::ops::close_gate("o-rev", Some("Needs changes: no new table"), crate::state::Status::Running).unwrap();
        publish("o-rev", &doc("design", "Design", "v2 text")).unwrap();
        let index = read_index(&dir).unwrap();
        let design = &index.outputs[0];
        assert_eq!(design.versions[0].reviews[0].answer, "Needs changes: no new table", "on v1, where it was asked");
        assert!(design.versions[1].reviews.is_empty());
        assert!(journal("o-rev").contains("\"about\":\"design@1\""), "the question says what it was about");
    }

    #[test]
    fn a_draft_waits_until_someone_deals_with_it() {
        let _h = TempHome::new();
        test_init("o-draft", "a goal").unwrap();
        let p = Publish {
            about: Some("https://slack.example/archives/C1/p1729071120"),
            draft: Some("Hi Priya — the bot read an archived doc."),
            ..doc("thread-1729071120", "Priya asked why refunds say 14 days", "An archived policy is still indexed.")
        };
        publish("o-draft", &p).unwrap();
        let out = read("o-draft", "thread-1729071120", None).unwrap();
        assert_eq!(out.output.state, Some(OutputState::Waiting));
        assert_eq!(out.version.draft.as_deref(), Some("Hi Priya — the bot read an archived doc."));

        let done = set_state("o-draft", "thread-1729071120", OutputState::Done, None, "person").unwrap();
        assert_eq!((done.state, done.state_by.as_deref()), (Some(OutputState::Done), Some("person")));
        assert!(set_state("o-draft", "nope", OutputState::Done, None, "person").is_err());
        assert!(journal("o-draft").contains(r#""event":"output-state","output":"thread-1729071120","state":"done","by":"person""#));
    }

    #[test]
    fn work_kept_elsewhere_is_a_link() {
        let _h = TempHome::new();
        test_init("o-link", "a goal").unwrap();
        let p = Publish { key: "PROJ-4412", title: "Add rate_tier to key_metadata", link: Some("https://jira.example/browse/PROJ-4412"), ..Default::default() };
        let out = publish("o-link", &p).unwrap();
        assert_eq!((out.key.as_str(), out.file), ("proj-4412", None));
        let dir = crate::paths::run_dir("o-link").unwrap();
        let r = reference(&dir, "proj-4412@1").unwrap();
        assert_eq!(r.link.as_deref(), Some("https://jira.example/browse/PROJ-4412"));
        let note = note_text(&dir, "proj-4412", "  wrong component  ").unwrap();
        assert_eq!(note, "About output proj-4412@1, \"Add rate_tier to key_metadata\":\n\nwrong component");
    }
}
