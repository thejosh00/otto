//! tests/feedback_driver.rs — naming the run that may assign work, through the real binary: who may
//! name it is decided by the environment a command runs in, and how an item reads by what it prints.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn otto(home: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_otto"));
    command.args(args).env("OTTO_HOME", home).env_remove("OTTO_RUN_ID").env_remove("OTTO_RUN_DIR").env_remove("OTTO_CHAT");
    command.envs(env.iter().copied());
    command.output().unwrap_or_else(|e| panic!("could not run otto {args:?}: {e}"))
}

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn err(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn run(home: &Path, name: &str) -> String {
    let made = otto(home, &[], &["state", "init", "--goal", "a goal", "--name", name]);
    assert!(made.status.success(), "{}", err(&made));
    let list: Value = serde_json::from_str(&out(&otto(home, &[], &["state", "list", "--json"]))).unwrap();
    list.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap()["id"].as_str().unwrap().to_string()
}

fn item(dir: &Path) -> String {
    let path = dir.join("item.md");
    std::fs::write(&path, "Build item 3 of the plan").unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn a_wake_cannot_name_a_driver_or_see_or_clear_one() {
    let home = tempfile::tempdir().unwrap();
    let id = run(home.path(), "driver");
    for marked in ["OTTO_RUN_ID", "OTTO_CHAT"] {
        let env = [(marked, id.as_str())];
        for args in [&["feedback", "driver", "driver"][..], &["feedback", "driver"], &["feedback", "driver", "--clear"]] {
            let refused = otto(home.path(), &env, args);
            assert_eq!(refused.status.code(), Some(1), "{marked} {args:?}");
            assert!(err(&refused).contains("a person's to say"), "{}", err(&refused));
        }
    }
    assert!(!home.path().join("feedback-driver").exists());
    let assign = otto(home.path(), &[("OTTO_RUN_ID", &id)], &["state", "assign", &id, "--file", &item(home.path())]);
    assert_eq!(assign.status.code(), Some(2), "it named nothing, so it can't assign");
    assert!(err(&assign).contains("otto feedback driver"), "{}", err(&assign));
}

#[test]
fn a_person_names_it_and_the_config_never_hears_of_it() {
    let home = tempfile::tempdir().unwrap();
    let id = run(home.path(), "driver");
    std::fs::write(home.path().join("config.json"), "{}\n").unwrap();
    assert!(out(&otto(home.path(), &[], &["feedback", "driver"])).contains("no run may assign"));
    assert!(out(&otto(home.path(), &[], &["feedback", "driver", "driver"])).contains("driver may now assign"));
    assert_eq!(std::fs::read_to_string(home.path().join("feedback-driver")).unwrap().trim(), id);
    assert_eq!(std::fs::read_to_string(home.path().join("config.json")).unwrap(), "{}\n");
    assert!(out(&otto(home.path(), &[], &["feedback", "driver"])).contains("driver may assign"));
    assert_eq!(otto(home.path(), &[], &["feedback", "driver", "driver", "--clear"]).status.code(), Some(2), "one or the other");
    assert!(out(&otto(home.path(), &[], &["feedback", "driver", "--clear"])).contains("no run may assign"));
    assert!(out(&otto(home.path(), &[], &["feedback", "driver"])).contains("no run may assign"));
}

#[test]
fn an_item_from_the_driver_is_told_apart_from_a_person_s_and_a_wake_s() {
    let home = tempfile::tempdir().unwrap();
    let driver = run(home.path(), "driver");
    let other = run(home.path(), "other");
    assert!(otto(home.path(), &[], &["feedback", "driver", "driver"]).status.success());
    let file = item(home.path());

    let refused = otto(home.path(), &[("OTTO_RUN_ID", &other)], &["state", "assign", "other", "--file", &file]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(err(&refused).contains("only") && err(&refused).contains("driver may assign"), "told who: {}", err(&refused));
    let posing = otto(home.path(), &[("OTTO_RUN_ID", &other)], &["state", "assign", "driver", "--file", &file]);
    assert_eq!(posing.status.code(), Some(2), "{}", err(&posing));
    assert_eq!(out(&otto(home.path(), &[], &["feedback", "next"])).trim(), "nothing to build now");

    let filed = otto(home.path(), &[("OTTO_RUN_ID", &driver)], &["state", "assign", "driver", "--file", &file]);
    assert!(filed.status.success(), "{}", err(&filed));
    let next = out(&otto(home.path(), &[], &["feedback", "next"]));
    let first = next.lines().next().unwrap();
    assert!(first.contains(&format!("from #{driver} driver")), "{next}");
    assert!(!first.contains("you"), "not a person's: {next}");
    assert!(!first.contains("goal-only") && !first.contains("wake"), "and not a wake's report: {next}");
    assert!(next.contains("Build item 3 of the plan"), "{next}");

    let json: Value = serde_json::from_str(&out(&otto(home.path(), &[], &["feedback", "next", "--json"]))).unwrap();
    assert_eq!((json["source"].as_str(), json["assignedBy"].as_str()), (Some("wake"), Some(driver.as_str())));

    assert!(otto(home.path(), &[], &["feedback", "add", "mine"]).status.success());
    assert!(otto(home.path(), &[("OTTO_RUN_ID", &other)], &["state", "feedback", "other", "--body", "friction"]).status.success());
    let list = out(&otto(home.path(), &[], &["feedback"]));
    let who: Vec<&str> = list.lines().filter(|l| l.contains(':') && l.contains(" · ")).collect();
    assert_eq!(who.len(), 3, "{list}");
    assert!(who[0].contains(&format!("#{other} other")) && !who[0].contains("from"), "{list}");
    assert!(who[1].contains("you"), "{list}");
    assert!(who[2].contains("from #"), "{list}");
}
