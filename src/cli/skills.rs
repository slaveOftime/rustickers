//! A catalogue of worked examples.
//!
//! The `cmd` command can express a lot of different stickers, and the interesting combinations are
//! not obvious from a list of flags. A skill is one of those combinations, written down: what it
//! is for, which parts you are expected to change, and the exact `rusticker` invocation it stands
//! for.
//!
//! Skills expand to argument lists that are fed back through the ordinary parser, so a skill can
//! never drift away from the command it claims to run — `skill show` prints the same arguments
//! `skill run` executes.

use anyhow::bail;
use std::collections::BTreeMap;

/// A value the caller is expected to change, with the value used when they do not.
#[derive(Debug)]
pub struct Var {
    pub name: &'static str,
    pub description: &'static str,
    /// `None` marks the variable as required.
    pub default: Option<&'static str>,
}

#[derive(Debug)]
pub struct Skill {
    pub name: &'static str,
    /// One line, shown in the catalogue listing.
    pub summary: &'static str,
    /// The longer explanation: when to reach for it, and what it will do.
    pub detail: &'static str,
    pub vars: &'static [Var],
    /// `rusticker` arguments, with `${name}` standing in for a variable.
    pub template: &'static [&'static str],
}

pub const SKILLS: &[Skill] = &[
    Skill {
        name: "ask",
        summary: "Ask an AI CLI about the text you have selected, answered as markdown",
        detail: "\
Select text anywhere — a browser, an editor, a chat — press the selected-text hotkey and pick this
sticker. The selection is passed to your AI CLI as a single argument and the reply is rendered as
markdown, so headings, lists and code blocks come out formatted.

The sticker is created closed: it never needs a window of its own, it only exists to be offered
when you trigger the hotkey.",
        vars: &[
            Var {
                name: "tool",
                description: "the AI CLI and the flag that takes a prompt",
                default: Some("copilot -p"),
            },
            Var {
                name: "title",
                description: "how the sticker is labelled in the selection picker",
                default: Some("Ask about selection"),
            },
        ],
        template: &[
            "cmd",
            "${tool} {{RUSTICKERS_SELECTION}}",
            "--accept-selection",
            "--result",
            "markdown",
            "--title",
            "${title}",
            "--closed",
            "--width",
            "620",
            "--height",
            "460",
            "--color",
            "blue",
            "--padding",
            "14",
        ],
    },
    Skill {
        name: "translate",
        summary: "Translate the selected text into another language",
        detail: "\
The same shape as `ask`, with the prompt fixed and the target language pulled out as a variable.
Create one per language you care about and the selection picker becomes a language menu.",
        vars: &[
            Var {
                name: "language",
                description: "the language to translate into",
                default: Some("English"),
            },
            Var {
                name: "tool",
                description: "the AI CLI and the flag that takes a prompt",
                default: Some("copilot -p"),
            },
        ],
        template: &[
            "cmd",
            "${tool} \"Translate the following into ${language}. Reply with the translation and \
             nothing else.\" {{RUSTICKERS_SELECTION}}",
            "--accept-selection",
            "--result",
            "markdown",
            "--title",
            "Translate to ${language}",
            "--closed",
            "--width",
            "560",
            "--height",
            "360",
            "--color",
            "green",
            "--padding",
            "14",
        ],
    },
    Skill {
        name: "explain-code",
        summary: "Explain the selected code, rendered as markdown with syntax-highlighted blocks",
        detail: "\
Aimed at reading unfamiliar code: select a function, trigger the hotkey, get a walkthrough back in
the same place you were reading. Markdown output means the code blocks in the explanation stay
readable.",
        vars: &[Var {
            name: "tool",
            description: "the AI CLI and the flag that takes a prompt",
            default: Some("copilot -p"),
        }],
        template: &[
            "cmd",
            "${tool} \"Explain what this code does, step by step. Use markdown.\" \
             {{RUSTICKERS_SELECTION}}",
            "--accept-selection",
            "--result",
            "markdown",
            "--title",
            "Explain code",
            "--closed",
            "--width",
            "680",
            "--height",
            "520",
            "--color",
            "blue",
            "--padding",
            "14",
        ],
    },
    Skill {
        name: "web-search",
        summary: "Search the web for the selected text and open the results inside a sticker",
        detail: "\
Shows the other half of the selection contract. Instead of the {{RUSTICKERS_SELECTION}} argument
placeholder, this reads the RUSTICKERS_SELECTION environment variable, which is what you want when
the text has to be transformed — here, percent-encoded — before it can be used.

The command prints a URL and nothing else, and `--result source` treats that output as something
to open rather than something to display, so the search results render in the sticker.

Uses PowerShell, so it is Windows-only as written.",
        vars: &[Var {
            name: "engine",
            description: "search URL prefix that the encoded query is appended to",
            default: Some("https://duckduckgo.com/?q="),
        }],
        template: &[
            "cmd",
            "powershell -NoProfile -Command \"'${engine}' + \
             [uri]::EscapeDataString($env:RUSTICKERS_SELECTION)\"",
            "--accept-selection",
            "--result",
            "source",
            "--title",
            "Search the web",
            "--closed",
            "--width",
            "900",
            "--height",
            "700",
        ],
    },
    Skill {
        name: "to-clipboard",
        summary: "Transform the selected text and put the result straight on the clipboard",
        detail: "\
A sticker that never shows a window. `--no-window` runs the command hidden and throws the window
away when it succeeds; you only ever see it if something goes wrong, and then it appears with the
error. That makes it the right shape for anything whose result is a side effect rather than
something to read.

Change `transform` to any PowerShell pipeline: `ConvertTo-Json`, a regex replace, a formatter.
Uses PowerShell, so it is Windows-only as written.",
        vars: &[Var {
            name: "transform",
            description: "PowerShell pipeline applied to the selection",
            default: Some("ForEach-Object { $_.ToUpper() }"),
        }],
        template: &[
            "cmd",
            "powershell -NoProfile -Command \"$env:RUSTICKERS_SELECTION | ${transform} | \
             Set-Clipboard\"",
            "--accept-selection",
            "--no-window",
            "--title",
            "Selection to clipboard",
            "--closed",
        ],
    },
    Skill {
        name: "format-json",
        summary: "Pretty-print the selected JSON",
        detail: "\
A local transform with no network and no AI: select a blob of minified JSON, trigger the hotkey,
read it indented. Plain text output, because JSON is already the formatting.

Uses PowerShell, so it is Windows-only as written.",
        vars: &[Var {
            name: "depth",
            description: "how many levels deep to expand nested objects",
            default: Some("20"),
        }],
        template: &[
            "cmd",
            "powershell -NoProfile -Command \"$env:RUSTICKERS_SELECTION | ConvertFrom-Json | \
             ConvertTo-Json -Depth ${depth}\"",
            "--accept-selection",
            "--result",
            "text",
            "--title",
            "Format JSON",
            "--closed",
            "--width",
            "520",
            "--height",
            "480",
            "--padding",
            "10",
        ],
    },
    Skill {
        name: "watch",
        summary: "Re-run a command on a schedule and keep its latest output on screen",
        detail: "\
A always-on dashboard tile. The command runs on a cron schedule and `--stream` clears the previous
output and fills the sticker line by line as the new run produces it.

Schedules are Quartz-style and start with a seconds field, so every five minutes is
'0 */5 * * * *' rather than '*/5 * * * *'.

The schedule is not tied to the window. Close the sticker and it keeps ticking in the background,
so `rusticker result <id>` always returns the most recent run. Add --no-window to build a schedule
that never puts anything on screen at all, and watch the sticker list for the running indicator.",
        vars: &[
            Var {
                name: "command",
                description: "the command to run on each tick",
                default: Some("git status --short"),
            },
            Var {
                name: "cron",
                description: "6-field schedule: second minute hour day month weekday",
                default: Some("0 */1 * * * *"),
            },
            Var {
                name: "dir",
                description: "directory to run the command in",
                default: Some("."),
            },
        ],
        template: &[
            "cmd",
            "${command}",
            "--cron",
            "${cron}",
            "--run-now",
            "--stream",
            "--result",
            "text",
            "--dir",
            "${dir}",
            "--title",
            "watch: ${command}",
            "--width",
            "420",
            "--height",
            "320",
            "--padding",
            "10",
        ],
    },
    Skill {
        name: "html-report",
        summary: "Render a command's HTML output in an embedded browser view",
        detail: "\
For commands that already produce a styled document — a coverage report, a generated dashboard, a
rendered template. The output has to be a complete HTML document; it is loaded into a real browser
view, so CSS and JavaScript work and links open in your normal browser.",
        vars: &[
            Var {
                name: "command",
                description: "command that prints an HTML document to stdout",
                default: None,
            },
            Var {
                name: "title",
                description: "how the sticker is labelled",
                default: Some("HTML report"),
            },
        ],
        template: &[
            "cmd",
            "${command}",
            "--result",
            "html",
            "--title",
            "${title}",
            "--width",
            "860",
            "--height",
            "640",
        ],
    },
    Skill {
        name: "svg-chart",
        summary: "Draw a command's SVG output as a picture",
        detail: "\
For anything that can generate a diagram: a graph of your metrics, a rendered chart, a plotted
sparkline. The command has to print one complete SVG document to stdout — anything else renders
as nothing at all, because there is no error to show, just an image that will not decode.",
        vars: &[
            Var {
                name: "command",
                description: "command that prints an SVG document to stdout",
                default: None,
            },
            Var {
                name: "title",
                description: "how the sticker is labelled",
                default: Some("Chart"),
            },
        ],
        template: &[
            "cmd",
            "${command}",
            "--result",
            "svg",
            "--title",
            "${title}",
            "--width",
            "480",
            "--height",
            "320",
        ],
    },
    Skill {
        name: "note",
        summary: "Pin a markdown note to the desktop",
        detail: "\
Not every sticker runs something. This is the plain case: a markdown note that stays on screen and
is edited in place.

For anything longer than a line, skip the variable and pipe the document in instead:
`rusticker markdown --file notes.md`, or `--file -` to read standard input.",
        vars: &[Var {
            name: "text",
            description: "the markdown content",
            default: Some("# Notes\n\n- "),
        }],
        template: &["markdown", "--content", "${text}", "--color", "yellow"],
    },
];

