//! Argument-vector substitution for `rst cp ls -- <program> <args>...`.
//!
//! Rules ([`build`]):
//! - The first token is the fixed executable. `{{txt}}` there is rejected so a malicious clipboard
//!   cannot redirect the user to a different program.
//! - Every `{{txt}}` in subsequent arguments is replaced with the exact selected text, without
//!   recursive substitution.
//! - When no argument contains the placeholder, the selected text is appended as one terminator
//!   argument so it lands on argv unaltered.
//! - Empty argv (just `--`) is rejected at the parser level; this module always has at least one
//!   program token.
//! - NUL bytes are rejected because the platform argv cannot carry them. The check runs after
//!   substitution so the selected text is also validated.

use anyhow::{Result, anyhow, bail};

/// The placeholder token used in command templates.
pub const PLACEHOLDER: &str = "{{txt}}";

/// Return the substituted argv ready for `std::process::Command`.
///
/// `template` is the post-`--` argument list; `selected` is the clipboard text to inject.
pub fn build(template: &[String], selected: &str) -> Result<Vec<String>> {
    validate_template(template)?;
    if selected.contains('\0') {
        bail!("selection contains a NUL byte, which cannot be passed as an argument");
    }
    let program = &template[0];
    let mut argv: Vec<String> = Vec::with_capacity(template.len() + 1);
    argv.push(program.clone());

    let mut any_placeholder = false;
    for token in &template[1..] {
        let replaced = if token.contains(PLACEHOLDER) {
            any_placeholder = true;
            token.replace(PLACEHOLDER, selected)
        } else {
            token.clone()
        };
        if replaced.contains('\0') {
            bail!("argument contains a NUL byte after substitution");
        }
        argv.push(replaced);
    }

    if !any_placeholder {
        argv.push(selected.to_owned());
    }

    #[cfg(windows)]
    validate_windows_size(&argv)?;
    Ok(argv)
}

/// Validate that `template` is a usable argument list, without doing substitution. Used by the CLI
/// to give a fast error before opening the chooser.
pub fn validate_template(template: &[String]) -> Result<()> {
    if template.is_empty() {
        bail!("cp ls requires a program after `--`. Usage: rst cp ls -- <program> <args>...");
    }
    let program = &template[0];
    if program.is_empty() {
        bail!("program name must not be empty");
    }
    if template.iter().any(|token| token.contains('\0')) {
        bail!("command template contains a NUL byte, which cannot be passed to a program");
    }
    if program.contains(PLACEHOLDER) {
        bail!("`{PLACEHOLDER}` is not allowed in the program name; pass it as an argument");
    }
    Ok(())
}

// Conservative bound for ordinary Windows Command arguments, including separators, quotes,
// backslash/quote escaping, surrogate pairs, and the terminating NUL. No command is rendered.
// Batch-file launchers may impose tighter limits; this is not a guarantee of successful launch.
#[cfg(any(windows, test))]
fn validate_windows_size(argv: &[String]) -> Result<()> {
    const LIMIT: usize = 32_760;
    let mut units = 1usize; // terminating NUL
    for (index, token) in argv.iter().enumerate() {
        units = units
            .saturating_add(usize::from(index != 0))
            .saturating_add(2);
        let mut backslashes = 0usize;
        for unit in token.encode_utf16() {
            units = units.saturating_add(1);
            if unit == b'\\' as u16 {
                backslashes += 1;
            } else {
                if unit == b'"' as u16 {
                    units = units.saturating_add(backslashes).saturating_add(1);
                }
                backslashes = 0;
            }
        }
        units = units.saturating_add(backslashes); // escaping before closing quote
        if units > LIMIT {
            bail!(
                "resulting Windows command exceeds the safe UTF-16 command-line limit; use a shorter clip or pipe `rst cp show --id <id>` to a program that accepts stdin"
            );
        }
    }
    Ok(())
}
/// The actual child argv that will be executed, as opposed to the raw template. Used to report
/// what is about to run for diagnostics without printing any clipboard text.
pub fn argv_for_display(argv: &[String]) -> Vec<String> {
    argv.iter()
        .map(|token| {
            if token.is_empty() {
                // An empty arg can collide with option parsing in the child. Mark it for display
                // so ambiguities are caught.
                "\u{201C}\u{201D}".to_owned()
            } else if token
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '"' | '\''))
            {
                format!("\u{201C}{token}\u{201D}")
            } else {
                token.clone()
            }
        })
        .collect()
}

