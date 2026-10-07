//! Clipboard history: safe argv execution, terminal selection, and script-friendly output.
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::io::{self, Read, Write};
use std::sync::Arc;
use zeroize::Zeroizing;

use super::clipboard_terminal::{choose, safe_preview};
use super::vault::unlock;
use crate::cli::{
    output::{Format, table},
    runtime::block_on,
};
use crate::clipboard::{
    argv, filter,
    items::{ClipboardItem, ClipboardItemKind},
    storage::{ClipboardHistoryStore, RetentionFlag, SqliteClipboardStore, UpsertOutcome},
    vault_controller::VaultController,
};
use crate::storage::paths::AppPaths;

/// Exit codes needing propagation without skipping Rust destructors.
#[derive(Debug)]
pub struct ExitCode(pub i32);
impl std::fmt::Display for ExitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "command exited with status {}", self.0)
    }
}
impl std::error::Error for ExitCode {}

#[derive(clap::Subcommand, Debug)]
pub enum CpCommand {
    /// Browse, search, or reuse previous copies.
    #[command(visible_alias = "list")]
    Ls(CpArgs),
    /// Add text to history (or pipe text with --stdin).
    Add(AddArgs),
    /// Remove reservation/edit retention; protected items remain protected.
    Rm(RmArgs),
    /// Print live clipboard text, or an exact history value with --id.
    Show(ShowTextArgs),
}