pub fn find(name: &str) -> Option<&'static Skill> {
    SKILLS.iter().find(|skill| skill.name == name)
}

pub fn names() -> Vec<&'static str> {
    SKILLS.iter().map(|skill| skill.name).collect()
}

impl Skill {
    /// Resolve `overrides` against the declared defaults.
    ///
    /// An unknown name is an error rather than a silent no-op: a typo in `--var` would otherwise
    /// produce a sticker that looks right and quietly uses the default.
    pub fn resolve_vars(
        &self,
        overrides: &BTreeMap<String, String>,
    ) -> anyhow::Result<BTreeMap<String, String>> {
        if let Some(unknown) = overrides
            .keys()
            .find(|key| !self.vars.iter().any(|var| var.name == key.as_str()))
        {
            bail!(
                "'{unknown}' is not a variable of the '{}' skill. It takes: {}",
                self.name,
                self.var_names().join(", ")
            );
        }

        let mut resolved = BTreeMap::new();
        let mut missing = Vec::new();
        for var in self.vars {
            match overrides.get(var.name).map(String::as_str).or(var.default) {
                Some(value) => {
                    resolved.insert(var.name.to_owned(), value.to_owned());
                }
                None => missing.push(var.name),
            }
        }

        if !missing.is_empty() {
            bail!(
                "the '{}' skill needs {}. Pass {}.",
                self.name,
                missing.join(" and "),
                missing
                    .iter()
                    .map(|name| format!("--var {name}=..."))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }

        Ok(resolved)
    }

    pub fn var_names(&self) -> Vec<&'static str> {
        self.vars.iter().map(|var| var.name).collect()
    }

