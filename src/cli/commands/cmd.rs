//! `rusticker cmd` — create a command sticker.
//!
//! This is the command with the most surface, because a command sticker is the most configurable
//! thing Rustickers has: it can render its output five different ways, run on a schedule, run
//! invisibly, or wait to be fed the text you have selected in another application.

use anyhow::{Context as _, bail};
use clap::ValueEnum;
use serde_json::json;
use std::path::Path;

use crate::model::content::{
    CommandContent, CommandResult, SELECTION_ENV_VAR, SELECTION_PLACEHOLDER, Scheduler,
};
use crate::model::sticker::{StickerColor, StickerType};
use crate::storage::paths::AppPaths;
use crate::utils::time::now_unix_millis;

use crate::cli::draft::{Geometry, StickerDraft};
use crate::cli::output::{Format, ellipsize};
use crate::cli::runtime::open_store;
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
pub struct CmdArgs {
    /// The command to run, as a single string
    ///
    /// It is split with Windows argument rules and the program is looked up on PATH. There is no
    /// shell, so pipes, redirection and `&&` are literal text unless you pass --shell.
    #[arg(value_name = "COMMAND")]
    pub command: String,

    /// Title shown in the sticker list and the selection picker
    #[arg(long, short = 't', value_name = "TEXT")]
    pub title: Option<String>,

    /// How to render the output
    #[arg(long, value_enum, default_value = "text", value_name = "FORMAT")]
    pub result: ResultFormat,

    /// Run the command through the system shell, so pipes and redirection work
    #[arg(long)]
    pub shell: bool,

    /// Offer this sticker when you trigger the selected-text hotkey
    ///
    /// The selected text replaces {{RUSTICKERS_SELECTION}} in the arguments, and is always
    /// exported as the RUSTICKERS_SELECTION environment variable.
    #[arg(long, help_heading = "Behaviour")]
    pub accept_selection: bool,

    /// Run on a schedule, as a 6-field Quartz cron expression
    ///
    /// Fields are: second minute hour day-of-month month day-of-week. "0 */5 * * * *" is every
    /// five minutes.
    #[arg(long, value_name = "EXPR", help_heading = "Behaviour")]
    pub cron: Option<String>,

    /// With --cron, also run once immediately when started from the sticker's own window
    ///
    /// It does not affect the run that follows creation, or a run resumed at app launch: those
    /// always wait for the first tick so that starting the app does not fire every schedule at
    /// once.
    #[arg(long = "run-now", help_heading = "Behaviour")]
    pub run_now: bool,

    /// Create the sticker stopped, so nothing runs until you press Run
    ///
    /// Without this the sticker is armed: it runs when its window opens, and again each time the
    /// app starts, which is what makes `rusticker result` useful.
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
    #[arg(long, value_name = "KEY=VALUE", help_heading = "Behaviour")]
    pub env: Vec<String>,

    /// Working directory for the command
    ///
    /// A relative path is resolved against the current directory now, because the sticker will
    /// later run from the desktop app's working directory, not from here.
    #[arg(long, value_name = "PATH", help_heading = "Behaviour")]
    pub dir: Option<String>,

    /// Padding around the output, 0-64 pixels
    #[arg(long, value_name = "PX", value_parser = parse_padding, help_heading = "Appearance")]
    pub padding: Option<u8>,

    /// Store the command even if its program cannot be found on PATH
    #[arg(long)]
    pub no_validate: bool,

    #[command(flatten)]
    pub geometry: Geometry,
}

fn parse_padding(s: &str) -> Result<u8, String> {
    match s.parse::<u8>() {
        Ok(value) if value <= 64 => Ok(value),
        _ => Err(format!("'{s}' is not a whole number between 0 and 64")),
    }
}

/// Turn `KEY=VALUE` arguments into the newline-separated form a command sticker stores.
fn parse_env(pairs: &[String]) -> anyhow::Result<String> {
    let mut lines = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            bail!("--env expects KEY=VALUE, got '{pair}'");
        };
        if key.trim().is_empty() {
            bail!("--env has an empty variable name in '{pair}'");
        }
        if value.contains('\n') {
            bail!("--env values cannot contain newlines ('{key}')");
        }
        lines.push(format!("{}={}", key.trim(), value.trim()));
    }
    Ok(lines.join("\n"))
}