#[derive(clap::Args, Debug)]
#[command(
    about = "Browse and reuse previous copies",
    after_help = "Examples:\n  rst cp ls                              browse (↑↓, Enter, Esc)\n  rst cp ls -s rust                      start with a search\n  rst cp ls --list -n 20                  print recent history\n  rst cp ls --json --list                 machine-readable history\n  rst cp ls -- rg -F -- '{{cp}}' .         select and run\n  rst cp ls --id 42 -- my-tool            append one exact argument\n\nQuote '{{cp}}' in POSIX shells and PowerShell; use \"{{cp}}\" in cmd.exe.\nClipboard text stays data; a shell/interpreter code argument can still execute it.\nChooser/password input uses the terminal, leaving child stdin and stdout untouched.\nExit codes: 130 cancelled, 127 program missing, 126 not executable, otherwise child status."
)]
pub struct CpArgs {
    /// Print a compact table (or one JSON object with --json), without choosing.
    #[arg(long, conflicts_with_all = ["id", "command"])]
    pub list: bool,
    /// Filter history; in interactive mode, prefill the search field.
    #[arg(long, short = 's', value_name = "TEXT", conflicts_with = "id")]
    pub search: Option<String>,
    /// Maximum rows to list.
    #[arg(long, short = 'n', value_name = "COUNT", requires = "list")]
    pub limit: Option<usize>,
    /// Use a specific history row without a chooser.
    #[arg(long, value_name = "ID")]
    pub id: Option<i64>,
    /// Child program and arguments. Use -- before the program.
    #[arg(last = true, num_args = 1.., value_name = "COMMAND")]
    pub command: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct AddArgs {
    /// Read exact text from stdin.
    #[arg(long, conflicts_with = "value")]
    pub stdin: bool,
    /// Text to record, unchanged. Values starting with `-` are kept as text.
    #[arg(value_name = "VALUE", required_unless_present = "stdin", allow_hyphen_values = true)]
    pub value: Option<String>,
    /// Encrypt before writing; prompts for the master password on the terminal.
    #[arg(long)]
    pub protect: bool,
}
#[derive(clap::Args, Debug)]
pub struct RmArgs {
    #[arg(value_name = "ID")]
    pub id: i64,
}
#[derive(clap::Args, Debug)]
pub struct ShowTextArgs {
    /// Print this history value instead of the live clipboard.
    #[arg(long, value_name = "ID")]
    pub id: Option<i64>,
}

pub(super) fn open_store(paths: &AppPaths) -> Result<Arc<dyn ClipboardHistoryStore>> {
    Ok(Arc::new(block_on(SqliteClipboardStore::open(
        paths.db_path.with_file_name("clipboard.db"),
    ))?))
}

fn row_text(
    store: Arc<dyn ClipboardHistoryStore>,
    row: &ClipboardItem,
) -> Result<Zeroizing<String>> {
    if row.kind == ClipboardItemKind::Plain {
        return Ok(Zeroizing::new(row.content.clone()));
    }
    let controller = VaultController::new(store);
    unlock(&controller)?;
    let bytes = controller
        .open(&row.content)?
        .context("vault locked before decrypting")?;
    Ok(Zeroizing::new(
        std::str::from_utf8(&bytes)
            .context("protected item is not UTF-8 text")?
            .to_owned(),
    ))
}

pub fn run(paths: &AppPaths, args: CpArgs, format: Format) -> Result<()> {
    if !args.command.is_empty() {
        argv::validate_template(&args.command).context("invalid command template")?;
    }
    if format.is_json() && !args.list && args.id.is_none() {
        bail!("--json requires --list or --id <ID>; interactive selection is for humans");
    }
    let store = open_store(paths)?;
    let snapshot = block_on(store.snapshot())?;
    if args.list {
        return run_list(&snapshot, args.search.as_deref(), args.limit, format);
    }
    let id = if let Some(id) = args.id {
        id
    } else {
        if snapshot.is_empty() {
            bail!(
                "no clipboard history yet; enable collection in the desktop app or use `rst cp add <TEXT>`"
            );
        }
        choose(&snapshot, args.search.as_deref().unwrap_or(""))?.ok_or(ExitCode(130))?
    };
    // Re-read after selection; a concurrent delete must not execute stale clipboard content.
    let row = block_on(store.get(id))
        .with_context(|| format!("clipboard row {id} is no longer available"))?;
    if args.command.is_empty() {
        format.emit(json!({"id": id, "kind": kind(&row), "hint": "pass -- <PROGRAM> <ARGS> to reuse this item"}), || {
            println!("Selected #{id}. To reuse it: rst cp ls --id {id} -- <PROGRAM> '{{{{cp}}}}'");
        });
        return Ok(());
    }
    let text = row_text(store.clone(), &row)?;
    let current =
        block_on(store.get(id)).context("clipboard item disappeared during authorization")?;
    if current.kind != row.kind || current.content != row.content {
        bail!("clipboard item changed during authorization; select it again");
    }
    let final_args = Zeroizing::new(argv::build(&args.command, &text)?);
    let mut child = std::process::Command::new(&final_args[0]);
    child
        .args(&final_args[1..])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    let mut child = match child.spawn() {
        Ok(child) => child,
        Err(err) => {
            let code = match err.kind() {
                io::ErrorKind::NotFound => 127,
                io::ErrorKind::PermissionDenied => 126,
                _ => 1,
            };
            eprintln!(
                "error: could not launch `{}`: {err}",
                safe_preview(&final_args[0], 100)
            );
            return Err(ExitCode(code).into());
        }
    };
    if let Err(err) = block_on(store.mark_used(id, crate::utils::time::now_unix_millis())) {
        eprintln!("warning: could not update clipboard recency: {err}");
    }
    let status = child.wait().context("wait for child command")?;
    #[cfg(unix)]
    let code = {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
    };
    #[cfg(not(unix))]
    let code = status.code().unwrap_or(1);
    if code == 0 {
        Ok(())
    } else {
        Err(ExitCode(code).into())
    }
}

fn kind(row: &ClipboardItem) -> &'static str {
    if row.kind == ClipboardItemKind::Protected {
        "protected"
    } else {
        "plain"
    }
}

fn run_list(
    snapshot: &[ClipboardItem],
    search: Option<&str>,
    limit: Option<usize>,
    format: Format,
) -> Result<()> {
    let indices = filter::filter_indices(snapshot, search.unwrap_or(""));
    let rows: Vec<_> = indices
        .iter()
        .take(limit.unwrap_or(usize::MAX))
        .map(|index| &snapshot[*index])
        .collect();
    let payload: Vec<_> = rows.iter().map(|row| json!({
        "id": row.id, "kind": kind(row), "byte_size": row.byte_size, "preview": safe_preview(&row.preview_text(), 120),
        "public_label": row.public_label, "retention": row.retention, "created_at": row.created_at,
        "last_copied_at": row.last_copied_at, "last_used_at": row.last_used_at, "last_edited_at": row.last_edited_at,
    })).collect();
    format.emit(json!({"items": payload, "count": rows.len(), "search": search}), || {
        if rows.is_empty() {
            println!("{}", if search.is_some() { "No matching clipboard items." } else { "No clipboard history yet. Enable collection in the desktop app, or use cp add <TEXT>." });
            return;
        }
        let cells: Vec<_> = rows.iter().map(|row| vec![row.id.to_string(), safe_preview(&row.preview_text(), 72), row.byte_size.to_string(), {
            let mut flags = Vec::new();
            if row.retention.protected_ { flags.push("🔒 Protected"); }
            if row.retention.explicit { flags.push("Reserved"); }
            if row.retention.edited { flags.push("Edited"); }
            flags.join(", ")
        }]).collect();
        table(&["ID", "PREVIEW", "BYTES", "SAVED"], &cells);
    });
    Ok(())
}

