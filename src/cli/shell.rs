//! Turning arguments typed by a person into the string a command sticker stores, and checking it
//! before we store it.
//!
//! A command sticker does **not** run its command through a shell. The stored string is split with
//! Windows argv rules and the first token is looked up on `PATH`, so `ls | wc -l`, `echo $HOME`
//! and `a && b` all mean nothing to it. That is the single most common way to end up with a
//! sticker that quietly does the wrong thing, so this module does three things about it:
//!
//! * [`encode_argv`] stores the arguments exactly as they were typed, so what runs is what was
//!   written, and refuses what the storage format cannot carry.
//! * [`wrap_in_shell`] builds the `pwsh -Command …` (or `sh -c …`) form for people who *want* a
//!   shell.
//! * [`inspect`] refuses a command that is structurally broken, and [`Inspection::resolve_program_in`]
//!   reports the program that will actually run, which is the one step a caller may skip.

use anyhow::{Context as _, bail};

use crate::model::content::SELECTION_PLACEHOLDER;
use crate::utils::command_line;

/// Characters that mean something to a shell and nothing at all to a command sticker.
const SHELL_METACHARACTERS: [&str; 7] = ["|", "&&", "||", ">", "<", "$(", "`"];

/// The longest command line Windows will hand to a process, in UTF-16 units. The stored string is
/// later rebuilt into one, so a longer command is only discovered broken once the sticker runs.
const WINDOWS_COMMAND_LIMIT: usize = 32_760;

