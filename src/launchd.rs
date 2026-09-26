//! The two launchd agents otto installs, both generated here so they always point at whichever
//! `otto` binary is actually installed, not wherever the source checkout happens to sit:
//!
//! - **the reviver** (`otto agent start|stop|status`) runs `otto poke` every ~5 minutes. Without
//!   it a sleeping run never wakes.
//! - **the web service** (`otto service start|stop|status`) keeps `otto serve` running, so the
//!   web UI is there after a login or a crash without anyone starting it. Optional: the server
//!   holds no state, and runs carry on without it.
//!
//! `start` is idempotent and self-installing for both: it always rewrites the plist (cheap,
//! and self-healing if the binary moved) and always does a fresh bootout-then-bootstrap, rather
//! than trying to detect "is it already loaded" — that detection is exactly the kind of
//! `launchctl print` output-parsing that has drifted across macOS versions before.

use crate::error::OttoError;
use serde::Serialize;
use std::path::{Path, PathBuf};

const LABEL: &str = "com.joshuahill.otto-poke";
const SERVICE_LABEL: &str = "com.joshuahill.otto-serve";

fn launch_agents_dir() -> PathBuf {
    crate::paths::home_dir().join("Library").join("LaunchAgents")
}

fn plist_path_for(label: &str) -> PathBuf {
    launch_agents_dir().join(format!("{label}.plist"))
}

fn plist_path() -> PathBuf {
    plist_path_for(LABEL)
}

fn path_env() -> String {
    let local_bin = crate::paths::home_dir().join(".local").join("bin");
    format!("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:{}", local_bin.display())
}

/// How launchd should run a job: on a timer, or kept alive for as long as the user is logged in.
enum Schedule {
    /// Every this many seconds, at low priority — the reviver is background housekeeping.
    Every(u32),
    /// Restarted whenever it exits — a server nobody should have to start by hand.
    KeepAlive,
}

struct Job<'a> {
    label: &'a str,
    /// The arguments after the binary.
    args: Vec<String>,
    schedule: Schedule,
}

fn reviver_job() -> Job<'static> {
    Job { label: LABEL, args: vec!["poke".to_string()], schedule: Schedule::Every(300) }
}

fn service_job(port: u16) -> Job<'static> {
    Job {
        label: SERVICE_LABEL,
        args: vec!["serve".to_string(), "--port".to_string(), port.to_string()],
        schedule: Schedule::KeepAlive,
    }
}

/// The plist content `start` writes, with the binary path, working directory, `$OTTO_HOME` and
/// log path resolved fresh from wherever otto is actually installed and where its data actually
/// lives — never a path baked in at some earlier point in time. `OTTO_HOME` is passed through so
/// a job started from a shell with a non-default home serves and revives that home, not `~/.otto`.
fn generate_plist(job: &Job, binary: &Path, otto_home: &Path, log_path: &Path) -> String {
    let args: String = std::iter::once(binary.display().to_string())
        .chain(job.args.iter().cloned())
        .map(|a| format!("\t\t<string>{a}</string>\n"))
        .collect();
    let schedule = match job.schedule {
        Schedule::Every(seconds) => format!(
            "\t<key>StartInterval</key>\n\t<integer>{seconds}</integer>\n\t<key>LowPriorityIO</key>\n\t<true/>\n\t<key>Nice</key>\n\t<integer>5</integer>\n"
        ),
        // ThrottleInterval keeps a server that cannot bind its port from spinning.
        Schedule::KeepAlive => {
            "\t<key>KeepAlive</key>\n\t<true/>\n\t<key>ThrottleInterval</key>\n\t<integer>30</integer>\n".to_string()
        }
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
{args}	</array>
	<key>RunAtLoad</key>
	<true/>
{schedule}	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key>
		<string>{path_env}</string>
		<key>OTTO_HOME</key>
		<string>{otto_home}</string>
	</dict>
	<key>WorkingDirectory</key>
	<string>{otto_home}</string>
	<key>StandardOutPath</key>
	<string>{log_path}</string>
	<key>StandardErrorPath</key>
	<string>{log_path}</string>
</dict>
</plist>
"#,
        label = job.label,
        path_env = path_env(),
        otto_home = otto_home.display(),
        log_path = log_path.display(),
    )
}