pub fn add(paths: &AppPaths, args: AddArgs, format: Format) -> Result<()> {
    let value = Zeroizing::new(if args.stdin {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .context("read stdin")?;
        text
    } else {
        args.value.context("provide VALUE or --stdin")?
    });
    if !crate::clipboard::items::is_collectable(&value) {
        bail!("cannot add empty or whitespace-only text");
    }
    if value.contains('\0') {
        bail!("clipboard text cannot contain a NUL byte");
    }
    let store = open_store(paths)?;
    let bytes = value.len() as i64;
    let now = crate::utils::time::now_unix_millis();
    let outcome = if args.protect {
        let controller = VaultController::new(store.clone());
        unlock(&controller)?;
        let sealed = controller
            .seal_for_storage(value.as_bytes())?
            .context("vault locked before encryption")?;
        block_on(store.upsert_protected_bound(
            None,
            None,
            None,
            &sealed.ciphertext,
            bytes,
            now,
            &sealed.vault_envelope,
        ))?
    } else {
        let settings = block_on(store.settings())?;
        if settings.normal_byte_limit > 0 && bytes > settings.normal_byte_limit {
            bail!(
                "text exceeds ordinary history byte limit ({} > {}); nothing added",
                bytes,
                settings.normal_byte_limit
            );
        }
        block_on(store.upsert_ordinary(
            &value,
            &safe_preview(&value, 240),
            &SqliteClipboardStore::ordinary_digest(&value),
            bytes,
            now,
        ))?
    };
    let (id, outcome) = match outcome {
        UpsertOutcome::Inserted { id } => (id, "inserted"),
        UpsertOutcome::Updated { id } => (id, "updated"),
    };
    format.emit(
        json!({"id": id, "outcome": outcome, "protected": args.protect, "byte_size": bytes}),
        || {
            println!(
                "{} #{id} · {bytes} bytes{}",
                if outcome == "inserted" {
                    "Added"
                } else {
                    "Updated"
                },
                if args.protect { " · 🔒 Protected" } else { "" }
            )
        },
    );
    Ok(())
}

pub fn rm(paths: &AppPaths, args: RmArgs, format: Format) -> Result<()> {
    let store = open_store(paths)?;
    let row = block_on(store.get(args.id))?;
    if row.kind == ClipboardItemKind::Protected {
        bail!("item is protected; unprotect it in the desktop app before removing retention");
    }
    block_on(store.set_retention_flag(args.id, RetentionFlag::Edited, false))?;
    block_on(store.set_retention_flag(args.id, RetentionFlag::Explicit, false))?;
    format.emit(json!({"id": args.id}), || {
        println!("Item #{} is eligible for ordinary eviction.", args.id)
    });
    Ok(())
}

