//! Turning a command line typed by a person into the string a command sticker stores.
//!
//! A command sticker does **not** run its command through a shell. The stored string is split with
//! Windows argv rules and the first token is looked up on `PATH`, so `ls | wc -l`, `echo $HOME`
//! and `a && b` all mean nothing to it. That is the single most common way to end up with a
//! sticker that quietly does the wrong thing, so this module does two things about it:
//!
//! * [`wrap_in_shell`] builds the `pwsh -Command …` (or `sh -c …`) form for people who *want* a
//!   shell.
//! * [`validate`] refuses to store a command whose program is not on `PATH`, and points out shell
//!   syntax that is about to be taken literally.

use anyhow::{Context as _, bail};

use crate::model::content::SELECTION_PLACEHOLDER;

/// Characters that mean something to a shell and nothing at all to a command sticker.
const SHELL_METACHARACTERS: [&str; 7] = ["|", "&&", "||", ">", "<", "$(", "`"];

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

/// What we could work out about a command before storing it.
#[derive(Debug)]
pub struct Inspection {
    /// The resolved path of the program, for the "this is what will actually run" report.
    pub program_path: String,
    /// Shell syntax that is going to be passed through as literal text.
    pub literal_shell_syntax: Vec<String>,
    /// Whether any argument carries the selection placeholder.
    pub uses_selection_placeholder: bool,
}

/// Check that a command sticker built from this string can actually run.
///
/// Returns an error for the things that are certainly broken, and reports the merely suspicious
/// ones in [`Inspection`] so the caller can warn without refusing.
pub fn validate(command: &str) -> anyhow::Result<Inspection> {
    let command = command.trim();
    if command.is_empty() {
        bail!("the command is empty");
    }

    let mut tokens = winsplit::split(command);
    if tokens.is_empty() {
        bail!("the command is empty once quotes are resolved");
    }

    let program = tokens.remove(0);
    if program.contains(SELECTION_PLACEHOLDER) {
        bail!(
            "{SELECTION_PLACEHOLDER} is only substituted in arguments, never in the program name \
             ('{program}'). Put the placeholder in an argument, or read the selection from the \
             RUSTICKERS_SELECTION environment variable."
        );
    }

    let program_path = which::which(&program)
        .with_context(|| {
            format!(
                "'{program}' is not on PATH. Command stickers run the program directly rather \
                 than through a shell — pass --shell to run this through the system shell instead."
            )
        })?
        .to_string_lossy()
        .into_owned();

    let literal_shell_syntax = SHELL_METACHARACTERS
        .iter()
        .filter(|meta| tokens.iter().any(|token| token.contains(**meta)))
        .map(|meta| (*meta).to_owned())
        .collect();

    Ok(Inspection {
        program_path,
        literal_shell_syntax,
        uses_selection_placeholder: tokens
            .iter()
            .any(|token| token.contains(SELECTION_PLACEHOLDER)),
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

    /// Split a wrapped command the same way the sticker runtime will.
    fn resplit(wrapped: &str) -> Vec<String> {
        winsplit::split(wrapped)
    }

    /// The shell's own flags come first; everything the user typed is the final argument.
    fn payload(wrapped: &str) -> String {
        resplit(wrapped).pop().expect("a wrapped command has parts")
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
        assert_eq!(
            payload(&wrap_in_shell("git status | findstr modified")),
            "git status | findstr modified"
        );
    }

    #[test]
    fn embedded_quotes_round_trip() {
        assert_eq!(
            payload(&wrap_in_shell(r#"git log --format="%h %s""#)),
            r#"git log --format="%h %s""#
        );
    }

    #[test]
    fn trailing_backslashes_round_trip() {
        assert_eq!(payload(&wrap_in_shell(r"dir C:\temp\")), r"dir C:\temp\");
    }

    #[test]
    fn backslashes_before_a_quote_round_trip() {
        assert_eq!(payload(&wrap_in_shell(r#"echo a\"b"#)), r#"echo a\"b"#);
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
        assert!(validate("   ").is_err());
    }

    #[test]
    fn a_program_that_is_not_installed_is_rejected() {
        let err = validate("definitely-not-a-real-program-xyz --flag").unwrap_err();
        assert!(format!("{err:#}").contains("not on PATH"));
    }

    #[test]
    fn a_placeholder_in_the_program_position_is_rejected() {
        let err = validate("{{RUSTICKERS_SELECTION}} --flag").unwrap_err();
        assert!(format!("{err:#}").contains("only substituted in arguments"));
    }

    #[test]
    fn shell_syntax_in_a_bare_command_is_reported() {
        // `cargo` is present wherever these tests run.
        let inspection = validate("cargo --version | findstr x").unwrap();
        assert!(inspection.literal_shell_syntax.contains(&"|".to_string()));
    }

    #[test]
    fn a_placeholder_argument_is_detected() {
        let inspection = validate("cargo {{RUSTICKERS_SELECTION}}").unwrap();
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