fn gui_domain() -> Result<String, OttoError> {
    let output = std::process::Command::new("id")
        .arg("-u")
        .output()
        .map_err(|e| OttoError::usage(format!("cannot determine your uid: {e}")))?;
    if !output.status.success() {
        return Err(OttoError::usage("cannot determine your uid: `id -u` failed"));
    }
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(format!("gui/{uid}"))
}

/// What `start_agent` / `start_service` did, for the caller to report.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    pub target: String,
    pub binary: String,
    pub plist: String,
    pub log: String,
}

/// Write (or rewrite) a job's plist and ensure launchd has it registered and running.
fn install(job: &Job, log_path: PathBuf) -> Result<Started, OttoError> {
    let binary = crate::paths::current_exe()?;
    let otto_home = crate::paths::otto_home();
    std::fs::create_dir_all(launch_agents_dir())?;
    let plist_path = plist_path_for(job.label);
    crate::state::write_atomic(&plist_path, &generate_plist(job, &binary, &otto_home, &log_path))?;

    let domain = gui_domain()?;
    let target = format!("{domain}/{}", job.label);
    // Idempotent by construction: unload first (fine if it wasn't loaded), then load
    // fresh from the plist we just wrote.
    let _ = std::process::Command::new("launchctl").args(["bootout", &target]).output();
    let bootstrap = std::process::Command::new("launchctl")
        .args(["bootstrap", &domain, &plist_path.display().to_string()])
        .output()
        .map_err(|e| OttoError::usage(format!("cannot run launchctl: {e}")))?;
    if !bootstrap.status.success() {
        return Err(OttoError::usage(format!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&bootstrap.stderr).trim()
        )));
    }
    let _ = std::process::Command::new("launchctl").args(["enable", &target]).output();
    Ok(Started {
        target,
        binary: binary.display().to_string(),
        plist: plist_path.display().to_string(),
        log: log_path.display().to_string(),
    })
}

/// Write (or rewrite) the reviver's plist and ensure it is registered and running.
/// Installs it from scratch if it isn't there yet.
pub fn start_agent() -> Result<Started, OttoError> {
    install(&reviver_job(), crate::paths::poke_log_path()?)
}

pub fn start() -> Result<(), OttoError> {
    let started = start_agent()?;
    println!("otto: the reviver is running ({})", started.target);
    println!("  binary: {}", started.binary);
    println!("  plist:  {}", started.plist);
    println!("  log:    {}", started.log);
    Ok(())
}

fn is_loaded_label(label: &str) -> Option<bool> {
    let domain = gui_domain().ok()?;
    let target = format!("{domain}/{label}");
    let output = std::process::Command::new("launchctl").args(["print", &target]).output().ok()?;
    Some(output.status.success())
}

/// Is the reviver currently loaded? `None` means we could not tell (`launchctl`/`id` itself
/// failed) — callers must treat that as "don't know", never as "not loaded", or a broken
/// `launchctl` would print a false warning every time.
///
/// A plain exit-code check, deliberately not the `launchctl print` output-parsing this module's
/// own doc comment warns off elsewhere: that warning is about *deciding* whether to bootstrap
/// (where `start` sidesteps the question entirely by always doing a fresh bootout-then-bootstrap),
/// not about *answering* a read-only "is it loaded" question, which an exit code settles fine.
pub fn is_loaded() -> Option<bool> {
    is_loaded_label(LABEL)
}

/// The timestamp leading the last non-empty line of `poke.log` — every line poke prints starts
/// with one (`poke::poke_run`'s `stamp`). `None` with no log yet, or a log poke has never
/// written a normal line to.
fn last_poke() -> Option<crate::clock::Timestamp> {
    let path = crate::paths::poke_log_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let last = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let stamp = last.split_whitespace().next()?;
    crate::clock::Timestamp::parse(stamp).ok()
}