/// Point out the combinations that are accepted but will not do what they look like they do.
///
/// These are all cases the sticker runtime resolves silently, so the only chance to say anything
/// is here, while the person (or agent) that chose them is still paying attention.
fn collect_warnings(
    args: &CmdArgs,
    inspection: Option<&shell::Inspection>,
    cron: Option<&String>,
) -> Vec<String> {
    let mut warnings = Vec::new();

    if let Some(inspection) = inspection
        && !inspection.literal_shell_syntax.is_empty()
    {
        warnings.push(format!(
            "{} will reach the program as literal text, not interpreted — pass --shell if you \
             meant to use a shell.",
            inspection
                .literal_shell_syntax
                .iter()
                .map(|s| format!("'{s}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let uses_placeholder = inspection.is_some_and(|i| i.uses_selection_placeholder);
    if args.accept_selection && !uses_placeholder {
        warnings.push(format!(
            "the command has no {SELECTION_PLACEHOLDER} argument, so the selected text will only \
             reach it through the {SELECTION_ENV_VAR} environment variable."
        ));
    }
    if !args.accept_selection && uses_placeholder {
        warnings.push(format!(
            "{SELECTION_PLACEHOLDER} is only substituted during a selection run — add \
             --accept-selection, or it is passed through literally."
        ));
    }
    if args.accept_selection && !args.geometry.closed {
        warnings.push(
            "a selection sticker does not have to be open to be offered; --closed creates it \
             without a window getting in the way."
                .to_owned(),
        );
    }

    if args.run_now && cron.is_none() {
        warnings.push(
            "--run-now only matters together with --cron; without a schedule the command already \
             runs whenever the sticker opens."
                .to_owned(),
        );
    }
    if cron.is_some() && !args.idle {
        warnings.push(
            "a scheduled sticker waits for its first tick, so `rusticker result` stays empty until \
             then."
                .to_owned(),
        );
    }
    if args.stream && !args.result.supports_streaming() {
        warnings.push(format!(
            "--stream only renders progressively for text and markdown; for {} it just clears the \
             previous output when a run starts.",
            args.result.as_str()
        ));
    }
    if args.auto_close && args.no_window {
        warnings.push(
            "--auto-close is redundant with --no-window: a hidden window already disposes of \
             itself when the command succeeds."
                .to_owned(),
        );
    }
    if cron.is_some() && (args.auto_close || args.no_window) {
        warnings.push(
            "a scheduled sticker is never closed automatically, because closing it would stop the \
             schedule."
                .to_owned(),
        );
    }
    if args.padding.is_some() && args.result == ResultFormat::Source {
        warnings.push("--padding has no effect on a source result.".to_owned());
    }
    if args.idle && args.accept_selection {
        warnings.push(
            "--idle is redundant with --accept-selection: a selection sticker only ever runs when \
             the selected-text hotkey feeds it."
                .to_owned(),
        );
    }
    if args.idle {
        warnings.push(
            "--idle means nothing runs yet, so `rusticker result` will stay empty until you press \
             Run on the sticker."
                .to_owned(),
        );
    }

    warnings
}

/// Turn a relative working directory into an absolute one.
///
/// The sticker runs inside the desktop app, whose current directory is unrelated to the shell the
/// CLI was invoked from, so `--dir .` has to be pinned down here to mean what the user meant.
fn resolve_dir(dir: Option<&str>) -> anyhow::Result<String> {
    let Some(dir) = dir.map(str::trim).filter(|dir| !dir.is_empty()) else {
        return Ok(String::new());
    };

    let path = Path::new(dir);
    if path.is_absolute() {
        return Ok(dir.to_owned());
    }

    let absolute = std::env::current_dir()
        .context("resolve the current directory for --dir")?
        .join(path);
    // `canonicalize` would give a \\?\ path that some programs mishandle, so only tidy up the
    // components we introduced ourselves.
    Ok(absolute
        .to_str()
        .map(|s| s.trim_end_matches("\\.").to_owned())
        .unwrap_or_else(|| absolute.to_string_lossy().into_owned()))
}

pub fn run(app_paths: &AppPaths, args: CmdArgs, format: Format) -> anyhow::Result<()> {
    let command = if args.shell {
        shell::wrap_in_shell(args.command.trim())
    } else {
        args.command.trim().to_owned()
    };

    if command.is_empty() {
        bail!("the command is empty");
    }

    let inspection = if args.no_validate {
        None
    } else {
        Some(shell::validate(&command)?)
    };
    let cron = args.cron.as_deref().map(shell::validate_cron).transpose()?;
    let working_dir = resolve_dir(args.dir.as_deref())?;

    // `started_at` is what arms a command sticker: the window only runs or schedules its command
    // when a start time is present. Leaving it unset would create a sticker that sits there doing
    // nothing until someone presses Run. A selection sticker is the exception — it is triggered by
    // the selected-text hotkey, which sets its own start time, and arming it would make it run
    // with no selection every time the app launches.
    let started_at = (!args.idle && !args.accept_selection).then(now_unix_millis);
    let warnings = collect_warnings(&args, inspection.as_ref(), cron.as_ref());

    let content = CommandContent {
        command: command.clone(),
        environments: parse_env(&args.env)?,
        working_dir: working_dir.clone(),
        scheduler: cron.clone().map(Scheduler::Cron),
        run_immediately: args.run_now,
        result: args.result.to_result(),
        stream_result: args.stream,
        padding: args.padding,
        started_at,
        accept_selection: args.accept_selection,
        auto_close: args.auto_close,
        run_without_window: args.no_window,
    };

    let title = args
        .title
        .clone()
        .unwrap_or_else(|| ellipsize(args.command.trim(), 40));

    let store = open_store(app_paths)?;
    let created = StickerDraft {
        title: title.clone(),
        sticker_type: StickerType::Command,
        content: serde_json::to_string(&content)?,
        default_color: StickerColor::Gray,
        default_width: 400,
        default_height: 300,
    }
    .create(&store, &args.geometry)?;

    let mut payload = created.json("command");
    payload["title"] = json!(title);
    payload["command"] = json!(command);
    payload["result_format"] = json!(args.result.as_str());
    payload["accept_selection"] = json!(args.accept_selection);
    payload["cron"] = json!(cron);
    payload["run_without_window"] = json!(args.no_window);
    payload["program"] = json!(inspection.as_ref().map(|i| i.program_path.clone()));
    payload["warnings"] = json!(warnings);

    format.emit(payload, || {
        created.report(format, "command");
        println!("  title:     {title}");
        println!("  command:   {command}");
        println!("  output:    {}", args.result.as_str());
        if let Some(cron) = &cron {
            println!("  schedule:  {cron}");
        }
        if args.accept_selection {
            println!("  selection: offered on the selected-text hotkey");
        }
        if args.no_window {
            println!("  window:    hidden unless the command fails");
        }
        for warning in &warnings {
            println!("note: {warning}");
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_working_directory_is_pinned_to_an_absolute_one() {
        assert_eq!(resolve_dir(None).unwrap(), "");
        assert_eq!(resolve_dir(Some("   ")).unwrap(), "");

        let here = std::env::current_dir().unwrap();
        assert_eq!(
            resolve_dir(Some(".")).unwrap(),
            here.to_string_lossy().trim_end_matches('\\')
        );
        assert!(Path::new(&resolve_dir(Some("src")).unwrap()).is_absolute());

        let absolute = here.to_string_lossy().into_owned();
        assert_eq!(resolve_dir(Some(&absolute)).unwrap(), absolute);
    }

    #[test]
    fn env_pairs_become_newline_separated_lines() {
        let env = parse_env(&["A=1".into(), "B=two words".into()]).unwrap();
        assert_eq!(env, "A=1\nB=two words");
    }

    #[test]
    fn an_env_value_may_contain_equals_signs() {
        assert_eq!(parse_env(&["URL=a=b".into()]).unwrap(), "URL=a=b");
    }

    #[test]
    fn env_without_an_equals_sign_is_rejected() {
        assert!(parse_env(&["JUST_A_NAME".into()]).is_err());
    }

    #[test]
    fn env_with_an_empty_name_is_rejected() {
        assert!(parse_env(&["=value".into()]).is_err());
    }

    #[test]
    fn no_env_pairs_produce_an_empty_string() {
        assert_eq!(parse_env(&[]).unwrap(), "");
    }

    #[test]
    fn padding_is_limited_to_the_sliders_range() {
        assert_eq!(parse_padding("64"), Ok(64));
        assert_eq!(parse_padding("0"), Ok(0));
        assert!(parse_padding("65").is_err());
        assert!(parse_padding("-1").is_err());
    }

    #[test]
    fn only_text_and_markdown_can_stream() {
        assert!(ResultFormat::Text.supports_streaming());
        assert!(ResultFormat::Markdown.supports_streaming());
        assert!(!ResultFormat::Html.supports_streaming());
        assert!(!ResultFormat::Svg.supports_streaming());
        assert!(!ResultFormat::Source.supports_streaming());
    }
}
