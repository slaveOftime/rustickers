//! `rst command` — create a command sticker.
//!
//! This is the command with the most surface, because a command sticker is the most configurable
//! thing Rustickers has: it can render its output five different ways, run on a schedule, run
//! invisibly, or wait to be fed the text you have selected in another application.
//!
//! What gets stored is an argument vector, not a shell line. Everything after `--` is kept token
//! for token ([`shell::encode_argv`]), so an argument with spaces, quotes, backslashes, newlines or
//! nothing in it at all arrives at the program exactly as typed. A shell is available with
//! `--shell "…"`, and it is exclusive: either a script for the shell, or a program and arguments.
//!
//! [`CommandArgs::validate`] does the entire job except writing, which is what lets `rst skill` prove a
//! recipe is well formed without creating anything.

use anyhow::{Context as _, bail};
use clap::ValueEnum;
use serde_json::json;
use std::path::{Component, Path, PathBuf};

use crate::model::content::{
    CommandContent, CommandResult, SELECTION_ENV_VAR, SELECTION_PLACEHOLDER, Scheduler,
};
use crate::model::sticker::{StickerColor, StickerType};
use crate::utils::time::now_unix_millis;

use crate::cli::draft::{Geometry, StickerDraft};
use crate::cli::output::{Format, ellipsize, sanitize};
use crate::cli::runtime::Runtime;
use crate::cli::shell;

/// How a command sticker renders whatever its command writes to stdout and stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ResultFormat {
    /// Plain text. The safe default.
    Text,
    /// Rendered markdown: headings, lists, tables, code blocks.
    Markdown,
    /// The output is a full HTML document, shown in an embedded browser view.
    Html,
    /// The output is an SVG document, drawn as an image.
    Svg,
    /// The output is a single file path or URL, opened in the file preview.
    Source,
}

impl ResultFormat {
    fn to_result(self) -> CommandResult {
        match self {
            Self::Text => CommandResult::Text(None),
            Self::Markdown => CommandResult::Markdown(None),
            Self::Html => CommandResult::Html(None),
            Self::Svg => CommandResult::Svg(None),
            Self::Source => CommandResult::Source(None),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Markdown => "markdown",
            Self::Html => "html",
            Self::Svg => "svg",
            Self::Source => "source",
        }
    }

    /// Whether output can be appended line by line as it arrives. The other formats need a
    /// complete document before they can render anything.
    fn supports_streaming(self) -> bool {
        matches!(self, Self::Text | Self::Markdown)
    }
}