/// Whether the reviver is loaded and when it last actually ran.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    /// `None`: could not tell (launchctl or id failed) — never the same as "not loaded".
    pub loaded: Option<bool>,
    pub last_poke: Option<crate::clock::Timestamp>,
    pub last_poke_relative: Option<String>,
    pub plist: String,
    pub log: String,
}

pub fn agent_status() -> Result<AgentStatus, OttoError> {
    let last = last_poke();
    Ok(AgentStatus {
        loaded: is_loaded(),
        last_poke: last,
        last_poke_relative: last.map(crate::clock::relative),
        plist: plist_path().display().to_string(),
        log: crate::paths::poke_log_path()?.display().to_string(),
    })
}

/// `otto agent status` — is the reviver loaded, and when did it last actually run? The README
/// says a sleeping run never wakes without it; this is how to find out it stopped being true
/// before a run has been silently stuck for days.
pub fn status() -> Result<(), OttoError> {
    let status = agent_status()?;
    match status.loaded {
        Some(true) => println!("otto: the reviver is loaded"),
        Some(false) => {
            println!("otto: the reviver is NOT loaded — sleeping runs will never wake on their own");
            println!("  `otto agent start` to fix");
        }
        None => println!("otto: could not tell whether the reviver is loaded (launchctl or id failed)"),
    }
    match (status.last_poke, &status.last_poke_relative) {
        (Some(at), Some(relative)) => println!("  last poke: {relative} ({at})"),
        _ => println!("  last poke: never (no log yet)"),
    }
    println!("  plist:     {}", status.plist);
    println!("  log:       {}", status.log);
    Ok(())
}

/// What `stop_agent` found.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    pub target: String,
    pub was_running: bool,
}

/// Unregister a launchd job. Safe to call whether or not it was running. The plist is left
/// where it is: launchd only reads it on `bootstrap`, and `start` rewrites it anyway.
fn unload(label: &str) -> Result<Stopped, OttoError> {
    let domain = gui_domain()?;
    let target = format!("{domain}/{label}");
    let output = std::process::Command::new("launchctl")
        .args(["bootout", &target])
        .output()
        .map_err(|e| OttoError::usage(format!("cannot run launchctl: {e}")))?;
    Ok(Stopped { target, was_running: output.status.success() })
}

/// Unregister the reviver. Safe to call whether or not it was running.
pub fn stop_agent() -> Result<Stopped, OttoError> {
    unload(LABEL)
}