pub fn show(paths: &AppPaths, args: ShowTextArgs, format: Format) -> Result<()> {
    let text = if let Some(id) = args.id {
        let store = open_store(paths)?;
        let row = block_on(store.get(id))?;
        row_text(store, &row)?
    } else {
        Zeroizing::new(
            arboard::Clipboard::new()
                .context("open clipboard")?
                .get_text()
                .context("clipboard does not contain text")?,
        )
    };
    if format.is_json() {
        format.emit(json!({"text": *text}), || {});
    } else {
        io::stdout().lock().write_all(text.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    pub(crate) struct TestCli {
        #[command(subcommand)]
        pub(crate) command: CpCommand,
    }

    #[test]
    fn parser_preserves_child_flags_and_placeholder() {
        let parsed = TestCli::try_parse_from([
            "cp", "ls", "--id", "42", "--", "rg", "--json", "-F", "--", "{{cp}}", ".",
        ])
        .unwrap();
        let CpCommand::Ls(args) = parsed.command else {
            panic!()
        };
        assert_eq!(args.command, ["rg", "--json", "-F", "--", "{{cp}}", "."]);
        assert_eq!(args.id, Some(42));
    }

    #[test]
    fn parser_accepts_search_without_list_and_rejects_discarded_commands() {
        assert!(TestCli::try_parse_from(["cp", "ls", "-s", "rust"]).is_ok());
        assert!(TestCli::try_parse_from(["cp", "ls", "--list", "--", "echo"]).is_err());
        assert!(TestCli::try_parse_from(["cp", "add"]).is_err());
        assert!(TestCli::try_parse_from(["cp", "add", "--stdin"]).is_ok());
    }

    #[test]
    fn parser_keeps_flag_like_text_as_value() {
        // Clipboard content very often starts with a dash; it must arrive as text, not as options.
        for text in ["--json", "-n 5", "--protect-but-not"] {
            let parsed = TestCli::try_parse_from(["cp", "add", text]).unwrap();
            let CpCommand::Add(args) = parsed.command else {
                panic!()
            };
            assert_eq!(args.value.as_deref(), Some(text));
            assert!(!args.protect);
        }
    }

    #[test]
    fn parser_still_accepts_protect_and_stdin_flags() {
        let parsed = TestCli::try_parse_from(["cp", "add", "--protect", "secret"]).unwrap();
        let CpCommand::Add(args) = parsed.command else {
            panic!()
        };
        assert!(args.protect);
        assert_eq!(args.value.as_deref(), Some("secret"));
        assert!(matches!(
            TestCli::try_parse_from(["cp", "rm", "42"]).unwrap().command,
            CpCommand::Rm(RmArgs { id: 42 })
        ));
    }
}

#[cfg(test)]
mod regression_tests {
    use super::tests::TestCli;
    use super::*;
    use clap::Parser;

    fn paths(tmp: &tempfile::TempDir) -> AppPaths {
        AppPaths {
            db_path: tmp.path().join("stickers.db"),
        }
    }

    fn add_text(paths: &AppPaths, text: &str) {
        add(
            paths,
            AddArgs {
                stdin: false,
                value: Some(text.into()),
                protect: false,
            },
            Format::Json,
        )
        .unwrap();
    }

    #[test]
    fn failed_protected_add_never_stores_plaintext() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        let error = add(
            &paths,
            AddArgs {
                stdin: false,
                value: Some("must remain secret".into()),
                protect: true,
            },
            Format::Json,
        )
        .unwrap_err();
        assert!(error.to_string().contains("no master password"));
        assert!(
            block_on(open_store(&paths).unwrap().snapshot())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn add_preserves_exact_payload_and_rejects_nul() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        let text = "  Mixed CASE\n日本語\t\"$() & ;\"  ";
        add_text(&paths, text);
        let rows = block_on(open_store(&paths).unwrap().snapshot()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, text);
        assert!(
            add(
                &paths,
                AddArgs {
                    stdin: false,
                    value: Some("bad\0text".into()),
                    protect: false
                },
                Format::Json
            )
            .is_err()
        );
        assert_eq!(
            block_on(open_store(&paths).unwrap().snapshot())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn add_rejects_empty_and_whitespace_only_text() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        for blank in ["", "   ", "\n\t "] {
            assert!(
                add(
                    &paths,
                    AddArgs {
                        stdin: false,
                        value: Some(blank.into()),
                        protect: false
                    },
                    Format::Json
                )
                .is_err(),
                "expected rejection of {blank:?}"
            );
        }
        assert!(
            block_on(open_store(&paths).unwrap().snapshot())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn add_hyphen_prefixed_text_end_to_end() {
        // The parser-level guarantee, verified through the real store: flag-like text lands verbatim.
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        let parsed = TestCli::try_parse_from(["cp", "add", "--grep --flags -x"]).unwrap();
        let CpCommand::Add(args) = parsed.command else {
            panic!()
        };
        add(&paths, args, Format::Json).unwrap();
        let rows = block_on(open_store(&paths).unwrap().snapshot()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "--grep --flags -x");
    }

    #[test]
    fn add_moves_existing_duplicate_to_top() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        add_text(&paths, "first");
        std::thread::sleep(std::time::Duration::from_millis(5));
        add_text(&paths, "second");
        std::thread::sleep(std::time::Duration::from_millis(5));
        add_text(&paths, "first"); // same text again: must refresh recency, not duplicate
        let rows = block_on(open_store(&paths).unwrap().snapshot()).unwrap();
        assert_eq!(rows.len(), 2, "duplicate text must upsert, not insert");
        assert_eq!(rows[0].content, "first");
        assert!(rows[0].last_copied_at >= rows[1].last_copied_at);
    }

    #[test]
    fn add_reports_ids_and_json_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        add(
            &paths,
            AddArgs {
                stdin: false,
                value: Some("payload".into()),
                protect: false,
            },
            Format::Json,
        )
        .unwrap();
        let row = block_on(open_store(&paths).unwrap().snapshot())
            .unwrap()
            .remove(0);
        assert_eq!(row.content, "payload");
        assert_eq!(row.byte_size, 7);
    }

    #[test]
    fn missing_program_preserves_127_and_does_not_promote() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        add_text(&paths, "payload");
        let store = open_store(&paths).unwrap();
        let row = block_on(store.snapshot()).unwrap().remove(0);
        let error = run(
            &paths,
            CpArgs {
                list: false,
                search: None,
                limit: None,
                id: Some(row.id),
                command: vec![
                    tmp.path()
                        .join("missing-program")
                        .to_string_lossy()
                        .into_owned(),
                ],
            },
            Format::Human,
        )
        .unwrap_err();
        assert_eq!(error.downcast_ref::<ExitCode>().unwrap().0, 127);
        assert_eq!(block_on(store.get(row.id)).unwrap().last_used_at, None);
    }

    #[test]
    fn rm_rejects_protected_ciphertext() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        let store = open_store(&paths).unwrap();
        let controller = VaultController::new(store.clone());
        block_on(controller.create("test-master-password")).unwrap();
        assert!(block_on(controller.unlock("test-master-password")).unwrap());
        let envelope = controller.seal_for_storage(b"secret").unwrap().unwrap();
        let outcome = block_on(store.upsert_protected_bound(
            None,
            None,
            None,
            &envelope.ciphertext,
            6,
            1,
            &envelope.vault_envelope,
        ))
        .unwrap();
        let id = match outcome {
            UpsertOutcome::Inserted { id } | UpsertOutcome::Updated { id } => id,
        };
        assert!(rm(&paths, RmArgs { id }, Format::Json).is_err());
        let row = block_on(store.get(id)).unwrap();
        assert_eq!(row.kind, ClipboardItemKind::Protected);
        assert!(row.retention.protected_);
    }

    #[cfg(windows)]
    #[test]
    fn child_receives_exact_argument_and_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        let text = "日本語\nquotes \" ' ; $(not code) & --json";
        add_text(&paths, text);
        let store = open_store(&paths).unwrap();
        let row = block_on(store.snapshot()).unwrap().remove(0);
        let output = tmp.path().join("received.txt");
        let script = tmp.path().join("child.ps1");
        std::fs::write(&script, "param([string]$OutputPath,[string]$Payload)\n[IO.File]::WriteAllText($OutputPath, $Payload)\nexit 23\n").unwrap();
        let error = run(
            &paths,
            CpArgs {
                list: false,
                search: None,
                limit: None,
                id: Some(row.id),
                command: vec![
                    "pwsh".into(),
                    "-NoProfile".into(),
                    "-File".into(),
                    script.to_string_lossy().into_owned(),
                    output.to_string_lossy().into_owned(),
                    "{{cp}}".into(),
                ],
            },
            Format::Human,
        )
        .unwrap_err();
        assert_eq!(error.downcast_ref::<ExitCode>().unwrap().0, 23);
        assert_eq!(std::fs::read_to_string(output).unwrap(), text);
        assert!(block_on(store.get(row.id)).unwrap().last_used_at.is_some());
    }
}