#[derive(clap::Args, Debug)]
pub struct CommandArgs {
    /// Run this script through the system shell instead of a program and arguments
    ///
    /// Pipes, redirection and `&&` work inside the script. Cannot combine with `--`: a script is
    /// interpreted by a shell, an argument vector is not.
    #[arg(
        long,
        value_name = "SCRIPT",
        conflicts_with = "argv",
        help_heading = "Command"
    )]
    pub shell: Option<String>,

    /// Program to run and its arguments, each kept exactly as typed
    #[arg(last = true, num_args = 1.., required_unless_present = "shell", value_name = "PROGRAM_ARGS", help_heading = "Command")]
    pub argv: Vec<String>,

    /// Store the sticker without checking that the program is on PATH
    ///
    /// Everything else is still checked: the argument encoding, the placeholder rules, the
    /// schedule, the working directory, the environment. A program that is not installed fails when
    /// the sticker runs.
    #[arg(long, help_heading = "Command")]
    pub allow_missing_program: bool,

    /// Title shown in the sticker list and the selection picker
    #[arg(long, short = 't', value_name = "TEXT")]
    pub title: Option<String>,

    /// How to render the output
    #[arg(long, value_enum, default_value = "text", value_name = "FORMAT")]
    pub result: ResultFormat,

    /// Offer this sticker when you trigger the selected-text hotkey
    ///
    /// The selected text replaces {{RUSTICKERS_SELECTION}} in the arguments, and is always exported
    /// as the RUSTICKERS_SELECTION environment variable. Created closed: a selection sticker is
    /// offered from the hotkey, not left sitting on the desktop.
    #[arg(long, help_heading = "Behaviour")]
    pub accept_selection: bool,

    /// Run on a schedule, as a 6-field Quartz cron expression
    ///
    /// Fields are: second minute hour day-of-month month day-of-week. "0 */5 * * * *" is every
    /// five minutes. The schedule runs for as long as the app does, whether or not the window is
    /// open, so `rst result` has the latest run waiting.
    #[arg(long, value_name = "EXPR", help_heading = "Behaviour")]
    pub cron: Option<String>,

    /// With --cron, also run once immediately when started from the sticker's own window
    #[arg(long = "run-now", requires = "cron", help_heading = "Behaviour")]
    pub run_now: bool,

    /// Create the sticker stopped, so nothing runs until you press Run
    ///
    /// Without this the sticker is armed: it runs when its window opens, and again each time the
    /// app starts, which is what makes `rst result` useful.
    #[arg(long, help_heading = "Behaviour")]
    pub idle: bool,

    /// Show output line by line while the command runs, clearing the previous run first
    #[arg(long, help_heading = "Behaviour")]
    pub stream: bool,

    /// Never show a window; it only appears if the command fails
    #[arg(long = "no-window", help_heading = "Behaviour")]
    pub no_window: bool,

    /// Close the window once the command finishes successfully
    #[arg(long = "auto-close", help_heading = "Behaviour")]
    pub auto_close: bool,

    /// Environment variable for the command, repeatable
    ///
    /// The name is a portable identifier and the value is stored exactly as typed. Duplicate names
    /// are refused.
    #[arg(
        long,
        short = 'e',
        value_name = "KEY=VALUE",
        help_heading = "Behaviour"
    )]
    pub env: Vec<String>,

    /// Working directory for the command, which has to exist
    ///
    /// The sticker keeps this path and runs the command in it later, from the desktop app, whatever
    /// the current directory is by then, so a relative path — and the default, the directory this
    /// was run from — is pinned to an absolute one here.
    #[arg(long, value_name = "PATH", help_heading = "Behaviour")]
    pub dir: Option<PathBuf>,

    /// Padding around the output, 0-64 pixels
    #[arg(long, value_name = "PX", value_parser = parse_padding, help_heading = "Appearance")]
    pub padding: Option<u8>,

    #[command(flatten)]
    pub geometry: Geometry,
}

/// Everything about the sticker this call would create, resolved and checked with no database
/// involved. Building one is the whole of [`CommandArgs::validate`].
struct Plan {
    /// The string the sticker stores, and later splits back into argv.
    command: String,
    /// The typed argument vector; empty for `--shell`, which has a script instead.
    argv: Vec<String>,
    script: Option<String>,
    inspection: shell::Inspection,
    cron: Option<String>,
    environments: String,
    env_names: Vec<String>,
    working_dir: String,
    title: String,
    accept_selection: bool,
    run_now: bool,
    /// `Some` when the sticker is armed to run on its own; `None` means it waits for a person to
    /// press Run, or for the selected-text hotkey.
    started_at: Option<i64>,
    closed: bool,
    warnings: Vec<String>,
}

impl Plan {
    /// The shared geometry with this command's one override: a selection sticker is created closed,
    /// so offering it on the hotkey never puts a window in the way.
    fn geometry(&self, base: &Geometry) -> Geometry {
        Geometry {
            closed: self.closed,
            ..base.clone()
        }
    }

    /// What was asked for, in the shape it was typed.
    fn display(&self) -> &str {
        self.script.as_deref().unwrap_or(&self.command)
    }

    /// When the sticker will actually run. About the sticker, not about this call: nothing has run
    /// yet, and for an idle or scheduled sticker nothing runs from here either.
    fn runs_line(&self, id: i64) -> String {
        if self.accept_selection {
            "only when the selected-text hotkey feeds it".to_owned()
        } else if self.started_at.is_none() {
            format!("not until you press Run — `rst open {id}` shows the window")
        } else if let Some(cron) = &self.cron {
            format!(
                "on schedule ({cron}); the first run waits for the next tick{}",
                if self.run_now {
                    ", plus once when the sticker's own window starts it"
                } else {
                    ""
                }
            )
        } else {
            "when its window opens, and again each time Rustickers starts".to_owned()
        }
    }
}

