//! Authorization for outward-facing actions
//!
//! Pushing and merging are the two things otto does that cannot be undone by deleting a
//! file. A gate answered "yes" is necessary but not sufficient: over a multi-day run the
//! branch keeps moving, so "yes" has to mean "yes, THIS code" or it means very little. An
//! authorization therefore binds to a commit, and goes stale the moment the branch does.

use super::{transaction, OutwardAction};
use crate::clock::now_iso;
use crate::error::OttoError;
use crate::event::Event;
use serde_json::{json, Value};
use std::path::Path;

fn action_str(action: OutwardAction) -> &'static str {
    match action {
        OutwardAction::Push => "push",
        OutwardAction::Merge => "merge",
        OutwardAction::Comment => "comment",
        OutwardAction::DeleteBranch => "delete-branch",
    }
}

/// Whether the gate with this id has been answered, and detail for the caller.
pub fn gate_is_answered(run_path: &Path, gate_id: &str) -> Result<(bool, String), OttoError> {
    let gates_dir = run_path.join("gates");
    let mut matches: Vec<_> = if gates_dir.is_dir() {
        std::fs::read_dir(&gates_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{gate_id}-")))
            .collect()
    } else {
        Vec::new()
    };
    matches.sort_by_key(|e| e.file_name());
    let Some(entry) = matches.first() else {
        return Ok((false, format!("no gate file for {gate_id}")));
    };
    let body = std::fs::read_to_string(entry.path())?;
    if body.contains("_unanswered_") {
        return Ok((false, format!("gate {gate_id} is still unanswered")));
    }
    Ok((true, entry.file_name().to_string_lossy().into_owned()))
}

/// Where an authorization is filed. `item` lets a run that fans out over several
/// repos hold one authorization per repo — keyed by action alone, approving the second
/// repo's head would silently void the first.
fn authorization_key(action: OutwardAction, item: Option<&str>) -> String {
    match item {
        Some(item) => format!("{}:{}", action_str(action), crate::clock::slugify(item, "item")),
        None => action_str(action).to_string(),
    }
}

#[derive(clap::Args, Debug)]
pub struct AuthorizeArgs {
    /// The run: its id, a prefix of it, or its slug
    pub id: String,
    /// The outward action being authorized
    #[arg(long, value_enum, required = true)]
    pub action: OutwardAction,
    /// the commit this authorizes, full or short sha
    #[arg(long, required = true)]
    pub head: String,
    /// the answered gate that authorizes it
    #[arg(long)]
    pub gate: Option<String>,
    /// the policy flag that authorizes it, e.g. autoMergeWhenGreen
    #[arg(long)]
    pub policy: Option<String>,
    /// which target, for a run that fans out over several
    #[arg(long)]
    pub item: Option<String>,
    /// the human's words, verbatim
    #[arg(long)]
    pub quote: Option<String>,
}

pub fn authorize(args: AuthorizeArgs) -> Result<(), OttoError> {
    if args.gate.is_some() == args.policy.is_some() {
        return Err(OttoError::usage(
            "give exactly one of --gate (a human said so) or --policy (you said so in advance)",
        ));
    }
    let id = args.id.clone();
    let mut record = Value::Null;
    transaction(&id, |path, state| {
        let (source, note) = if let Some(gate) = &args.gate {
            let (answered, detail) = gate_is_answered(path, gate)?;
            if !answered {
                return Err(OttoError::conflict(format!("cannot authorize from gate {gate}: {detail}")));
            }
            ("human", format!("gate {gate} ({detail})"))
        } else {
            let policy_key = args.policy.as_deref().expect("exactly one of gate/policy is set");
            if !state.policy.is_true(policy_key) {
                return Err(OttoError::conflict(format!(
                    "policy.{policy_key} is not true — it does not authorize this",
                )));
            }
            ("policy", format!("policy.{policy_key}"))
        };
        let mut entry = json!({
            "action": action_str(args.action),
            "head": args.head,
            "by": source,
            "source": note,
            "at": now_iso(),
        });
        if let Some(item) = &args.item {
            entry["item"] = json!(item);
        }
        if let Some(quote) = &args.quote {
            entry["quote"] = json!(quote.trim());
        }
        let key = authorization_key(args.action, args.item.as_deref());
        state.authorizations.insert(key, entry.clone());
        record = entry;
        crate::event::record(
            path,
            &Event::Authorized {
                action: action_str(args.action).to_string(),
                item: args.item.clone(),
                head: args.head.clone(),
                by: source.to_string(),
                source: note,
            },
        )
    })?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct CheckAuthorizedArgs {
    /// The run: its id, a prefix of it, or its slug
    pub id: String,
    /// The outward action about to be taken
    #[arg(long, value_enum, required = true)]
    pub action: OutwardAction,
    /// the commit you are about to act on
    #[arg(long, required = true)]
    pub head: String,
    /// which target, for a run that fans out over several
    #[arg(long)]
    pub item: Option<String>,
}

/// Gate an outward action on disk, not on recollection. Exit 2 means DO NOT ACT.
pub fn check_authorized(args: CheckAuthorizedArgs) -> Result<(), OttoError> {
    let state = super::read_run(&args.id)?;
    let key = authorization_key(args.action, args.item.as_deref());
    let what = match &args.item {
        Some(item) => format!("{} for {item}", action_str(args.action)),
        None => action_str(args.action).to_string(),
    };
    let record = state.authorizations.get(&key).ok_or_else(|| {
        OttoError::conflict(format!(
            "{what} is not authorized for this run — open a gate, get an answer, then \
             `authorize --action {}{} --gate <NNN> --head <sha>`",
            action_str(args.action),
            args.item.as_deref().map(|i| format!(" --item {i}")).unwrap_or_default(),
        ))
    })?;
    let recorded_head = record.get("head").and_then(Value::as_str).unwrap_or("");
    if recorded_head != args.head {
        return Err(OttoError::conflict(format!(
            "{what} was authorized for {recorded_head}, but the branch is now {}. \
             The approval was for code that has since changed — re-ask.",
            args.head
        )));
    }
    println!("{}", serde_json::to_string_pretty(record)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::{close_gate, open_gate, test_init, CloseGateArgs, OpenGateArgs};
    use crate::state::Status;

    fn open_and_answer(id: &str, slug: &str, answer: &str) {
        open_gate(OpenGateArgs {
            id: id.to_string(),
            slug: slug.to_string(),
            question: Some("well?".to_string()),
            question_file: None,
            stdin: false,
            expires_at: None,
            expires_in: None,
        })
        .unwrap();
        close_gate(CloseGateArgs {
            id: id.to_string(),
            answer: Some(answer.to_string()),
            answer_file: None,
            stdin: false,
            expired: false,
            status: Status::Running,
        })
        .unwrap();
    }

    #[test]
    fn answered_gate_authorizes_push() {
        let _home = TempHome::new();
        test_init("auth-a", "dev-flow").unwrap();
        open_and_answer("auth-a", "push-review", "ship it");
        authorize(AuthorizeArgs {
            id: "auth-a".to_string(),
            action: OutwardAction::Push,
            head: "abc123".to_string(),
            gate: Some("001".to_string()),
            policy: None,
            item: None,
            quote: None,
        })
        .unwrap();
        check_authorized(CheckAuthorizedArgs {
            id: "auth-a".to_string(),
            action: OutwardAction::Push,
            head: "abc123".to_string(),
            item: None,
        })
        .unwrap();
    }

    #[test]
    fn unanswered_gate_does_not_authorize() {
        let _home = TempHome::new();
        test_init("auth-b", "dev-flow").unwrap();
        open_gate(OpenGateArgs {
            id: "auth-b".to_string(),
            slug: "push-review".to_string(),
            question: Some("well?".to_string()),
            question_file: None,
            stdin: false,
            expires_at: None,
            expires_in: None,
        })
        .unwrap();
        let result = authorize(AuthorizeArgs {
            id: "auth-b".to_string(),
            action: OutwardAction::Push,
            head: "abc123".to_string(),
            gate: Some("001".to_string()),
            policy: None,
            item: None,
            quote: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn policy_must_be_literal_true() {
        let _home = TempHome::new();
        test_init("auth-c", "dev-flow").unwrap();
        // default policy.autoMergeWhenGreen is false
        let result = authorize(AuthorizeArgs {
            id: "auth-c".to_string(),
            action: OutwardAction::Merge,
            head: "abc123".to_string(),
            gate: None,
            policy: Some("autoMergeWhenGreen".to_string()),
            item: None,
            quote: None,
        });
        assert!(result.is_err());

        transaction("auth-c", |_p, state| {
            state.policy.auto_merge_when_green = true;
            Ok(())
        })
        .unwrap();
        authorize(AuthorizeArgs {
            id: "auth-c".to_string(),
            action: OutwardAction::Merge,
            head: "abc123".to_string(),
            gate: None,
            policy: Some("autoMergeWhenGreen".to_string()),
            item: None,
            quote: None,
        })
        .unwrap();
    }

    #[test]
    fn check_authorized_fails_on_head_mismatch_or_missing() {
        let _home = TempHome::new();
        test_init("auth-d", "dev-flow").unwrap();
        let missing = check_authorized(CheckAuthorizedArgs {
            id: "auth-d".to_string(),
            action: OutwardAction::Push,
            head: "abc123".to_string(),
            item: None,
        });
        assert!(missing.is_err());

        open_and_answer("auth-d", "push-review", "ship it");
        authorize(AuthorizeArgs {
            id: "auth-d".to_string(),
            action: OutwardAction::Push,
            head: "abc123".to_string(),
            gate: Some("001".to_string()),
            policy: None,
            item: None,
            quote: None,
        })
        .unwrap();
        let stale = check_authorized(CheckAuthorizedArgs {
            id: "auth-d".to_string(),
            action: OutwardAction::Push,
            head: "def456".to_string(),
            item: None,
        });
        assert!(stale.is_err());
    }

    #[test]
    fn item_keyed_authorizations_do_not_collide() {
        let _home = TempHome::new();
        test_init("auth-e", "upgrade-flow").unwrap();
        open_and_answer("auth-e", "batch-review", "go");
        for repo in ["repoA", "repoB"] {
            authorize(AuthorizeArgs {
                id: "auth-e".to_string(),
                action: OutwardAction::Push,
                head: format!("{repo}-sha"),
                gate: Some("001".to_string()),
                policy: None,
                item: Some(repo.to_string()),
                quote: None,
            })
            .unwrap();
        }
        for repo in ["repoA", "repoB"] {
            check_authorized(CheckAuthorizedArgs {
                id: "auth-e".to_string(),
                action: OutwardAction::Push,
                head: format!("{repo}-sha"),
                item: Some(repo.to_string()),
            })
            .unwrap();
        }
        // The no-item key is untouched by either item-keyed authorization.
        let no_item = check_authorized(CheckAuthorizedArgs {
            id: "auth-e".to_string(),
            action: OutwardAction::Push,
            head: "repoA-sha".to_string(),
            item: None,
        });
        assert!(no_item.is_err());
    }
}
