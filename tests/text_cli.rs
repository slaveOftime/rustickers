//! Public CLI regression coverage; every runtime call uses an isolated database.
#![cfg(feature = "cli")]

use clap::Parser;
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
    fn run(&self, args: &[&str]) -> anyhow::Result<()> {
        cli::run(Cli::try_parse_from(args).unwrap(), &self.paths)
    }
    fn execute(
        &self,
        selectors: &[&str],
        id: TextEntryId,
        command: &[String],
    ) -> anyhow::Result<()> {
        let mut args = strings(&["rst", "ls"]);
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
fn merged_grammar_aliases_and_removed_families() {
    for name in ["ls", "list"] {
        for tail in [
            vec![],
            vec!["--state", "all"],
            vec!["--text"],
            vec!["--cp"],
            vec!["--text", "--cp", "-s", "rust", "-n", "20"],
            vec!["--text", "--cp", "--json"],
            vec!["--text", "--cp", "--", "some_cmd", "xxx", "{{txt}}", "xxx"],
            vec!["--text", "--id", "sticker:42", "--", "my-tool", "{{txt}}"],
            vec!["--cp", "--id", "clipboard:42", "--", "my-tool"],
        ] {
            let mut args = vec!["rst", name];
            args.extend(tail);
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?}");
        }
    }
    let names = cli::subcommand_names();
    assert!(names.iter().any(|s| s == "ls"));
    assert!(names.iter().any(|s| s == "list"));
    for removed in ["cp", "vault"] {
        assert!(!names.iter().any(|s| s == removed));
        assert!(Cli::try_parse_from(["rst", removed]).is_err());
    }
}

#[test]
fn id_limit_and_search_conflicts_are_parser_errors() {
    for tail in [
        vec!["--id", "sticker:1"],
        vec!["--id", "sticker:1", "-s", "needle", "--", "tool"],
        vec!["--id", "sticker:1", "-n", "1", "--", "tool"],
        vec!["-n", "1", "--", "tool"],
    ] {
        let mut args = vec!["rst", "ls", "--text"];
        args.extend(tail);
        assert!(Cli::try_parse_from(&args).is_err(), "{args:?}");
    }
    for id in [
        "42",
        "sticker:0",
        "clipboard:-1",
        "unknown:1",
        "sticker:nope",
    ] {
        assert!(
            Cli::try_parse_from(["rst", "ls", "--text", "--id", id, "--", "tool"]).is_err(),
            "{id}"
        );
    }
    assert!(Cli::try_parse_from(["rst", "ls", "--text", "-s", "needle", "--", "tool"]).is_ok());
}

#[test]
fn empty_child_separator_is_ordinary_listing() {
    // An optional delimiter without a child preserves standard Clap listing semantics.
    assert!(Cli::try_parse_from(["rst", "ls", "--text", "--"]).is_ok());
}

#[test]
fn parent_json_is_rejected_but_child_json_is_not_global() {
    let f = Fixture::new();
    for args in [
        vec![
            "rst",
            "--json",
            "ls",
            "--text",
            "--id",
            "sticker:1",
            "--",
            "tool",
        ],
        vec![
            "rst",
            "ls",
            "--text",
            "--json",
            "--id",
            "sticker:1",
            "--",
            "tool",
        ],
    ] {
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(cli.format().is_json());
        assert!(
            cli::run(cli, &f.paths)
                .unwrap_err()
                .to_string()
                .contains("--json")
        );
    }
    let cli = Cli::try_parse_from([
        "rst",
        "ls",
        "--cp",
        "--id",
        "clipboard:1",
        "--",
        "tool",
        "--json",
        "{{txt}}",
    ])
    .unwrap();
    assert!(!cli.format().is_json());
    // A missing ID proves execution got past the parent JSON guard, without launching a child.
    assert!(
        cli::run(cli, &f.paths)
            .unwrap_err()
            .to_string()
            .contains("not available")
    );
}

#[test]
fn runtime_rejects_execution_without_sources_and_nontext_types() {
    let f = Fixture::new();
    assert!(
        f.run(&["rst", "ls", "--", "tool"])
            .unwrap_err()
            .to_string()
            .contains("requires --text")
    );
    for kind in ["command", "file", "timer", "paint"] {
        let error = f
            .run(&["rst", "ls", "--text", "--type", kind, "--", "tool"])
            .unwrap_err();
        assert!(
            error.to_string().contains("only supports Markdown"),
            "{error}"
        );
    }
    // Noninteractive listings and the legacy alias always operate on this fixture, never AppData.
    for args in [
        vec!["rst", "ls"],
        vec!["rst", "list", "--state", "all"],
        vec![
            "rst", "ls", "--text", "--cp", "-s", "rust", "-n", "1", "--json",
        ],
        vec!["rst", "ls", "--text", "--type", "markdown"],
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
        (vec!["--text"], closed), // omitted state includes closed stickers
        (vec!["--text", "--state", "open"], open),
        (vec!["--text", "--state", "close"], closed),
        (vec!["--text", "--state", "all"], closed),
        (vec!["--cp", "--state", "open"], capture),
        (vec!["--cp", "--state", "close"], capture),
        (vec!["--text", "--cp"], capture),
    ] {
        f.execute(&selectors, id, &command).unwrap();
    }
    for (selectors, id, expected) in [
        (vec!["--cp"], closed, "outside selected text sources"),
        (vec!["--text"], capture, "outside selected text sources"),
        (vec!["--text", "--state", "open"], closed, "not available"),
        (vec!["--text", "--state", "close"], open, "not available"),
    ] {
        let error = f.execute(&selectors, id, &command).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
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
    let error = f
        .run(&["rst", "ls", "--text", "--", "{{txt}}"])
        .unwrap_err();
    assert!(error.to_string().contains("program name"));
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
    f.execute(&["--cp"], id, &command).unwrap();
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
    f.execute(&["--cp"], id, &appended).unwrap();
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
            .execute(&["--text"], TextEntryId::Sticker(id), &command)
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
    f.execute(&["--cp"], id, &child(0, f._dir.path())).unwrap();
    let nonzero_id = f.capture("nonzero child still counts as successful delivery");
    let error = f
        .execute(&["--cp"], nonzero_id, &child(23, f._dir.path()))
        .unwrap_err();
    assert_eq!(error.downcast_ref::<ExitCode>().unwrap().0, 23);
    assert!(
        block_on(f.repo.get(nonzero_id)).unwrap().activity_at() > 100,
        "successful spawn advances MRU even when the child exits nonzero"
    );
    let before = block_on(f.repo.get(id)).unwrap().activity_at();
    let missing = f._dir.path().join("definitely-missing-executable");
    let error = f
        .execute(&["--cp"], id, &[missing.to_str().unwrap().to_owned()])
        .unwrap_err();
    assert_eq!(error.downcast_ref::<ExitCode>().unwrap().0, 127);
    assert_eq!(block_on(f.repo.get(id)).unwrap().activity_at(), before);
}
