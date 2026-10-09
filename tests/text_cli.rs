//! Public CLI regression coverage for the text grammar; every runtime call uses an isolated
//! database and the library seam (`cli::run`) is always offline, so no IPC leaves the process.
#![cfg(feature = "cli")]

use futures::executor::block_on;
use rustickers::{
    cli::{
        self, Cli,
        commands::{text_argv, text_execution::ExitCode},
    },
    model::sticker::StickerState,
    storage::{StickerStore, paths::AppPaths, sqlite::SqliteStore},
    text::{TextEntryId, TextRepository},
};
use serde_json::Value;
use std::process::Command;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

struct Fixture {
    _dir: tempfile::TempDir,
    paths: AppPaths,
    repo: TextRepository,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            db_path: dir.path().join("stickers.db"),
        };
        let repo = block_on(TextRepository::open(&paths.db_path)).unwrap();
        Self {
            _dir: dir,
            paths,
            repo,
        }
    }
    fn capture(&self, text: &str) -> TextEntryId {
        let id = block_on(self.repo.capture(text, 100))
            .unwrap()
            .capture_id()
            .expect("expected the CLI helper to seed a fresh capture");
        TextEntryId::Clipboard(id)
    }
    fn sticker(&self, text: &str, state: StickerState) -> TextEntryId {
        let capture = block_on(self.repo.get(self.capture(text))).unwrap();
        let id = block_on(self.repo.promote(&capture, false)).unwrap();
        let store = block_on(SqliteStore::open(&self.paths.db_path)).unwrap();
        block_on(store.update_sticker_state(id, state)).unwrap();
        TextEntryId::Sticker(id)
    }
    fn locked_sticker(&self, text: &str, password: &str) -> TextEntryId {
        block_on(self.repo.set_master_password(password)).unwrap();
        let capture = block_on(self.repo.get(self.capture(text))).unwrap();
        TextEntryId::Sticker(block_on(self.repo.promote(&capture, true)).unwrap())
    }
    fn run(&self, args: &[&str]) -> anyhow::Result<()> {
        cli::run(Cli::try_parse_from(args).unwrap(), &self.paths)
    }
    fn execute(
        &self,
        selectors: &[&str],
        id: TextEntryId,
        command: &[String],
    ) -> anyhow::Result<()> {
        let mut args = strings(&["rst", "text", "exec"]);
        args.extend(strings(selectors));
        args.extend(["--id".into(), id.to_string(), "--".into()]);
        args.extend_from_slice(command);
        cli::run(Cli::try_parse_from(args).unwrap(), &self.paths)
    }
}

// Explicit interpreters are test helpers only: the production CLI must not invent a shell.
fn child(code: i32, dir: &std::path::Path) -> Vec<String> {
    #[cfg(windows)]
    {
        let script = dir.join(format!("exit-{code}.ps1"));
        std::fs::write(&script, format!("param([string]$Text)\nexit {code}\n")).unwrap();
        strings(&[
            "powershell.exe",
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-File",
            script.to_str().unwrap(),
            "{{txt}}",
        ])
    }
    #[cfg(unix)]
    {
        let _ = dir;
        strings(&["/bin/sh", "-c", &format!("exit {code}"), "{{txt}}"])
    }
}

#[test]
fn text_list_json_uses_the_spoken_state_names() {
    let f = Fixture::new();
    f.sticker("closed text", StickerState::Close);
    f.capture("clipboard text");
    let output = Command::new(env!("CARGO_BIN_EXE_rst"))
        .arg("--data-dir")
        .arg(f._dir.path())
        .args(["text", "list", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true);
    // Saved stickers say "closed", matching `--state closed`; captures have no state.
    let closed = value["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("sticker:"))
        })
        .expect("the closed saved sticker");
    assert_eq!(closed["state"], "closed");
    let capture = value["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("clipboard:"))
        })
        .expect("the capture");
    assert_eq!(capture["state"], Value::Null);
}