pub fn program_not_found(program: &str) -> anyhow::Error {
    anyhow!("program not found on PATH: {program}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn empty_template_is_rejected() {
        let err = build(&[], "value").unwrap_err();
        assert!(format!("{err:#}").contains("requires a program"));
    }

    #[test]
    fn placeholder_in_program_is_rejected() {
        let err = build(&args(&["{{txt}}", "arg"]), "value").unwrap_err();
        assert!(format!("{err:#}").contains("not allowed in the program"));
    }

    #[test]
    fn replaces_every_placeholder_in_arguments() {
        let out = build(&args(&["rg", "--", "{{txt}}"]), "needle").unwrap();
        assert_eq!(out, vec!["rg", "--", "needle"]);
    }

    #[test]
    fn replaces_multiple_placeholders_in_one_argument() {
        let out = build(&args(&["sh", "-c", "echo {{txt}} | {{txt}}"]), "abc").unwrap();
        assert_eq!(out, vec!["sh", "-c", "echo abc | abc"]);
    }

    #[test]
    fn appends_when_no_placeholder_is_present() {
        let out = build(&args(&["cargo", "test"]), "needle").unwrap();
        assert_eq!(out, vec!["cargo", "test", "needle"]);
    }

    #[test]
    fn does_not_recurse_into_selected_text() {
        let out = build(&args(&["echo", "{{txt}}"]), "{{txt}}").unwrap();
        assert_eq!(out, vec!["echo", "{{txt}}"]);
    }

    #[test]
    fn nul_bytes_in_selection_are_rejected() {
        let err = build(&args(&["echo"]), "a\0b").unwrap_err();
        assert!(format!("{err:#}").contains("NUL"));
    }

    #[test]
    fn argv_display_marks_whitespace_arguments() {
        let display = argv_for_display(&args(&["echo", "hello world", "ok"]));
        assert_eq!(display[0], "echo");
        assert_eq!(display[1], "\u{201C}hello world\u{201D}");
        assert_eq!(display[2], "ok");
    }

    #[test]
    fn validation_steps_match_run_steps() {
        validate_template(&args(&["prog", "arg"])).unwrap();
        assert!(validate_template(&args(&["{{txt}}"])).is_err());
    }
}

#[cfg(test)]
mod validation_regressions {
    use super::*;

    #[test]
    fn invalid_executable_and_template_nul_fail_before_selection() {
        for template in [
            vec![String::new()],
            vec!["bad\0program".into()],
            vec!["program".into(), "bad\0argument".into()],
        ] {
            assert!(validate_template(&template).is_err());
            assert!(build(&template, "ordinary clip").is_err());
        }
    }

    #[test]
    fn windows_limit_counts_surrogates_escaping_and_combined_arguments() {
        // 7 units of overhead: "p" (3), separator (1), argument quotes (2), NUL (1).
        let largest_ascii_argument = 32_760 - 7;
        assert!(validate_windows_size(&["p".into(), "a".repeat(largest_ascii_argument)]).is_ok());
        assert!(
            validate_windows_size(&["p".into(), "a".repeat(largest_ascii_argument + 1)]).is_err()
        );
        assert!(validate_windows_size(&["p".into(), "😀".repeat(16_378)]).is_err());
        assert!(
            validate_windows_size(&["p".into(), "a".repeat(20_000), "b".repeat(13_000)]).is_err()
        );
        assert!(validate_windows_size(&["p".into(), "\\\"".repeat(8_190)]).is_err());
        assert!(validate_windows_size(&["p".into(), "\\".repeat(16_378)]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn substituted_command_limit_is_checked_without_echoing_secret() {
        let secret = format!("private-marker{}", "a".repeat(32_760));
        let error = build(&["program".into(), PLACEHOLDER.into()], &secret).unwrap_err();
        assert!(error.to_string().contains("UTF-16"));
        assert!(!error.to_string().contains("private-marker"));
    }
}