pub fn stop() -> Result<(), OttoError> {
    let stopped = stop_agent()?;
    if stopped.was_running {
        println!("otto: stopped the reviver ({})", stopped.target);
    } else {
        println!("otto: the reviver was not running ({})", stopped.target);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The web service: `otto serve` under launchd.
// ---------------------------------------------------------------------------

/// `otto service start` — install (if missing) and start `otto serve` as a launchd agent that
/// starts at login and restarts if it exits.
pub fn service_start(port: u16) -> Result<(), OttoError> {
    let started = install(&service_job(port), crate::paths::serve_log_path()?)?;
    println!("otto: the web UI is running as a service ({})", started.target);
    println!("  url:    http://127.0.0.1:{port}/");
    println!("  binary: {}", started.binary);
    println!("  plist:  {}", started.plist);
    println!("  log:    {}", started.log);
    Ok(())
}

pub fn service_stop() -> Result<(), OttoError> {
    let stopped = unload(SERVICE_LABEL)?;
    if stopped.was_running {
        println!("otto: stopped the web service ({})", stopped.target);
    } else {
        println!("otto: the web service was not running ({})", stopped.target);
    }
    Ok(())
}

/// The port the installed service was told to listen on, read back from its own plist —
/// `--port` is the only argument that follows a `<string>--port</string>`.
fn service_port(plist: &str) -> Option<u16> {
    let after = plist.split("<string>--port</string>").nth(1)?;
    let value = after.split("<string>").nth(1)?.split("</string>").next()?;
    value.trim().parse().ok()
}

pub fn service_status() -> Result<(), OttoError> {
    let plist_path = plist_path_for(SERVICE_LABEL);
    let plist = std::fs::read_to_string(&plist_path).ok();
    match is_loaded_label(SERVICE_LABEL) {
        Some(true) => println!("otto: the web service is loaded"),
        Some(false) if plist.is_none() => println!("otto: the web service is not installed — `otto service start` to install it"),
        Some(false) => println!("otto: the web service is NOT loaded — `otto service start` to fix"),
        None => println!("otto: could not tell whether the web service is loaded (launchctl or id failed)"),
    }
    if let Some(port) = plist.as_deref().and_then(service_port) {
        println!("  url:   http://127.0.0.1:{port}/");
    }
    println!("  plist: {}", plist_path.display());
    println!("  log:   {}", crate::paths::serve_log_path()?.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;

    #[test]
    fn last_poke_reads_the_trailing_timestamp_from_the_log() {
        let _h = TempHome::new();
        let path = crate::paths::poke_log_path().unwrap();
        std::fs::write(
            &path,
            "2026-09-12T14:00:00Z nothing due (1 run(s) checked)\n2026-09-12T14:05:00Z spawn r1 stranded\n",
        )
        .unwrap();
        assert_eq!(last_poke().unwrap().to_string(), "2026-09-12T14:05:00Z");
    }

    #[test]
    fn last_poke_is_none_with_no_log_yet() {
        let _h = TempHome::new();
        assert!(last_poke().is_none());
    }

    #[test]
    fn last_poke_ignores_a_trailing_blank_line() {
        let _h = TempHome::new();
        let path = crate::paths::poke_log_path().unwrap();
        std::fs::write(&path, "2026-09-12T14:00:00Z nothing due (1 run(s) checked)\n\n").unwrap();
        assert_eq!(last_poke().unwrap().to_string(), "2026-09-12T14:00:00Z");
    }

    #[test]
    fn generated_plist_carries_the_real_binary_and_log_paths() {
        let plist = generate_plist(&reviver_job(), Path::new("/opt/otto/bin/otto"), Path::new("/home/x/.otto"), Path::new("/home/x/.otto/logs/poke.log"));
        assert!(plist.contains("<string>com.joshuahill.otto-poke</string>"));
        assert!(plist.contains("<string>/opt/otto/bin/otto</string>"));
        assert!(plist.contains("<string>poke</string>"));
        assert!(plist.contains("<string>/home/x/.otto</string>"));
        assert!(plist.contains("<string>/home/x/.otto/logs/poke.log</string>"));
        assert!(plist.contains("<integer>300</integer>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(plist.contains("<integer>5</integer>"));
        assert!(plist.contains("<key>OTTO_HOME</key>\n\t\t<string>/home/x/.otto</string>"));
        assert!(!plist.contains("KeepAlive"), "the reviver runs on a timer, not continuously");
    }

    #[test]
    fn the_service_plist_keeps_serve_alive_on_its_port() {
        let plist = generate_plist(&service_job(7979), Path::new("/opt/otto/bin/otto"), Path::new("/home/x/.otto"), Path::new("/home/x/.otto/logs/serve.log"));
        assert!(plist.contains("<string>com.joshuahill.otto-serve</string>"));
        assert!(plist.contains("\t\t<string>/opt/otto/bin/otto</string>\n\t\t<string>serve</string>\n\t\t<string>--port</string>\n\t\t<string>7979</string>\n"));
        assert!(plist.contains("<key>KeepAlive</key>\n\t<true/>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(!plist.contains("StartInterval"));
        assert!(plist.contains("<string>/home/x/.otto/logs/serve.log</string>"));
        assert_eq!(service_port(&plist), Some(7979));
    }

    #[test]
    fn a_plist_without_a_port_has_none() {
        let plist = generate_plist(&reviver_job(), Path::new("/b/otto"), Path::new("/h"), Path::new("/h/l"));
        assert_eq!(service_port(&plist), None);
    }
}