    /// Substitute the resolved variables into the template to get real CLI arguments.
    pub fn expand(&self, vars: &BTreeMap<String, String>) -> Vec<String> {
        self.template
            .iter()
            .map(|part| {
                let mut part = (*part).to_owned();
                for (name, value) in vars {
                    part = part.replace(&format!("${{{name}}}"), value);
                }
                part
            })
            .collect()
    }
}

/// Render an argument list as a command line someone can paste into a terminal.
pub fn command_line(args: &[String]) -> String {
    let quoted: Vec<String> = args
        .iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains([' ', '\t', '\n', '"']) {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.clone()
            }
        })
        .collect();
    format!("rusticker {}", quoted.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn every_skill_has_a_unique_name() {
        let mut names = names();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "skill names must be unique");
    }

    #[test]
    fn every_template_placeholder_has_a_declared_variable() {
        for skill in SKILLS {
            let expanded = skill.expand(
                &skill
                    .vars
                    .iter()
                    .map(|var| (var.name.to_owned(), "X".to_owned()))
                    .collect(),
            );
            for part in expanded {
                assert!(
                    !part.contains("${"),
                    "skill '{}' has an undeclared placeholder in {part:?}",
                    skill.name
                );
            }
        }
    }

    #[test]
    fn every_declared_variable_is_used_by_its_template() {
        for skill in SKILLS {
            for var in skill.vars {
                let needle = format!("${{{}}}", var.name);
                assert!(
                    skill.template.iter().any(|part| part.contains(&needle)),
                    "skill '{}' declares unused variable '{}'",
                    skill.name,
                    var.name
                );
            }
        }
    }

    #[test]
    fn defaults_are_used_when_nothing_is_passed() {
        let skill = find("translate").unwrap();
        let vars = skill.resolve_vars(&BTreeMap::new()).unwrap();
        assert_eq!(vars["language"], "English");
    }

    #[test]
    fn an_override_wins_over_the_default() {
        let skill = find("translate").unwrap();
        let vars = skill
            .resolve_vars(&overrides(&[("language", "Japanese")]))
            .unwrap();
        assert_eq!(vars["language"], "Japanese");
        assert!(
            skill
                .expand(&vars)
                .contains(&"Translate to Japanese".to_owned())
        );
    }

    #[test]
    fn an_unknown_variable_is_rejected() {
        let skill = find("translate").unwrap();
        let err = skill
            .resolve_vars(&overrides(&[("langauge", "Japanese")]))
            .unwrap_err();
        assert!(format!("{err:#}").contains("is not a variable"));
    }

    #[test]
    fn a_required_variable_must_be_supplied() {
        let skill = find("html-report").unwrap();
        let err = skill.resolve_vars(&BTreeMap::new()).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("--var command="), "{message}");
    }

    #[test]
    fn the_selection_placeholder_survives_expansion() {
        let skill = find("ask").unwrap();
        let vars = skill.resolve_vars(&BTreeMap::new()).unwrap();
        let expanded = skill.expand(&vars);
        assert!(
            expanded
                .iter()
                .any(|part| part.contains(crate::model::content::SELECTION_PLACEHOLDER))
        );
    }

    #[test]
    fn a_command_line_quotes_arguments_that_need_it() {
        let line = command_line(&["cmd".into(), "git status".into(), "--closed".into()]);
        assert_eq!(line, "rusticker cmd \"git status\" --closed");
    }

    #[test]
    fn every_skill_expands_to_a_known_subcommand() {
        for skill in SKILLS {
            let first = skill.template.first().copied().unwrap_or_default();
            assert!(
                ["cmd", "markdown", "view"].contains(&first),
                "skill '{}' expands to unknown subcommand '{first}'",
                skill.name
            );
        }
    }
}