impl CommandArgs {
    /// Check everything about these arguments that needs no database and writes nothing.
    ///
    /// This is all of [`run`] except the insert, so `rst skill` can say whether a recipe is well
    /// formed without creating a sticker.
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        self.plan()?;
        Ok(())
    }

    /// Resolve every argument into the exact data a sticker would store, or explain the problem.
    fn plan(&self) -> anyhow::Result<Plan> {
        self.check_combinations()?;

        let (command, argv, script) = self.prepare_command()?;
        // Structure is always checked; `--allow-missing-program` skips only the PATH lookup.
        let inspection = shell::inspect(&command)?;
        anyhow::ensure!(
            self.accept_selection || !inspection.uses_selection_placeholder,
            "{SELECTION_PLACEHOLDER} requires --accept-selection"
        );
        let working_dir = resolve_dir(self.dir.as_deref())?;
        let inspection = if self.allow_missing_program {
            inspection
        } else {
            inspection.resolve_program_in(&working_dir)?
        };

        let cron = self.cron.as_deref().map(shell::validate_cron).transpose()?;
        let env = parse_env(&self.env)?;
        let warnings = self.collect_warnings(&inspection, cron.as_deref(), &env);

        // `started_at` is what arms a command sticker: the window only runs or schedules its command
        // when a start time is present. A selection sticker is the exception — the hotkey sets its
        // own start time, and arming it would run it with no selection on every app launch.
        let started_at = (!self.idle && !self.accept_selection).then(now_unix_millis);

        Ok(Plan {
            command,
            argv,
            script: script.clone(),
            inspection,
            cron,
            environments: encode_env(&env),
            env_names: env.iter().map(|var| var.name.clone()).collect(),
            working_dir: into_stored_path(working_dir)?,
            title: self.title.clone().unwrap_or_else(|| {
                ellipsize(&script.unwrap_or_else(|| display_argv(&self.argv)), 40)
            }),
            accept_selection: self.accept_selection,
            run_now: self.run_now,
            started_at,
            closed: self.geometry.closed || self.accept_selection,
            warnings,
        })
    }

    /// The program and arguments, or the shell script, as the one string the sticker stores.
    fn prepare_command(&self) -> anyhow::Result<(String, Vec<String>, Option<String>)> {
        if let Some(script) = &self.shell {
            if script.trim().is_empty() {
                bail!(
                    "--shell was given an empty script; pass the command the shell should run, \
                     or drop --shell and use -- <program> <args>..."
                );
            }
            if script.contains(SELECTION_PLACEHOLDER) {
                bail!(
                    "{SELECTION_PLACEHOLDER} cannot be used with --shell. The selection is \
                     substituted into an argument the shell then re-reads, so whatever text is \
                     selected would be parsed as shell code. Run the program directly with -- \
                     <program> <args>... and the placeholder in an argument, or read the selection \
                     from {SELECTION_ENV_VAR}, which the shell inherits."
                );
            }
            return Ok((
                shell::wrap_in_shell(script),
                Vec::new(),
                Some(script.to_owned()),
            ));
        }

        if self.argv.is_empty() {
            bail!(
                "no command to run. Pass it after `--`: rst command [OPTIONS] -- <program> \
                 <args>..., or give --shell \"<script>\" to run through the system shell."
            );
        }

        Ok((shell::encode_argv(&self.argv)?, self.argv.clone(), None))
    }

    /// Refuse what cannot work, and what would be stored, accepted, and then quietly do nothing.
    /// The choices that do work but are not what they look like go to
    /// [`Self::collect_warnings`].
    fn check_combinations(&self) -> anyhow::Result<()> {
        if self.stream && !self.result.supports_streaming() {
            bail!(
                "--stream only renders progressively for --result text or markdown; --result {} \
                 needs a complete document before it can render. Drop --stream, or choose text or \
                 markdown.",
                self.result.as_str()
            );
        }
        anyhow::ensure!(
            !self.run_now || self.cron.is_some(),
            "--run-now requires --cron"
        );
        anyhow::ensure!(
            !(self.cron.is_some() && (self.auto_close || self.no_window)),
            "--auto-close and --no-window have no effect on armed schedules; use --closed instead"
        );
        if self.accept_selection && self.cron.is_some() {
            bail!(
                "--cron and --accept-selection cannot combine: a schedule fires on time with no \
                 selection to pass. A selection sticker runs from the selected-text hotkey, and \
                 that is the only thing that feeds it."
            );
        }
        if self.idle && self.accept_selection {
            bail!(
                "--idle is inert with --accept-selection: a selection sticker is never armed, it \
                 runs only when the selected-text hotkey feeds it."
            );
        }
        if self.idle && self.run_now {
            bail!(
                "--idle and --run-now contradict each other: a stopped sticker has nothing to \
                 start immediately."
            );
        }
        if self.auto_close && self.no_window {
            bail!(
                "--auto-close is inert with --no-window: a hidden window already disposes of \
                 itself when the command succeeds."
            );
        }
        if self.padding.is_some() && self.result == ResultFormat::Source {
            bail!(
                "--padding has no effect on --result source: the file it names is opened in the \
                 file preview, which is not drawn with the sticker's padding."
            );
        }
        Ok(())
    }

    /// Say anything about a combination that is accepted, does work, and is still not what it looks
    /// like. The sticker runtime resolves all of these silently, so this is the last moment when
    /// the person (or agent) that chose them is still paying attention.
    fn collect_warnings(
        &self,
        inspection: &shell::Inspection,
        cron: Option<&str>,
        env: &[EnvVar],
    ) -> Vec<String> {
        let mut warnings = Vec::new();

        // Shell syntax inside a `--shell` script reaches a shell, which is the whole point; only a
        // bare argument vector passes it through as text.
        if self.shell.is_none() && !inspection.literal_shell_syntax.is_empty() {
            warnings.push(format!(
                "{} will reach the program as literal text, not interpreted — the sticker runs the \
                 program directly; use --shell \"…\" if you meant to use a shell.",
                inspection
                    .literal_shell_syntax
                    .iter()
                    .map(|meta| format!("'{meta}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        if self.allow_missing_program {
            warnings.push(format!(
                "'{}' was not looked up on PATH (--allow-missing-program), so nothing here proves \
                 it can run; the sticker fails until the program exists.",
                inspection.program
            ));
        }

        if self.accept_selection && !inspection.uses_selection_placeholder {
            warnings.push(format!(
                "the command has no {SELECTION_PLACEHOLDER} argument, so the selected text will \
                 only reach it through the {SELECTION_ENV_VAR} environment variable."
            ));
        }
        if cron.is_some() {
            warnings.push(
                "a scheduled sticker waits for its first tick, so `rst result` stays empty until \
                 then."
                    .to_owned(),
            );
        }
        if self.accept_selection
            && env
                .iter()
                .any(|var| var.name.eq_ignore_ascii_case(SELECTION_ENV_VAR))
        {
            warnings.push(format!(
                "a selection run overwrites {SELECTION_ENV_VAR} with the selected text, so the \
                 value passed with --env is replaced every time the hotkey feeds it."
            ));
        }

        warnings
    }
}

fn parse_padding(s: &str) -> Result<u8, String> {
    match s.parse::<u8>() {
        Ok(value) if value <= 64 => Ok(value),
        _ => Err(format!("'{s}' is not a whole number between 0 and 64")),
    }
}

/// One `--env KEY=VALUE` pair, with the value exactly as typed.
#[derive(Debug, PartialEq, Eq)]
struct EnvVar {
    name: String,
    value: String,
}

/// Parse and check the `--env KEY=VALUE` arguments.
///
/// The name has to be a portable identifier, because it reaches a child process on every platform;
/// the value is kept exactly as typed, because a value that means something to the program (`" `, a
/// padded column, a token with a tab in it) is not ours to change.
fn parse_env(pairs: &[String]) -> anyhow::Result<Vec<EnvVar>> {
    let mut vars: Vec<EnvVar> = Vec::with_capacity(pairs.len());

    for pair in pairs {
        let Some((name, value)) = pair.split_once('=') else {
            bail!("--env expects KEY=VALUE, got '{pair}'");
        };

        if !is_portable_identifier(name) {
            bail!(
                "'{name}' is not a portable environment variable name; use letters, digits and _, \
                 starting with a letter or _"
            );
        }
        if value.contains(['\n', '\r', '\0']) {
            bail!(
                "{name} cannot be stored: values are kept one per line, so a newline, a carriage \
                 return or a NUL byte cannot survive into the command"
            );
        }
        if let Some(existing) = vars.iter().find(|var| var.name.eq_ignore_ascii_case(name)) {
            bail!(
                "--env {name} is given twice (as {}); the command would get one of them and which \
                 one depends on the platform",
                existing.name
            );
        }

        vars.push(EnvVar {
            name: name.to_owned(),
            value: value.to_owned(),
        });
    }

    Ok(vars)
}

/// The stored environment block: one `KEY=VALUE` per line, values untouched.
fn encode_env(vars: &[EnvVar]) -> String {
    vars.iter()
        .map(|var| format!("{}={}", var.name, var.value))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_portable_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Show an argument vector the way it was typed, quoting only what would otherwise fall apart.
fn display_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|token| {
            if token.is_empty() || token.contains(char::is_whitespace) || token.contains('"') {
                format!("\"{token}\"")
            } else {
                token.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Turn the working directory into an absolute path that means the same thing later, and require
/// that it exists.
///
/// The sticker runs inside the desktop app, whose current directory is unrelated to the shell the
/// CLI was invoked from, so `--dir .` — and the default, which is the current directory — has to be
/// pinned down here to mean what was meant. A missing directory is refused now rather than failing
/// inside the app, far from the command that created it.
fn resolve_dir(dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    let current = std::env::current_dir().context("resolve the current directory")?;

    let Some(dir) = dir else {
        return Ok(current);
    };
    if dir.as_os_str().is_empty() {
        bail!("--dir is empty; omit --dir to use the current directory");
    }

    let absolute = if dir.is_absolute() {
        dir.to_owned()
    } else {
        pin_against(&current, dir)
    };

    anyhow::ensure!(
        absolute.exists(),
        "--dir '{}' does not exist",
        absolute.display()
    );
    anyhow::ensure!(
        absolute.is_dir(),
        "--dir '{}' is not a directory",
        absolute.display()
    );

    Ok(absolute)
}

/// Resolve a relative path against `base` without touching the filesystem.
///
/// `canonicalize` would return a `\\?\` verbatim path that some programs mishandle, so `.` and `..`
/// are folded lexically. That is enough to pin a path to the directory it was typed in.
fn pin_against(base: &Path, path: &Path) -> PathBuf {
    let mut resolved = base.to_path_buf();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    resolved
}

/// The working directory as the sticker stores it: a string.
fn into_stored_path(path: PathBuf) -> anyhow::Result<String> {
    path.into_os_string().into_string().map_err(|_| {
        anyhow::anyhow!(
            "the working directory is not valid text, and the sticker stores it as text; put the \
             command in a directory with a plain name"
        )
    })
}

pub(crate) fn run(runtime: &Runtime, args: CommandArgs, format: Format) -> anyhow::Result<()> {
    let plan = args.plan()?;

    let content = CommandContent {
        command: plan.command.clone(),
        environments: plan.environments.clone(),
        working_dir: plan.working_dir.clone(),
        scheduler: plan.cron.clone().map(Scheduler::Cron),
        run_immediately: args.run_now,
        result: args.result.to_result(),
        stream_result: args.stream,
        padding: args.padding,
        started_at: plan.started_at,
        accept_selection: args.accept_selection,
        auto_close: args.auto_close,
        run_without_window: args.no_window,
    };

    let store = runtime.store()?;
    let created = StickerDraft {
        title: plan.title.clone(),
        sticker_type: StickerType::Command,
        content: serde_json::to_string(&content)?,
        default_color: StickerColor::Gray,
        default_width: 400,
        default_height: 300,
    }
    .create(runtime, &store, &plan.geometry(&args.geometry))?;

    let mut payload = created.json("command");
    payload["title"] = json!(plan.title);
    payload["command"] = json!(plan.command);
    payload["argv"] = json!(plan.argv);
    payload["shell"] = json!(plan.script);
    payload["program"] = json!(plan.inspection.program_display());
    payload["program_resolved"] = json!(plan.inspection.program_path.is_some());
    payload["result_format"] = json!(args.result.as_str());
    payload["stream"] = json!(args.stream);
    payload["accept_selection"] = json!(args.accept_selection);
    payload["cron"] = json!(plan.cron);
    payload["run_immediately"] = json!(args.run_now);
    payload["run_without_window"] = json!(args.no_window);
    payload["auto_close"] = json!(args.auto_close);
    payload["working_dir"] = json!(plan.working_dir);
    payload["environments"] = json!(plan.env_names);
    payload["armed"] = json!(plan.started_at.is_some());
    payload["warnings"] = json!(plan.warnings);

    let result_format = args.result.as_str();
    let accept_selection = args.accept_selection;
    let no_window = args.no_window;
    format.emit(payload, || {
        created.report(format, "command");
        println!("  title:     {}", sanitize(&plan.title));
        println!("  command:   {}", sanitize(plan.display()));
        if plan.script.is_some() {
            println!("  via shell: {}", plan.inspection.program_display());
        }
        println!("  output:    {result_format}");
        println!("  working:   {}", plan.working_dir);
        if let Some(cron) = &plan.cron {
            println!("  schedule:  {cron}");
        }
        if accept_selection {
            println!("  selection: offered on the selected-text hotkey");
        }
        if no_window {
            println!("  window:    hidden unless the command fails");
        }
        println!("  runs:      {}", plan.runs_line(created.id));
        println!(
            "  result:    rst result {} — nothing has run yet, so it stays empty until a run \
             finishes",
            created.id
        );
        for warning in &plan.warnings {
            println!("note: {warning}");
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::command_line;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Arguments as clap would deliver them, with the defaults that keep the checks quiet.
    fn args_for(argv: &[&str]) -> CommandArgs {
        CommandArgs {
            shell: None,
            argv: strings(argv),
            allow_missing_program: true,
            title: None,
            result: ResultFormat::Text,
            accept_selection: false,
            cron: None,
            run_now: false,
            idle: false,
            stream: false,
            no_window: false,
            auto_close: false,
            env: Vec::new(),
            dir: Some(std::env::current_dir().unwrap()),
            padding: None,
            geometry: Geometry {
                width: None,
                height: None,
                left: None,
                top: None,
                color: None,
                top_most: false,
                closed: true,
            },
        }
    }

    fn error_for(args: &CommandArgs) -> String {
        format!("{:#}", args.validate().unwrap_err())
    }

    #[test]
    fn an_argument_vector_is_stored_and_splits_back_into_argv() {
        let plan = args_for(&["cargo", "test", "--all-features"])
            .plan()
            .unwrap();
        assert_eq!(plan.command, "cargo test --all-features");
    }

    #[test]
    fn arguments_are_stored_exactly_enough_quoted_to_split_back_apart() {
        for argv in [
            &["git", "commit", "-m", "two words"][..],
            &["git", "log", r#"--format="%h %s""#],
            &["cmd", r"C:\temp\", r"x\"],
            &["cmd", "", "x", ""],
            &["cmd", "line1\nline2", "a\tb"],
            &["cmd", "日本語", "😀 emoji"],
        ] {
            let plan = args_for(argv).plan().unwrap();
            assert_eq!(
                command_line::split(&plan.command).unwrap(),
                argv,
                "stored as {:?}",
                plan.command
            );
        }
    }

    #[test]
    fn no_command_at_all_is_rejected_with_the_grammar_in_the_message() {
        let error = error_for(&args_for(&[]));
        assert!(error.contains("no command to run"), "{error}");
        assert!(error.contains("--shell"), "{error}");
    }

    #[test]
    fn a_nul_byte_is_rejected_even_when_the_program_lookup_is_skipped() {
        assert!(error_for(&args_for(&["cmd", "a\0b"])).contains("NUL"));
    }

    #[test]
    fn a_placeholder_in_the_program_position_is_rejected_even_then() {
        let error = error_for(&args_for(&[SELECTION_PLACEHOLDER, "--flag"]));
        assert!(error.contains("only substituted in arguments"), "{error}");
    }

    #[test]
    fn a_shell_script_is_wrapped_and_kept_as_one_argument() {
        let mut args = args_for(&[]);
        args.shell = Some("git status | findstr modified".to_owned());
        let plan = args.plan().unwrap();

        assert_eq!(plan.argv, Vec::<String>::new());
        assert_eq!(
            command_line::split(&plan.command).unwrap().last().unwrap(),
            "git status | findstr modified"
        );
        // The metacharacters reach a shell, so they must not be called literal text.
        assert!(plan.warnings.iter().all(|w| !w.contains("literal text")));
    }

    #[test]
    fn an_empty_shell_script_is_rejected_before_it_is_wrapped() {
        for script in ["", "   ", "\n"] {
            let mut args = args_for(&[]);
            args.shell = Some(script.to_owned());
            assert!(error_for(&args).contains("empty script"));
        }
    }

    #[test]
    fn a_selection_placeholder_in_a_shell_script_is_rejected_as_injection() {
        let mut args = args_for(&[]);
        args.shell = Some(format!("echo {SELECTION_PLACEHOLDER}"));
        args.accept_selection = true;
        let error = error_for(&args);
        assert!(error.contains(SELECTION_PLACEHOLDER), "{error}");
        assert!(error.contains("shell code"), "{error}");
        assert!(error.contains(SELECTION_ENV_VAR), "{error}");
    }

    #[test]
    fn a_selection_sticker_is_closed_unarmed_and_reports_both() {
        let mut args = args_for(&["git", "log", "-S", SELECTION_PLACEHOLDER]);
        args.accept_selection = true;

        let plan = args.plan().unwrap();
        assert!(plan.closed);
        assert!(plan.geometry(&args.geometry).closed);
        assert_eq!(plan.started_at, None);
        assert!(plan.runs_line(7).contains("selected-text hotkey"));
        assert!(
            !plan.warnings.iter().any(|w| w.contains("created closed")),
            "{:?}",
            plan.warnings
        );
    }

    #[test]
    fn run_now_needs_a_cron_expression() {
        let mut args = args_for(&["git", "status"]);
        args.run_now = true;
        assert!(error_for(&args).contains("--cron"));
    }

    #[test]
    fn stream_is_limited_to_the_formats_that_render_progressively() {
        for (result, name) in [
            (ResultFormat::Html, "html"),
            (ResultFormat::Svg, "svg"),
            (ResultFormat::Source, "source"),
        ] {
            let mut args = args_for(&["git", "status"]);
            args.stream = true;
            args.result = result;
            let error = error_for(&args);
            assert!(
                error.contains(name) && error.contains("--stream"),
                "{error}"
            );
        }

        let mut args = args_for(&["git", "status"]);
        args.stream = true;
        args.result = ResultFormat::Markdown;
        assert!(args.validate().is_ok());
    }

    #[test]
    fn combinations_that_cannot_work_or_would_do_nothing_are_refused() {
        let mut args = args_for(&["git", "status"]);
        args.cron = Some("0 */5 * * * *".to_owned());
        args.accept_selection = true;
        assert!(error_for(&args).contains("--cron and --accept-selection"));

        args.cron = None;
        args.idle = true;
        assert!(error_for(&args).contains("inert")); // --idle with --accept-selection

        let mut args = args_for(&["git", "status"]);
        args.idle = true;
        args.run_now = true;
        args.cron = Some("0 */5 * * * *".to_owned());
        assert!(error_for(&args).contains("--idle and --run-now"));

        args.idle = false;
        args.auto_close = true;
        args.no_window = true;
        assert!(error_for(&args).contains("--auto-close"));

        let mut args = args_for(&["git", "status"]);
        args.padding = Some(8);
        args.result = ResultFormat::Source;
        assert!(error_for(&args).contains("--padding"));
    }

    #[test]
    fn a_five_field_schedule_is_rejected_with_the_six_field_fix() {
        let mut args = args_for(&["git", "status"]);
        args.cron = Some("*/5 * * * *".to_owned());
        assert!(error_for(&args).contains("0 */5 * * * *"));
    }

    #[test]
    fn skipping_the_program_lookup_says_what_was_not_checked() {
        let args = args_for(&["definitely-not-a-real-program-xyz"]);
        assert!(
            args.plan()
                .unwrap()
                .warnings
                .iter()
                .any(|w| w.contains("--allow-missing-program"))
        );

        let mut checked = args;
        checked.allow_missing_program = false;
        assert!(error_for(&checked).contains("not on PATH"));
    }

    #[test]
    fn a_working_directory_that_is_missing_or_not_a_directory_is_refused() {
        let mut args = args_for(&["git", "status"]);
        args.dir = Some(PathBuf::from("no-such-directory-here-xyz"));
        assert!(error_for(&args).contains("does not exist"));

        args.dir = Some(std::env::current_dir().unwrap().join("Cargo.toml"));
        assert!(error_for(&args).contains("not a directory"));

        args.dir = Some(PathBuf::new());
        assert!(error_for(&args).contains("empty"));
    }

    #[test]
    fn the_working_directory_is_pinned_to_an_absolute_one() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(resolve_dir(None).unwrap(), here); // the default is pinned too

        let mut args = args_for(&["git", "status"]);
        args.dir = Some(PathBuf::from("src/../src"));
        let pinned = args.plan().unwrap().working_dir;
        assert_eq!(Path::new(&pinned), here.join("src"));
        assert!(
            !pinned.contains(r"\\?\") && !pinned.contains("/?/"),
            "{pinned}"
        );
    }

    #[test]
    fn env_pairs_keep_their_value_and_their_order() {
        let env = parse_env(&strings(&["A=1", "B=two words", "URL=a=b", "EMPTY="])).unwrap();
        assert_eq!(encode_env(&env), "A=1\nB=two words\nURL=a=b\nEMPTY=");
        assert_eq!(encode_env(&parse_env(&[]).unwrap()), "");
    }

    #[test]
    fn env_values_are_not_trimmed_or_leaked_in_warnings() {
        let env = parse_env(&strings(&["PADDED=  spaced  "])).unwrap();
        assert_eq!(env[0].value, "  spaced  ");

        let mut args = args_for(&["git", "status"]);
        args.env = strings(&["PADDED=  spaced  "]);
        let warnings = args.plan().unwrap().warnings;
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("trims") || w.contains("spaced")),
            "{warnings:?}"
        );
    }

    #[test]
    fn env_names_have_to_be_portable_identifiers() {
        for name in ["", "9LIVES", "HAS SPACE", "a-b", "$HOME", "ünicode"] {
            let error = format!("{:#}", parse_env(&[format!("{name}=value")]).unwrap_err());
            assert!(
                error.contains("portable environment variable name"),
                "{error}"
            );
        }
        assert!(parse_env(&strings(&["_OK=1", "A1_B=2"])).is_ok());
    }

    #[test]
    fn a_duplicate_env_name_is_refused_whichever_spelling_wins() {
        assert!(
            format!("{:#}", parse_env(&strings(&["A=1", "A=2"])).unwrap_err()).contains("twice")
        );
        assert!(
            format!("{:#}", parse_env(&strings(&["A=1", "a=2"])).unwrap_err()).contains("twice")
        );
    }

    #[test]
    fn an_env_value_cannot_carry_a_line_break_or_a_nul_byte() {
        for value in ["a\nb", "a\rb", "a\0b"] {
            assert!(parse_env(&[format!("KEY={value}")]).is_err(), "{value:?}");
        }
    }

    #[test]
    fn a_selection_run_overwrites_the_selection_variable_it_is_given() {
        let mut args = args_for(&["git", "log", "-S", SELECTION_PLACEHOLDER]);
        args.accept_selection = true;
        args.env = vec![format!("{SELECTION_ENV_VAR}=preset")];
        assert!(
            args.plan()
                .unwrap()
                .warnings
                .iter()
                .any(|w| w.contains("overwrites"))
        );
    }

    #[test]
    fn the_report_says_when_the_sticker_actually_runs() {
        let mut args = args_for(&["git", "status"]);
        assert!(
            args.plan()
                .unwrap()
                .runs_line(1)
                .contains("each time Rustickers starts")
        );

        args.idle = true;
        assert!(args.plan().unwrap().runs_line(12).contains("press Run"));

        args.idle = false;
        args.cron = Some("0 */5 * * * *".to_owned());
        let plan = args.plan().unwrap();
        assert!(plan.runs_line(3).contains("waits for the next tick"));
        assert!(plan.warnings.iter().any(|w| w.contains("first tick")));

        args.run_now = true;
        assert!(
            args.plan()
                .unwrap()
                .runs_line(3)
                .contains("own window starts it")
        );
    }

    #[test]
    fn the_title_falls_back_to_the_command_it_runs() {
        assert_eq!(
            args_for(&["git", "status"]).plan().unwrap().title,
            "git status"
        );
        assert_eq!(
            args_for(&["git", "commit", "-m", "a b"])
                .plan()
                .unwrap()
                .title,
            "git commit -m \"a b\""
        );

        let mut args = args_for(&[]);
        args.shell = Some("git status | findstr modified".to_owned());
        assert_eq!(args.plan().unwrap().title, "git status | findstr modified");

        let args = args_for(&["git", &"字".repeat(80)]);
        assert_eq!(args.plan().unwrap().title.chars().count(), 41);
    }

    #[test]
    fn padding_is_limited_to_the_sliders_range() {
        assert_eq!(parse_padding("64"), Ok(64));
        assert_eq!(parse_padding("0"), Ok(0));
        assert!(parse_padding("65").is_err());
        assert!(parse_padding("-1").is_err());
    }
}