#[test]
fn bad_child_template_does_not_create_storage() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("isolated");
    let output = Command::new(env!("CARGO_BIN_EXE_rst"))
        .arg("--data-dir")
        .arg(&dir)
        .args(["text", "exec", "--id", "sticker:1", "--", "{{txt}}"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("program name"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !dir.exists(),
        "a rejected child template must not touch storage"
    );
}

#[test]
fn non_interactive_without_id_is_a_usage_error_before_storage() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("isolated");
    let output = Command::new(env!("CARGO_BIN_EXE_rst"))
        .arg("--data-dir")
        .arg(&dir)
        .args(["text", "exec", "--non-interactive", "--", "tool"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "requires --id at the parser");
    assert!(!dir.exists(), "a usage error must not touch storage");
}

#[test]
fn canonical_grammar_and_removed_aliases() {
    // `list` is canonical; `ls` and the old text families are gone.
    assert!(Cli::try_parse_from(["rst", "list"]).is_ok());
    for removed in ["ls", "cp", "vault", "markdown", "cmd"] {
        assert!(Cli::try_parse_from(["rst", removed]).is_err(), "{removed}");
    }
    let names = cli::subcommand_names();
    assert!(names.iter().any(|s| s == "list"));
    assert!(!names.iter().any(|s| s == "ls"));

    for tail in [
        vec![],
        vec!["--state", "all"],
        vec!["--state", "open"],
        vec!["--state", "closed"],
        vec!["--type", "markdown"],
        vec!["-s", "rust", "-n", "20"],
        vec!["--json"],
    ] {
        let mut args = vec!["rst", "list"];
        args.extend(tail);
        assert!(Cli::try_parse_from(&args).is_ok(), "{args:?}");
    }
    // The old sticker-listing text flags are not part of `list` anymore.
    for tail in [
        vec!["--text"],
        vec!["--cp"],
        vec!["--state", "close"],
        vec!["-n", "0"],
        vec!["--", "tool"],
    ] {
        let mut args = vec!["rst", "list"];
        args.extend(tail);
        assert!(Cli::try_parse_from(&args).is_err(), "{args:?}");
    }

    for tail in [
        vec![],
        vec!["--source", "saved"],
        vec!["--source", "clipboard"],
        vec!["--source", "all", "--state", "closed"],
        vec!["--source", "saved", "-s", "rust", "-n", "5"],
        vec!["--json"],
    ] {
        let mut args = vec!["rst", "text", "list"];
        args.extend(tail);
        assert!(Cli::try_parse_from(&args).is_ok(), "{args:?}");
    }
}

#[test]
fn exec_parser_constraints() {
    // A child program is required, including when `--` is present but empty.
    for tail in [
        vec![],
        vec!["--"],
        vec!["--id", "sticker:1", "--"],
        vec!["--non-interactive", "--", "tool"],
        vec!["--id", "sticker:1", "-s", "needle", "--", "tool"],
        vec!["--json", "--id", "sticker:1", "--", "tool"],
    ] {
        let mut args = vec!["rst", "text", "exec"];
        args.extend(tail);
        assert!(Cli::try_parse_from(&args).is_err(), "{args:?}");
    }
    assert!(
        Cli::try_parse_from([
            "rst",
            "--json",
            "text",
            "exec",
            "--id",
            "sticker:1",
            "--",
            "tool"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "rst",
            "text",
            "exec",
            "--id",
            "sticker:1",
            "-s",
            "needle",
            "--",
            "tool"
        ])
        .is_err()
    );

    // Malformed cross-table IDs never reach the runtime.
    for id in [
        "42",
        "sticker:0",
        "clipboard:-1",
        "unknown:1",
        "sticker:nope",
    ] {
        assert!(
            Cli::try_parse_from(["rst", "text", "exec", "--id", id, "--", "tool"]).is_err(),
            "{id}"
        );
    }

    // A valid invocation, and a parent JSON flag after `--` belongs to the child.
    let cli = Cli::try_parse_from([
        "rst",
        "text",
        "exec",
        "--id",
        "clipboard:42",
        "--",
        "tool",
        "--json",
        "{{txt}}",
    ])
    .expect("child argv passes through");
    assert!(!cli.format().is_json());
}

#[test]
fn text_list_and_sticker_list_are_separate_domains() {
    let f = Fixture::new();
    f.sticker("open text", StickerState::Open);
    f.capture("clipboard text");
    for args in [
        vec!["rst", "list"],
        vec!["rst", "list", "--state", "closed"],
        vec!["rst", "list", "--type", "markdown", "--json"],
        vec!["rst", "text", "list"],
        vec!["rst", "text", "list", "--source", "saved"],
        vec!["rst", "text", "list", "--source", "clipboard", "--json"],
        vec![
            "rst", "text", "list", "--source", "all", "-s", "text", "-n", "1",
        ],
    ] {
        f.run(&args).unwrap();
    }
}

#[test]
fn runtime_honors_source_and_state_filters_for_direct_ids() {
    let f = Fixture::new();
    let closed = f.sticker("closed text", StickerState::Close);
    let open = f.sticker("open text", StickerState::Open);
    let capture = f.capture("clipboard text");
    let command = child(0, f._dir.path());
    for (selectors, id) in [
        (vec!["--source", "saved"], closed), // --state all is the default
        (vec!["--source", "saved", "--state", "open"], open),
        (vec!["--source", "saved", "--state", "closed"], closed),
        (vec!["--source", "saved", "--state", "all"], closed),
        (vec!["--source", "clipboard", "--state", "open"], capture),
        (vec!["--source", "clipboard", "--state", "closed"], capture),
        (vec!["--source", "all"], capture),
    ] {
        f.execute(&selectors, id, &command).unwrap();
    }
    for (selectors, id, expected) in [
        (
            vec!["--source", "clipboard"],
            closed,
            "outside selected text sources",
        ),
        (
            vec!["--source", "saved"],
            capture,
            "outside selected text sources",
        ),
        (
            vec!["--source", "saved", "--state", "open"],
            closed,
            "not available",
        ),
        (
            vec!["--source", "saved", "--state", "closed"],
            open,
            "not available",
        ),
    ] {
        let error = f.execute(&selectors, id, &command).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}

#[test]
fn empty_snapshot_fails_before_the_terminal_chooser() {
    let f = Fixture::new();
    let error = f
        .run(&["rst", "text", "exec", "--", "some-tool"])
        .unwrap_err();
    assert!(error.to_string().contains("no text entries"), "{error}");
}

#[test]
fn non_interactive_requires_id_and_refuses_encrypted_text_before_any_prompt() {
    let f = Fixture::new();
    let capture = f.capture("ordinary text");
    f.execute(
        &["--source", "clipboard", "--non-interactive"],
        capture,
        &child(0, f._dir.path()),
    )
    .unwrap();

    let locked = f.locked_sticker("private marker text", "master key");
    let error = f
        .execute(
            &["--source", "saved", "--non-interactive"],
            locked,
            &child(0, f._dir.path()),
        )
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("non-interactive"), "{message}");
    assert!(message.contains("encrypted"), "{message}");
    assert!(!message.contains("private marker"), "{message}");
}

#[test]
fn exact_argv_replacement_and_append_do_not_retokenize_or_recurse() {
    let text = "  Unicode 雪\r\n\"quotes\" 'single' ; & | $(no) {{txt}} ";
    let template = strings(&[
        "fixed-program",
        "--json",
        "prefix={{txt}}/{{txt}}",
        "{{txt}}",
        "",
    ]);
    assert_eq!(
        text_argv::build(&template, text).unwrap(),
        vec![
            "fixed-program".to_owned(),
            "--json".to_owned(),
            format!("prefix={text}/{text}"),
            text.to_owned(),
            String::new(),
        ]
    );
    assert_eq!(
        text_argv::build(&strings(&["fixed-program", "--flag", "two words"]), text).unwrap(),
        vec![
            "fixed-program".to_owned(),
            "--flag".to_owned(),
            "two words".to_owned(),
            text.to_owned(),
        ]
    );
    assert_eq!(
        text_argv::build(&strings(&["program"]), "").unwrap(),
        strings(&["program", ""])
    );
}

#[test]
fn nul_and_invalid_programs_fail_before_selection() {
    for template in [
        vec![],
        strings(&[""]),
        strings(&["{{txt}}"]),
        strings(&["prefix{{txt}}"]),
        strings(&["bad\0program"]),
        strings(&["program", "bad\0argument"]),
    ] {
        assert!(
            text_argv::validate_template(&template).is_err(),
            "{template:?}"
        );
        assert!(text_argv::build(&template, "safe").is_err());
    }
    assert!(
        text_argv::build(&strings(&["program", "{{txt}}"]), "secret\0text")
            .unwrap_err()
            .to_string()
            .contains("NUL")
    );
    assert!(text_argv::build(&strings(&["program"]), "secret\0text").is_err());
    let f = Fixture::new();
    f.capture("some text");
    let error = f
        .run(&["rst", "text", "exec", "--", "{{txt}}"])
        .unwrap_err();
    assert!(error.to_string().contains("program name"), "{error}");
}

#[test]
fn child_receives_exact_text_as_one_argument_without_shell_expansion() {
    let f = Fixture::new();
    let exact = "  Unicode 雪\r\n\"quotes\" 'single' ; & | $(no) {{txt}} ";
    let id = f.capture(exact);
    let output = f._dir.path().join("received.txt");
    #[cfg(windows)]
    let command = {
        let script = f._dir.path().join("receive.ps1");
        std::fs::write(&script, "param([string]$OutputPath,[string]$Text)\n[System.IO.File]::WriteAllText($OutputPath,$Text,(New-Object System.Text.UTF8Encoding($false)))\n").unwrap();
        strings(&[
            "powershell.exe",
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-File",
            script.to_str().unwrap(),
            output.to_str().unwrap(),
            "prefix={{txt}}/{{txt}}",
        ])
    };
    #[cfg(unix)]
    let command = strings(&[
        "/bin/sh",
        "-c",
        "printf '%s' \"$2\" > \"$1\"",
        "receive",
        output.to_str().unwrap(),
        "prefix={{txt}}/{{txt}}",
    ]);
    f.execute(&["--source", "clipboard"], id, &command).unwrap();
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        format!("prefix={exact}/{exact}")
    );
    assert!(block_on(f.repo.get(id)).unwrap().activity_at() > 100);
    let mut appended = command;
    *appended.last_mut().unwrap() = "fixed argument".to_owned();
    // With no placeholder the CLI appends exactly one argument after the fixed argument.
    #[cfg(windows)]
    {
        let script = f._dir.path().join("append.ps1");
        std::fs::write(&script, "param([string]$OutputPath,[string]$Fixed,[string]$Text)\nif ($Fixed -ne 'fixed argument') { exit 71 }\n[System.IO.File]::WriteAllText($OutputPath,$Text,(New-Object System.Text.UTF8Encoding($false)))\n").unwrap();
        appended[5] = script.to_str().unwrap().to_owned();
    }
    #[cfg(unix)]
    {
        appended[2] =
            "test \"$2\" = 'fixed argument' || exit 71; printf '%s' \"$3\" > \"$1\"".to_owned();
    }
    f.execute(&["--source", "clipboard"], id, &appended)
        .unwrap();
    assert_eq!(std::fs::read_to_string(output).unwrap(), exact);
}

