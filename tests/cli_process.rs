//! The shipped binary contract: isolated storage, streams, JSON, and saved command argv.
#![cfg(feature = "cli")]
use serde_json::{Value, json};
use std::{
    io::Write as _,
    process::{Command, Output, Stdio},
};

fn rst(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rst"))
        .arg("--data-dir")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn response(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
#[test]
fn help_and_skills_do_not_initialize_storage() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("not-created");
    for args in [
        vec!["--help"],
        vec!["skill", "list", "--json"],
        vec!["skill", "guide"],
        vec!["skill", "show", "note", "--json"],
    ] {
        let output = rst(&dir, &args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!dir.exists(), "discovery must not create storage");
    }
}
#[test]
fn usage_errors_are_json_and_do_not_create_storage() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("not-created");
    for args in [
        vec!["--json", "unknown"],
        vec!["show", "0", "--json"],
        vec!["note", "--width", "0", "--json"],
        vec!["list", "--state", "close", "--json"],
        vec!["text", "exec", "--json", "--", "tool"],
    ] {
        let output = rst(&dir, &args);
        assert_eq!(output.status.code(), Some(2), "args={args:?}");
        assert_eq!(response(&output)["ok"], false);
        assert!(output.stderr.is_empty(), "JSON usage errors use stdout");
        assert!(!dir.exists());
    }
}
#[test]
fn note_lifecycle_has_consistent_state_and_offline_notification() {
    let dir = tempfile::tempdir().unwrap();
    let output = rst(
        dir.path(),
        &[
            "note",
            "# 雪\n\nExact text",
            "--closed",
            "--top-most",
            "--json",
        ],
    );
    assert!(output.status.success());
    let created = response(&output);
    let id = created["id"].as_i64().unwrap().to_string();
    assert_eq!(created["state"], "closed");
    assert_eq!(created["desktop_notification"], "not_requested");
    let listed = response(&rst(dir.path(), &["list", "--json"]));
    assert_eq!(
        listed["count"], 1,
        "list includes closed stickers by default"
    );
    assert_eq!(listed["items"][0]["id"], created["id"]);
    let shown = response(&rst(dir.path(), &["show", &id, "--json"]));
    assert_eq!(shown["content"], "# 雪\n\nExact text");
    assert_eq!(shown["top_most"], true);
    for (action, state) in [("open", "open"), ("close", "closed")] {
        let output = rst(dir.path(), &[action, &id, "--json"]);
        assert!(output.status.success());
        let value = response(&output);
        assert_eq!(value["state"], state);
        assert_eq!(value["desktop_notification"], "offline");
    }
    let refused = rst(dir.path(), &["delete", &id, "--json"]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(response(&refused)["ok"], false);
    let deleted = rst(dir.path(), &["delete", &id, "--yes", "--json"]);
    assert!(deleted.status.success());
    assert_eq!(response(&deleted)["deleted"], true);
    assert_eq!(response(&rst(dir.path(), &["list", "--json"]))["count"], 0);
}
#[test]
fn stdin_note_and_raw_content_preserve_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let exact = "  snow 雪\r\n\"quotes\"\n\n";
    let mut child = Command::new(env!("CARGO_BIN_EXE_rst"))
        .arg("--data-dir")
        .arg(dir.path())
        .args(["note", "--file", "-", "--closed", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(exact.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let id = response(&output)["id"].as_i64().unwrap().to_string();
    let raw = rst(dir.path(), &["show", &id, "--content-only"]);
    assert_eq!(raw.stdout, exact.as_bytes());
}
#[test]
fn command_creation_is_not_claimed_as_execution() {
    let dir = tempfile::tempdir().unwrap();
    let created = rst(
        dir.path(),
        &["command", "--closed", "--json", "--", "cargo", "--version"],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stdout)
    );
    let id = response(&created)["id"].as_i64().unwrap().to_string();
    let output = response(&rst(dir.path(), &["result", &id, "--json"]));
    assert_eq!(output["ok"], true);
    assert_eq!(output["has_output"], false);
    assert_eq!(output["output"], Value::Null);
    assert_eq!(output["armed"], true);
    assert_eq!(output.get("has_run"), None);
}
#[test]
fn text_exec_child_flags_do_not_become_parent_flags() {
    let dir = tempfile::tempdir().unwrap();
    let output = rst(
        dir.path(),
        &[
            "text",
            "exec",
            "--id",
            "sticker:999",
            "--non-interactive",
            "--",
            "tool",
            "--json",
            "{{txt}}",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not available"));
}
#[test]
fn unattended_chooser_fails_instead_of_reading_the_parent_console() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        rst(dir.path(), &["note", "text", "--closed", "--json"])
            .status
            .success()
    );
    let output = rst(dir.path(), &["text", "exec", "--", "tool"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--non-interactive"));
}

#[test]
fn note_skill_preview_and_execution_agree_for_literal_variables() {
    let dir = tempfile::tempdir().unwrap();
    let literal = "text=literal ${tool} 雪 \"quotes\"";
    let preview = rst(
        dir.path(),
        &[
            "skill",
            "run",
            "note",
            "--set",
            literal,
            "--dry-run",
            "--json",
        ],
    );
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stdout)
    );
    let created = rst(
        dir.path(),
        &["skill", "run", "note", "--set", literal, "--json"],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stdout)
    );
    let id = response(&created)["id"].as_i64().unwrap().to_string();
    assert_eq!(
        response(&rst(dir.path(), &["show", &id, "--json"]))["content"],
        json!("literal ${tool} 雪 \"quotes\"")
    );
}
