//! Binary-level recipe contracts. Parse JSON rather than depending on field order/escaping.
#![cfg(feature = "cli")]
use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Stdio},
};

fn reply(dir: &Path, args: &[&str], success: bool) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_rst"))
        .arg("--data-dir")
        .arg(dir)
        .arg("--json")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)));
    assert_eq!(output.status.success(), success, "{args:?}: {value}");
    assert_eq!(value["ok"], success, "{value}");
    assert!(output.stderr.is_empty(), "JSON reports use stdout");
    value
}

fn catalog(dir: &Path) -> Vec<Value> {
    reply(dir, &["skill", "list"], true)["skills"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn catalog_and_guide_are_database_free() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("absent");
    let skills = catalog(&dir);
    assert_eq!(skills.len(), 9);
    assert!(skills.iter().any(|skill| skill["name"] == "note"));
    let guide = reply(&dir, &["skill", "guide"], true);
    assert!(
        guide["guide"]
            .as_str()
            .unwrap()
            .contains("--non-interactive")
    );
    assert!(!dir.exists());
}

#[test]
fn every_recipe_preview_validates_without_writing() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("absent");
    for skill in catalog(&dir) {
        let name = skill["name"].as_str().unwrap();
        if matches!(name, "web-search" | "format-json") && which::which("powershell").is_err() {
            // Platform-specific prerequisites are not installed by a test suite.
            let error = reply(&dir, &["skill", "run", name, "--dry-run"], false);
            assert!(error["error"].as_str().unwrap().contains("not on PATH"));
            continue;
        }
        let mut args = vec!["skill", "run", name, "--dry-run"];
        if matches!(name, "ask" | "translate" | "explain-code") {
            args.extend(["--set", "tool=cargo"]);
        }
        if matches!(name, "html-report" | "svg-chart") {
            args.extend(["--set", "script=echo example"]);
        }
        let preview = reply(&dir, &args, true);
        assert_eq!(preview["dry_run"], true);
        assert!(preview["argv"].is_array());
    }
    assert!(!dir.exists(), "dry runs must not initialize storage");
}

#[test]
fn ai_recipes_have_one_prompt_argument() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["ask", "translate", "explain-code"] {
        let value = reply(
            dir.path(),
            &["skill", "run", name, "--set", "tool=cargo", "--dry-run"],
            true,
        );
        let argv = value["argv"].as_array().unwrap();
        let separator = argv.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(argv.len() - separator - 1, 3);
        assert_eq!(argv[separator + 1], "cargo");
        assert_eq!(argv[separator + 2], "-p");
        assert!(
            argv[separator + 3]
                .as_str()
                .unwrap()
                .contains("{{RUSTICKERS_SELECTION}}")
        );
    }
}

#[test]
fn values_are_literal_and_overrides_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let value = reply(
        dir.path(),
        &[
            "skill",
            "run",
            "translate",
            "--set",
            "tool=cargo",
            "--set",
            "language=${tool}",
            "--dry-run",
        ],
        true,
    );
    assert!(value["argv"].as_array().unwrap().iter().any(|arg| {
        arg.as_str()
            .is_some_and(|arg| arg.contains("into ${tool}."))
    }));
    for (tail, expected) in [
        (vec!["--set", "langauge=Japanese"], "is not a variable"),
        (
            vec!["--set", "language=French", "--set", "language=Latin"],
            "more than once",
        ),
        (vec!["--set", "=Japanese"], "empty variable name"),
        (vec!["--set", "language"], "KEY=VALUE"),
    ] {
        let mut args = vec!["skill", "run", "translate", "--dry-run"];
        args.extend(tail);
        let value = reply(dir.path(), &args, false);
        assert!(value["error"].as_str().unwrap().contains(expected));
    }
}

#[test]
fn missing_variables_are_shown_as_incomplete_not_fake_commands() {
    let dir = tempfile::tempdir().unwrap();
    let failed = reply(
        dir.path(),
        &["skill", "run", "html-report", "--dry-run"],
        false,
    );
    assert!(failed["error"].as_str().unwrap().contains("--set script="));
    let shown = reply(dir.path(), &["skill", "show", "html-report"], true);
    assert_eq!(shown["complete"], false);
    assert_eq!(shown["missing"], serde_json::json!(["script"]));
    let available = reply(dir.path(), &["skill", "show", "ask"], true);
    assert_eq!(
        available["complete"], true,
        "discovery never requires copilot"
    );
    assert!(!dir.path().join("stickers.db").exists());
}

#[test]
fn bad_schedule_reports_the_seconds_field_fix() {
    let dir = tempfile::tempdir().unwrap();
    let value = reply(
        dir.path(),
        &[
            "skill",
            "run",
            "watch",
            "--set",
            "cron=*/5 * * * *",
            "--dry-run",
        ],
        false,
    );
    assert!(value["error"].as_str().unwrap().contains("0 */5 * * * *"));
}

#[test]
fn note_recipe_creates_one_stable_sticker() {
    let dir = tempfile::tempdir().unwrap();
    let created = reply(dir.path(), &["skill", "run", "note"], true);
    let list = reply(dir.path(), &["list"], true);
    assert_eq!(list["count"], 1);
    assert_eq!(list["items"][0]["id"], created["id"]);
    assert_eq!(list, reply(dir.path(), &["list"], true));
}

#[test]
fn search_recipe_keeps_engine_data_out_of_static_code() {
    let dir = tempfile::tempdir().unwrap();
    // Show is prerequisite-free; examine the authoritative argv, not its shell display.
    let shown = reply(dir.path(), &["skill", "show", "web-search"], true);
    let argv = shown["argv"].as_array().unwrap();
    let code = argv.last().unwrap().as_str().unwrap();
    assert!(code.starts_with("$env:SEARCH_ENGINE +"));
    assert!(!code.contains("duckduckgo.com"));
    assert!(
        argv.iter()
            .any(|arg| arg == "SEARCH_ENGINE=https://duckduckgo.com/?q=")
    );
}