#[test]
fn unsupported_envelopes_cannot_be_selected_or_spawn_a_child() {
    let f = Fixture::new();
    let pool = block_on(sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&f.paths.db_path),
    ))
    .unwrap();
    let marker = f._dir.path().join("received.txt");
    #[cfg(windows)]
    let command = {
        let script = f._dir.path().join("must-not-spawn.ps1");
        std::fs::write(&script, "param([string]$OutputPath,[string]$Text)\n[System.IO.File]::WriteAllText($OutputPath,'spawned')\n").unwrap();
        strings(&[
            "powershell.exe",
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-File",
            script.to_str().unwrap(),
            marker.to_str().unwrap(),
            "{{txt}}",
        ])
    };
    #[cfg(unix)]
    let command = strings(&[
        "/bin/sh",
        "-c",
        "printf spawned > \"$1\"",
        "marker",
        marker.to_str().unwrap(),
        "{{txt}}",
    ]);
    for content in [
        "RUSTICKERS_LOCKED_V1\nprivate legacy marker",
        "RUSTICKERS_VAULT_V1\nprivate legacy marker",
        "RUSTICKERS_TEXT_V2\nprivate legacy marker",
    ] {
        let id: i64 = block_on(sqlx::query_scalar("INSERT INTO stickers(title,state,left,top,width,height,color,type,content,created_at,updated_at) VALUES('old title','close',0,0,400,300,'yellow','markdown',?,1,1) RETURNING id")
            .bind(content).fetch_one(&pool)).unwrap();
        let error = f
            .execute(&["--source", "saved"], TextEntryId::Sticker(id), &command)
            .unwrap_err();
        assert!(error.to_string().contains("not available"), "{error}");
        assert!(!format!("{error:#}").contains("private legacy marker"));
        assert!(!marker.exists());
        let last_used: Option<i64> = block_on(
            sqlx::query_scalar("SELECT text_last_used_at FROM stickers WHERE id=?")
                .bind(id)
                .fetch_one(&pool),
        )
        .unwrap();
        assert_eq!(last_used, None);
    }
}