/// Encode an argument vector as the command string a sticker stores.
///
/// The result splits back into exactly `argv` under [`command_line::split`], the parser the
/// desktop runs with, so quoting, embedded quotes, backslashes, newlines, empty arguments and
/// non-ASCII all survive unchanged. Two things cannot be stored at all: a `NUL` byte, which no
/// platform can pass in an argument, and a command line longer than Windows can build.
pub fn encode_argv(argv: &[String]) -> anyhow::Result<String> {
    let Some((program, _)) = argv.split_first() else {
        bail!("there is no program to run; pass it after `--`: rst command -- <program> <args>...");
    };
    if program.is_empty() {
        bail!("the program is empty; pass the program to run after `--`");
    }

    for token in argv {
        if token.contains('\0') {
            bail!("a command argument contains a NUL byte, which cannot be passed to a program");
        }
    }

    let command = argv
        .iter()
        .map(|token| {
            if needs_quotes(token) {
                quote(token)
            } else {
                token.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let units = command.encode_utf16().count();
    if units > WINDOWS_COMMAND_LIMIT {
        bail!(
            "the encoded command is {units} UTF-16 units, over the {WINDOWS_COMMAND_LIMIT} \
             Windows can hand to a program; shorten the arguments or read them from a file"
        );
    }

    debug_assert_eq!(
        command_line::split(&command).as_deref(),
        Ok(argv),
        "encoding an argument vector must survive the parser the runtime uses"
    );

    Ok(command)
}

/// Whether a token needs quoting to survive the split intact.
///
/// Only whitespace and a double quote change how the parser cuts the string, and an empty token
/// needs quotes to be an argument at all. Anything else is stored verbatim, which keeps the stored
/// command readable in the sticker's own settings.
fn needs_quotes(token: &str) -> bool {
    token.is_empty() || token.contains(char::is_whitespace) || token.contains('"')
}

/// Quote a string so that splitting it back out yields exactly one argument.
///
/// This is the Windows convention the sticker runtime parses with: a backslash is only an escape
/// when it runs into a quote, so runs of backslashes are doubled in that position and left alone
/// everywhere else.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');

    let mut backslashes = 0usize;
    for c in s.chars() {
        match c {
            '\\' => {
                backslashes += 1;
                out.push('\\');
            }
            '"' => {
                // Double the backslashes that precede the quote, then escape the quote itself.
                for _ in 0..=backslashes {
                    out.push('\\');
                }
                backslashes = 0;
                out.push('"');
            }
            _ => {
                // A backslash only escapes when the character right after it is a quote, so a run
                // that ends here is stored as the literal text it is.
                backslashes = 0;
                out.push(c);
            }
        }
    }

    // The closing quote is preceded by the same run, so it needs the same doubling.
    for _ in 0..backslashes {
        out.push('\\');
    }
    out.push('"');
    out
}

/// Build the command string that runs `command` through the platform shell.
///
/// On Windows this is PowerShell rather than `cmd`, and not by preference. The stored string is
/// later split and handed to `std::process::Command`, which re-escapes an embedded `"` as `\"`.
/// PowerShell parses its command line by the same rules and gets the quote back; `cmd.exe` does
/// not, and silently tears a command like `git log --format="%h %s"` into the wrong arguments.
/// `pwsh` is preferred over `powershell` when it is installed because it starts faster and
/// supports `&&` and `||`.
pub fn wrap_in_shell(command: &str) -> String {
    if cfg!(windows) {
        let shell = if which::which("pwsh").is_ok() {
            "pwsh"
        } else {
            "powershell"
        };
        format!("{shell} -NoProfile -Command {}", quote(command))
    } else {
        format!("sh -c {}", quote(command))
    }
}

/// What we could work out about a command before storing it.
#[derive(Debug)]
pub struct Inspection {
    /// The token the runtime will treat as the executable.
    pub program: String,
    /// The resolved path of the program, once [`Inspection::resolve_program_in`] has run. `None`
    /// means the lookup was skipped, not that the program is missing.
    pub program_path: Option<String>,
    /// Shell syntax that is going to be passed through as literal text.
    pub literal_shell_syntax: Vec<String>,
    /// Whether any argument carries the selection placeholder.
    pub uses_selection_placeholder: bool,
}

impl Inspection {
    /// Fill in [`Self::program_path`] by looking the program up relative to its working directory and `PATH`.
    ///
    /// This is the one validation step a caller may skip (`--allow-missing-program`), because a
    /// sticker may legitimately be created before the program is installed. Every structural
    /// check has already happened by the time this is called.
    #[cfg(test)]
    pub fn resolve_program(self) -> anyhow::Result<Self> {
        self.resolve_program_in(&std::env::current_dir()?)
    }

    pub fn resolve_program_in(self, directory: &std::path::Path) -> anyhow::Result<Self> {
        let program_path = which::which_in(&self.program, std::env::var_os("PATH"), directory)
            .with_context(|| {
                format!(
                    "'{}' is not on PATH. Command stickers run the program directly rather than \
                 through a shell — use --shell \"…\" to run through the system shell, or \
                 --allow-missing-program to store it anyway.",
                    self.program
                )
            })?;
        Ok(Self {
            program_path: Some(program_path.to_string_lossy().into_owned()),
            ..self
        })
    }

    /// What to print in a report: the resolved path when we looked, the typed name otherwise.
    pub fn program_display(&self) -> String {
        self.program_path.clone().unwrap_or(self.program.clone())
    }
}

/// Check the shape of a command string, without touching `PATH`.
///
/// This is the part that cannot be skipped: an empty command, a command that is only quotes, and
/// the selection placeholder in the program position are all broken no matter what is installed.
pub fn inspect(command: &str) -> anyhow::Result<Inspection> {
    let command = command.trim();
    if command.is_empty() {
        bail!("the command is empty");
    }

    let mut tokens = command_line::split(command).map_err(anyhow::Error::msg)?;
    if tokens.is_empty() {
        bail!("the command is empty once quotes are resolved");
    }

    let program = tokens.remove(0);
    anyhow::ensure!(!program.trim().is_empty(), "the program is empty");
    if program.contains(SELECTION_PLACEHOLDER) {
        bail!(
            "{SELECTION_PLACEHOLDER} is only substituted in arguments, never in the program name \
             ('{program}'). Put the placeholder in an argument, or read the selection from the \
             RUSTICKERS_SELECTION environment variable."
        );
    }

    let literal_shell_syntax = SHELL_METACHARACTERS
        .iter()
        .filter(|meta| tokens.iter().any(|token| token.contains(**meta)))
        .map(|meta| (*meta).to_owned())
        .collect();

    let uses_selection_placeholder = tokens
        .iter()
        .any(|token| token.contains(SELECTION_PLACEHOLDER));

    Ok(Inspection {
        program,
        program_path: None,
        literal_shell_syntax,
        uses_selection_placeholder,
    })
}

/// Reject a cron expression the sticker runtime would refuse at run time.
///
/// The runtime uses Quartz-style expressions, which start with a seconds field. A five field
/// expression copied from crontab parses as something else entirely, or not at all, and the
/// failure only surfaces once the sticker is opened — so it is caught here instead.
pub fn validate_cron(expr: &str) -> anyhow::Result<String> {
    use std::str::FromStr as _;

    let expr = expr.trim();
    if expr.is_empty() {
        bail!("the cron expression is empty");
    }

    let fields = expr.split_whitespace().count();
    if fields < 6 {
        bail!(
            "'{expr}' has {fields} fields; Rustickers uses Quartz-style expressions with 6 or 7 \
             (second minute hour day-of-month month day-of-week [year]). A crontab expression \
             like '*/5 * * * *' becomes '0 */5 * * * *'."
        );
    }

    cron::Schedule::from_str(expr)
        .with_context(|| format!("'{expr}' is not a valid cron expression"))?;

    Ok(expr.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Split a stored command the same way the sticker runtime will.
    fn resplit(stored: &str) -> Vec<String> {
        command_line::split(stored).expect("a stored command we encoded is well formed")
    }

    /// Encoding must reproduce the argument vector exactly, token for token.
    fn round_trips(argv: &[&str]) {
        let argv = strings(argv);
        let stored = encode_argv(&argv).expect("encodable");
        assert_eq!(resplit(&stored), argv, "stored as {stored:?}");
    }

    #[test]
    fn ordinary_arguments_are_stored_unquoted() {
        assert_eq!(
            encode_argv(&strings(&["git", "status"])).unwrap(),
            "git status"
        );
    }

    #[test]
    fn a_program_with_no_arguments_round_trips() {
        round_trips(&["cargo"]);
    }

    #[test]
    fn arguments_with_spaces_round_trip_as_one_token() {
        round_trips(&["git", "commit", "-m", "two words"]);
    }

    #[test]
    fn embedded_quotes_round_trip() {
        round_trips(&["git", "log", r#"--format="%h %s""#]);
        round_trips(&["cmd", r#"a"b"#]);
        round_trips(&["cmd", r#"""#]);
    }

    #[test]
    fn backslashes_round_trip_in_every_position() {
        round_trips(&["cmd", r"C:\temp\"]);
        round_trips(&["cmd", r"a\b", r"c\"]);
        round_trips(&["cmd", r"\"]);
        round_trips(&["cmd", r"\\"]);
        round_trips(&["cmd", r#"a\"b"#]);
        round_trips(&["cmd", r#"\""#]);
        round_trips(&["cmd", r#"a\"b"#, r#"--x=\"y\"""#]);
    }

    #[test]
    fn newlines_and_tabs_round_trip_inside_one_argument() {
        round_trips(&["cmd", "line1\nline2"]);
        round_trips(&["cmd", "a\tb"]);
        round_trips(&["cmd", " "]);
    }

    #[test]
    fn unicode_round_trips() {
        round_trips(&["cmd", "日本語", "😀 emoji", "réf\u{301}"]);
        round_trips(&["cmd", "\u{200b}zero width"]);
    }

    #[test]
    fn shell_syntax_is_stored_as_literal_text_not_interpreted() {
        let stored = encode_argv(&strings(&["wc", "-l", "|", "more", "$HOME", ">"])).unwrap();
        assert_eq!(resplit(&stored), ["wc", "-l", "|", "more", "$HOME", ">"]);
    }

    #[test]
    fn an_empty_argument_vector_is_rejected() {
        assert!(encode_argv(&[]).is_err());
    }

    #[test]
    fn an_empty_argument_survives_as_an_argument() {
        round_trips(&["cmd", "", "x", ""]);
    }

    #[test]
    fn an_empty_program_is_rejected() {
        let err = encode_argv(&strings(&["", "x"])).unwrap_err();
        assert!(format!("{err:#}").contains("program is empty"), "{err:#}");
    }

    #[test]
    fn malformed_quoting_is_rejected_rather_than_stored() {
        assert!(inspect("cmd \"unterminated").is_err());
    }

    #[test]
    fn a_nul_byte_is_rejected() {
        assert!(encode_argv(&strings(&["cmd", "a\0b"])).is_err());
        assert!(encode_argv(&strings(&["cmd\0"])).is_err());
    }

    #[test]
    fn an_over_long_command_is_refused_before_it_can_fail_at_run_time() {
        let argv = vec!["cmd".to_owned(), "x".repeat(WINDOWS_COMMAND_LIMIT)];
        let err = encode_argv(&argv).unwrap_err();
        assert!(format!("{err:#}").contains("UTF-16"), "{err:#}");
    }

    #[test]
    fn a_shell_wrapped_command_survives_as_a_single_argument() {
        let wrapped = wrap_in_shell("echo hello world");
        let parts = resplit(&wrapped);
        assert_eq!(parts.len(), if cfg!(windows) { 4 } else { 3 });
        assert_eq!(parts.last().unwrap(), "echo hello world");
    }

    #[test]
    fn shell_metacharacters_stay_inside_the_single_argument() {
        let wrapped = wrap_in_shell("git status | findstr modified");
        assert_eq!(
            resplit(&wrapped).last().unwrap(),
            "git status | findstr modified"
        );
    }

    #[test]
    fn the_shell_wrapper_names_the_platform_shell() {
        let parts = resplit(&wrap_in_shell("whoami"));
        if cfg!(windows) {
            // cmd.exe is deliberately not used: it cannot recover an embedded quote.
            assert!(
                matches!(parts[0].as_str(), "pwsh" | "powershell"),
                "{parts:?}"
            );
            assert_eq!(&parts[1..3], ["-NoProfile", "-Command"]);
        } else {
            assert_eq!(&parts[..2], ["sh", "-c"]);
        }
    }

    #[test]
    fn an_empty_command_is_rejected() {
        assert!(inspect("   ").is_err());
        assert!(inspect("\"  \"").is_err());
    }

    #[test]
    fn a_placeholder_in_the_program_position_is_rejected() {
        let err = inspect("{{RUSTICKERS_SELECTION}} --flag").unwrap_err();
        assert!(
            format!("{err:#}").contains("only substituted in arguments"),
            "{err:#}"
        );
    }

    #[test]
    fn structural_inspection_needs_nothing_installed() {
        // `--allow-missing-program` skips the lookup, never these checks, so `inspect` must not
        // care whether the program exists.
        let inspection = inspect("definitely-not-a-real-program-xyz --flag").unwrap();
        assert_eq!(inspection.program, "definitely-not-a-real-program-xyz");
        assert_eq!(inspection.program_path, None);
        assert!(!inspection.uses_selection_placeholder);
        assert_eq!(
            inspection.program_display(),
            "definitely-not-a-real-program-xyz"
        );
    }

    #[test]
    fn a_program_that_is_not_installed_is_rejected_by_the_lookup() {
        let inspection = inspect("definitely-not-a-real-program-xyz --flag").unwrap();
        let err = inspection.resolve_program().unwrap_err();
        assert!(format!("{err:#}").contains("not on PATH"), "{err:#}");
    }

    #[test]
    fn a_program_that_is_installed_resolves_to_a_path() {
        // `cargo` is present wherever these tests run.
        let inspection = inspect("cargo --version")
            .unwrap()
            .resolve_program()
            .unwrap();
        assert!(
            inspection
                .program_path
                .as_deref()
                .is_some_and(|p| !p.is_empty())
        );
        assert!(inspection.program_display().contains("cargo"));
    }

    #[test]
    fn shell_syntax_in_a_bare_command_is_reported() {
        let inspection = inspect("cargo --version | findstr x").unwrap();
        assert!(inspection.literal_shell_syntax.contains(&"|".to_string()));
    }

    #[test]
    fn a_placeholder_argument_is_detected() {
        let inspection = inspect("cargo {{RUSTICKERS_SELECTION}}").unwrap();
        assert!(inspection.uses_selection_placeholder);
    }

    #[test]
    fn a_five_field_cron_expression_is_rejected_with_the_six_field_fix() {
        let err = validate_cron("*/5 * * * *").unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("6 or 7"), "{message}");
        assert!(message.contains("0 */5 * * * *"), "{message}");
    }

    #[test]
    fn a_six_field_cron_expression_is_accepted_and_trimmed() {
        assert_eq!(validate_cron("  0 */5 * * * *  ").unwrap(), "0 */5 * * * *");
    }

    #[test]
    fn nonsense_in_a_six_field_expression_is_still_rejected() {
        assert!(validate_cron("0 0 0 0 0 nope").is_err());
    }
}
