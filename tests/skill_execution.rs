//! Real-execution coverage for the bundled skills.
//!
//! Every sticker here is created by the real `rst` binary in an isolated `--data-dir` store,
//! read back from the database, resolved and spawned through the *production desktop runner*
//! (`RunSpec` + the shared command-line parser) — the same code the sticker window uses, minus
//! GPUI. Hostile selections are fed through as the runtime feeds them: the
//! `RUSTICKERS_SELECTION` environment variable and placeholder substitution, never as string
//! interpolation.
#![cfg(feature = "cli")]

// The borrowed runner module speaks in `crate::model::...` / `crate::utils::...` paths; these
// root re-exports make those names resolve inside this test crate.
pub use rustickers::{model, utils};

use futures::executor::block_on;
use rustickers::model::content::CommandContent;
use rustickers::storage::{StickerStore, sqlite::SqliteStore};
use std::path::Path;
use std::process::Command;

// The desktop runner module, borrowed whole so the test exercises the real spawn path. It has
// no GPUI dependencies. `pump`, `kill` and friends are used by the desktop app rather than by
// this file, hence the dead-code allowance scoped to exactly this module.
#[path = "../src/native/components/stickers/command/runner.rs"]
#[allow(dead_code)]
mod runner;

fn available(program: &str) -> bool {
    which::which(program).is_ok()
}

/// Run `rst --data-dir <dir> <args>`, require `ok:true`, and return the parsed reply.
fn rst(dir: &Path, args: &[&str]) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rst"));
    command.arg("--data-dir").arg(dir).arg("--json").args(args);
    let output = command.output().expect("the rst binary runs");
    let body = String::from_utf8_lossy(&output.stdout).into_owned();
    let value: serde_json::Value =
        serde_json::from_str(body.trim()).unwrap_or_else(|e| panic!("{args:?} -> {body}\n{e}"));
    assert!(
        output.status.success() && value["ok"] == true,
        "{args:?} -> {body}"
    );
    value
}

/// Create one skill's sticker and return its stored content string verbatim.
fn create_sticker(dir: &Path, skill: &str, sets: &[&str]) -> String {
    let mut args = vec!["skill", "run", skill];
    for set in sets {
        args.push("--set");
        args.push(set);
    }
    let created = rst(dir, &args);
    let id = created["id"]
        .as_i64()
        .unwrap_or_else(|| panic!("no id in {created}"));

    let db = dir.join("stickers.db");
    let store = block_on(SqliteStore::open(&db)).expect("open isolated store");
    block_on(store.get_sticker(id))
        .expect("sticker exists")
        .content
}

/// ... and as the command runtime sees it.
fn create(dir: &Path, skill: &str, sets: &[&str]) -> CommandContent {
    let content = create_sticker(dir, skill, sets);
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("command content for {skill}: {content}\n{e}"))
}

/// Resolve through the production runner and capture the child's whole output.
fn run_like_the_desktop(
    content: &CommandContent,
    selection: Option<&str>,
) -> std::io::Result<std::process::Output> {
    let spec = runner::RunSpec::resolve(content, selection).map_err(std::io::Error::other)?;
    spec.spawn()
        .map_err(std::io::Error::other)?
        .wait_with_output()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn report_external_default_availability() {
    // Not an assertion: the platform report the sweep is supposed to produce.
    for program in ["copilot", "pwsh", "powershell", "cargo", "git"] {
        eprintln!(
            "skill-default availability: {program}: {}",
            available(program)
        );
    }
}

#[test]
fn format_json_recipes_format_hostile_selection_as_pure_data() {
    if !available("powershell") {
        eprintln!("skip: no powershell on this platform");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let content = create(dir.path(), "format-json", &[]);

    // Valid JSON whose *values* are PowerShell injection attempts and expansions.
    let selection = r#"{"payload":"Invoke-Expression calc","stamp":"$(Get-Date)"}"#;
    let output = run_like_the_desktop(&content, Some(selection)).expect("format-json runs");
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout.contains("\"payload\""),
        "not re-emitted as JSON: {stdout}"
    );
    assert!(stdout.contains("Invoke-Expression"), "value lost: {stdout}");
    // If PowerShell had evaluated the expansion, a date would appear instead of this literal.
    assert!(
        stdout.contains("$(Get-Date)"),
        "expansion was evaluated, not kept: {stdout}"
    );
    assert!(
        stdout.contains('\n'),
        "output should be indented, not one line: {stdout}"
    );
}

#[test]
fn format_json_fails_as_data_on_hostile_invalid_json() {
    if !available("powershell") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let content = create(dir.path(), "format-json", &[]);

    // Invalid JSON wrapped around a quote-breaking injection attempt. The recipe sets
    // `$ErrorActionPreference = 'Stop'`, so malformed data must fail the run outright.
    let selection = r#"nope"; Start-Process calc; ""#;
    let output = run_like_the_desktop(&content, Some(selection)).expect("child spawned");
    let combined = text(&output.stdout) + &text(&output.stderr);
    assert!(
        !output.status.success(),
        "invalid JSON must fail the run: {combined}"
    );
    assert!(
        combined.to_lowercase().contains("json"),
        "selection must meet the JSON parser, not the code path: {combined}"
    );
    assert!(
        !combined.contains("Start-Process"),
        "script text was echoed back as code: {combined}"
    );
}

#[test]
fn web_search_recipes_encode_hostile_selection_into_exactly_one_url() {
    if !available("powershell") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let content = create(dir.path(), "web-search", &[]);

    // Quotes, an ampersand and a space: if any survived raw into the output, the sticker would
    // open several parameters or a mangled URL; if the code evaluated them, worse.
    let selection = r#"a="b&c" d"#;
    let output = run_like_the_desktop(&content, Some(selection)).expect("web-search runs");
    let stdout = text(&output.stdout).trim().to_owned();
    assert!(output.status.success(), "stdout: {stdout}");
    assert_eq!(
        stdout, "https://duckduckgo.com/?q=a%3D%22b%26c%22%20d",
        "engine prefix (single-quote regression) or encoding gap"
    );
}

#[test]
fn ask_recipes_hand_the_selection_as_one_prompt_argument() {
    let tool = if available("cargo") {
        "cargo"
    } else {
        eprintln!("skip: no cargo to stand in for the missing copilot default");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let content = create(dir.path(), "ask", &[&format!("tool={tool}")]);

    // Resolve exactly as the runner does; the spawn is skipped, the argv is the contract.
    let (program, args) = runner::split_command(&content.command, Some("pick me")).unwrap();
    let name = Path::new(&program)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert_eq!(name, tool, "program: {program}");
    assert_eq!(
        args,
        ["-p", "pick me"],
        "prompt must be one argument holding the selection"
    );
}

#[test]
fn watch_recipe_runs_a_multiline_script_through_the_platform_shell() {
    let has_shell = available("pwsh") || available("powershell") || cfg!(unix);
    if !has_shell {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = if cfg!(windows) {
        "Write-Output ONE\nWrite-Output TWO"
    } else {
        "echo ONE\necho TWO"
    };
    let content = create(dir.path(), "watch", &[&format!("script={script}")]);
    let output = run_like_the_desktop(&content, None).expect("watch script runs");
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout.contains("ONE") && stdout.contains("TWO"),
        "multiline script: {stdout}"
    );
}

#[test]
fn note_recipe_keeps_dash_prefixed_text_as_content() {
    let dir = tempfile::tempdir().unwrap();
    // Note content is stored verbatim; the dash-leading text must survive as data.
    let stored = create_sticker(dir.path(), "note", &["text=--json rm everything"]);
    assert_eq!(stored, "--json rm everything");
}