#[test]
fn child_success_nonzero_and_missing_executable_propagate_exit_codes() {
    let f = Fixture::new();
    let id = f.capture("exact argument with spaces\nquotes: \" ' ; &");
    f.execute(&["--source", "clipboard"], id, &child(0, f._dir.path()))
        .unwrap();
    let nonzero_id = f.capture("nonzero child still counts as successful delivery");
    let error = f
        .execute(
            &["--source", "clipboard"],
            nonzero_id,
            &child(23, f._dir.path()),
        )
        .unwrap_err();
    assert_eq!(error.downcast_ref::<ExitCode>().unwrap().0, 23);
    assert!(
        block_on(f.repo.get(nonzero_id)).unwrap().activity_at() > 100,
        "successful spawn advances MRU even when the child exits nonzero"
    );
    let before = block_on(f.repo.get(id)).unwrap().activity_at();
    let missing = f._dir.path().join("definitely-missing-executable");
    let error = f
        .execute(
            &["--source", "clipboard"],
            id,
            &[missing.to_str().unwrap().to_owned()],
        )
        .unwrap_err();
    assert_eq!(error.downcast_ref::<ExitCode>().unwrap().0, 127);
    assert_eq!(block_on(f.repo.get(id)).unwrap().activity_at(), before);
}
