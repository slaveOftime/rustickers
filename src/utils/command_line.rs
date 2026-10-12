//! The stored command-line grammar shared by CLI creation and desktop execution.
//!
//! Windows-style double quotes/backslashes, on every platform. Single quotes are literal data.
//! Unlike winsplit, explicit empty arguments survive and malformed quoting is an error.

pub fn split(command: &str) -> Result<Vec<String>, String> {
    if command.contains('\0') {
        return Err("Command contains a NUL byte".to_owned());
    }
    let mut chars = command.chars().peekable();
    let mut args = Vec::new();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut token = String::new();
        let mut quoted = false;
        while let Some(&c) = chars.peek() {
            if c.is_ascii_whitespace() && !quoted {
                break;
            }
            if c == '\\' {
                let mut slashes = 0;
                while chars.peek() == Some(&'\\') {
                    chars.next();
                    slashes += 1;
                }
                if chars.peek() == Some(&'"') {
                    token.extend(std::iter::repeat_n('\\', slashes / 2));
                    chars.next();
                    if slashes % 2 == 1 {
                        token.push('"');
                    } else if quoted && chars.peek() == Some(&'"') {
                        chars.next();
                        token.push('"');
                    } else {
                        quoted = !quoted;
                    }
                } else {
                    token.extend(std::iter::repeat_n('\\', slashes));
                }
            } else if c == '"' {
                chars.next();
                if quoted && chars.peek() == Some(&'"') {
                    chars.next();
                    token.push('"');
                } else {
                    quoted = !quoted;
                }
            } else {
                token.push(c);
                chars.next();
            }
        }
        if quoted {
            return Err("Command has an unterminated double quote".to_owned());
        }
        args.push(token);
    }
    Ok(args)
}

/// Desktop editor environment blocks retain exact values; surrounding key whitespace is UI syntax.
pub fn parse_environments(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| match line.split_once('=') {
            Some((key, value)) => (key.trim().to_owned(), value.to_owned()),
            None => (line.trim().to_owned(), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_empty_arguments_survive() {
        assert_eq!(
            split("program \"\" x \"\"").unwrap(),
            ["program", "", "x", ""]
        );
    }
    #[test]
    fn quoting_and_escaping_are_windows_style() {
        assert_eq!(
            split(r#"program "two words" a\"b C:\path\ 'literal'"#).unwrap(),
            ["program", "two words", "a\"b", "C:\\path\\", "'literal'"]
        );
        assert_eq!(
            split("program \"snow 雪\nline\" tail").unwrap(),
            ["program", "snow 雪\nline", "tail"]
        );
    }
    #[test]
    fn environment_values_preserve_whitespace_and_equals() {
        assert_eq!(
            parse_environments(" A =  data  \nEMPTY=\nURL=a=b\nFLAG\n"),
            [
                ("A".into(), "  data  ".into()),
                ("EMPTY".into(), "".into()),
                ("URL".into(), "a=b".into()),
                ("FLAG".into(), "".into())
            ]
        );
    }
    #[test]
    fn malformed_input_is_rejected() {
        assert!(split("program \"oops").is_err());
        assert!(split("program bad\0data").is_err());
        assert!(split(" \t\n").unwrap().is_empty());
    }
}
