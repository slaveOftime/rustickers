//! A catalogue of worked examples, written as exact argv segments.
//!
//! A skill is a combination of flags that is worth writing down: what it is for, which parts
//! you are expected to change, and the exact `rst` invocation it stands for.
//!
//! Three rules keep a skill honest, and they are structural rather than aspirational:
//!
//! * Every template entry is **one complete argv segment**. There is no nested command string
//!   and no second layer of quoting: what `expand` produces is what the parser receives, and
//!   the expansion is fed back through the ordinary parser so a skill can never drift away from
//!   the command it claims to run.
//! * Substitution is **one pass**: a value inserted for `${name}` is data, never rescanned, so a
//!   value containing `${...}` or `{{...}}` cannot smuggle in another expansion.
//! * Variables reach **data positions only**. Executable names, prompts and shell scripts each
//!   have their own contract (see the catalogue); a variable is never spliced into the middle
//!   of a code string.
//!
//! Human display and machine consumption are deliberately separate: JSON replies carry the
//! canonical `argv` array; [`human_command_line`] renders a paste-ready line quoted for the
//! host's shell (PowerShell rules on Windows, POSIX rules elsewhere).

use anyhow::bail;
use std::collections::BTreeMap;

use crate::cli::shell;

type Validator = fn(&str) -> Result<(), String>;

/// A value the caller is expected to change, with the value used when they do not.
#[derive(Debug)]
pub struct Var {
    pub name: &'static str,
    pub description: &'static str,
    /// `None` marks the variable as required.
    pub default: Option<&'static str>,
    /// Optional check on the fully resolved value, run before anything is expanded or stored.
    pub validate: Option<Validator>,
}

#[derive(Debug)]
pub struct Skill {
    pub name: &'static str,
    /// One line, shown in the catalogue listing.
    pub summary: &'static str,
    /// The longer explanation: when to reach for it, what it will do, and which shell (if any)
    /// executes its script variable.
    pub detail: &'static str,
    pub vars: &'static [Var],
    /// `rst` arguments, one entry per argv segment, with `${name}` standing in for a variable.
    pub template: &'static [&'static str],
}

// ---------------------------------------------------------------------------
// Variable validators
// ---------------------------------------------------------------------------

/// One argv segment naming an executable. Spaces are allowed (`C:\Program Files\..\tool.exe`
/// is one real path), but a NUL or newline would tear the segment apart, and the selection
/// placeholder here would ask the runtime to run the clipboard — it substitutes arguments only.
fn is_executable_name(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("the executable name is empty".to_owned());
    }
    if value.contains('\0') {
        return Err("must not contain a NUL byte".to_owned());
    }
    if value.contains(['\n', '\r']) {
        return Err("must be one line — a whole command line belongs in --shell".to_owned());
    }
    if value.contains(crate::model::content::SELECTION_PLACEHOLDER) {
        return Err(
            "the selection placeholder is never substituted in the program name".to_owned(),
        );
    }
    Ok(())
}

fn is_single_line(value: &str) -> Result<(), String> {
    if value.contains(['\n', '\r']) {
        return Err("must be a single line".to_owned());
    }
    Ok(())
}

fn is_json_depth(value: &str) -> Result<(), String> {
    match value.parse::<u32>() {
        Ok(depth) if (1..=100).contains(&depth) => Ok(()),
        _ => Err(format!(
            "'{value}' is not a whole-number depth from 1 to 100"
        )),
    }
}

fn is_search_url_prefix(value: &str) -> Result<(), String> {
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err("must start with http:// or https://".to_owned());
    }
    if value.contains(['\n', '\r', '"', '\'']) {
        return Err("must be a plain URL prefix on one line, without quotes".to_owned());
    }
    Ok(())
}

fn is_cron(value: &str) -> Result<(), String> {
    shell::validate_cron(value)
        .map(|_| ())
        .map_err(|err| format!("{err:#}"))
}

// ---------------------------------------------------------------------------
// The catalogue
// ---------------------------------------------------------------------------

pub const SKILLS: &[Skill] = &[
    Skill {
        name: "ask",
        summary: "Ask an AI CLI about the text you have selected, answered as markdown",
        detail: "\
Select text anywhere — a browser, an editor, a chat — press the selected-text hotkey and pick this
sticker. The selection lands inside the single prompt argument the AI CLI already receives, and
the reply is rendered as markdown, so headings, lists and code blocks come out formatted.

The `tool` variable is one executable name looked up on PATH — the `-p` flag is a separate,
fixed token. The selection never reaches a shell: the sticker runtime passes argv directly, and
{{RUSTICKERS_SELECTION}} is substituted as argument data, never re-parsed.

The sticker is created closed: it never needs a window of its own, it only exists to be offered
when you trigger the hotkey.",
        vars: &[
            Var {
                name: "tool",
                description: "the AI CLI executable to run (one name, looked up on PATH)",
                default: Some("copilot"),
                validate: Some(is_executable_name),
            },
            Var {
                name: "title",
                description: "how the sticker is labelled in the selection picker",
                default: Some("Ask about selection"),
                validate: Some(is_single_line),
            },
        ],
        template: &[
            "command",
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
            "--",
            "${tool}",
            "-p",
            "{{RUSTICKERS_SELECTION}}",
        ],
    },
    Skill {
        name: "translate",
        summary: "Translate the selected text into another language",
        detail: "\
The same shape as `ask`, with the instruction fixed and the target language pulled out as a
variable. The whole prompt — instruction plus placeholder — is one argv segment, so the AI CLI
gets exactly one argument. Create one sticker per language and the selection picker becomes a
language menu.",
        vars: &[
            Var {
                name: "language",
                description: "the language to translate into",
                default: Some("English"),
                validate: Some(is_single_line),
            },
            Var {
                name: "tool",
                description: "the AI CLI executable to run (one name, looked up on PATH)",
                default: Some("copilot"),
                validate: Some(is_executable_name),
            },
        ],
        template: &[
            "command",
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
            "--",
            "${tool}",
            "-p",
            "Translate the following text into ${language}. Reply with the translation and \
             nothing else. Text: {{RUSTICKERS_SELECTION}}",
        ],
    },
    Skill {
        name: "explain-code",
        summary: "Explain the selected code, rendered as markdown with syntax-highlighted blocks",
        detail: "\
Aimed at reading unfamiliar code: select a function, trigger the hotkey, get a walkthrough back
in the same place you were reading. The instruction and the code share one prompt argument, and
markdown output means the code blocks in the explanation stay readable.",
        vars: &[Var {
            name: "tool",
            description: "the AI CLI executable to run (one name, looked up on PATH)",
            default: Some("copilot"),
            validate: Some(is_executable_name),
        }],
        template: &[
            "command",
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
            "--",
            "${tool}",
            "-p",
            "Explain what this code does, step by step. Use markdown. Code: \
             {{RUSTICKERS_SELECTION}}",
        ],
    },
    Skill {
        name: "web-search",
        summary: "Search the web for the selected text and open the results inside a sticker",
        detail: "\
Shows the other half of the selection contract. Instead of the {{RUSTICKERS_SELECTION}} argument
placeholder, the static PowerShell code reads the RUSTICKERS_SELECTION environment variable,
which the runtime always exports for a selection run — what you want when the text has to be
transformed (here, percent-encoded) before it can be used.

The `engine` variable only ever reaches the `--env SEARCH_ENGINE=...` data argument; the
executed code is a fixed string with nothing spliced into it. The command prints a URL and
nothing else, and `--result source` treats that output as something to open, not to display.

Windows-only as written: it invokes `powershell` directly as argv, not through a shell. On other
platforms write the equivalent with `--shell` and your own script.",
        vars: &[Var {
            name: "engine",
            description: "search URL prefix that the encoded query is appended to",
            default: Some("https://duckduckgo.com/?q="),
            validate: Some(is_search_url_prefix),
        }],
        template: &[
            "command",
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
            "--env",
            "SEARCH_ENGINE=${engine}",
            "--",
            "powershell",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$env:SEARCH_ENGINE + [uri]::EscapeDataString($env:RUSTICKERS_SELECTION)",
        ],
    },
    Skill {
        name: "format-json",
        summary: "Pretty-print the selected JSON",
        detail: "\
A local transform with no network and no AI: select a blob of minified JSON, trigger the
hotkey, read it indented. Plain text output, because JSON is already the formatting.

The `depth` variable is validated and reaches the command only as the `--env JSON_DEPTH=...`
data argument; the PowerShell code is static and reads it back as an environment variable.

Windows-only as written: it invokes `powershell` directly as argv, not through a shell.",
        vars: &[Var {
            name: "depth",
            description: "how many levels deep to expand nested objects (1-100)",
            default: Some("20"),
            validate: Some(is_json_depth),
        }],
        template: &[
            "command",
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
            "--env",
            "JSON_DEPTH=${depth}",
            "--",
            "powershell",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference = 'Stop'; $env:RUSTICKERS_SELECTION | ConvertFrom-Json | ConvertTo-Json -Depth ([int]$env:JSON_DEPTH)",
        ],
    },
    Skill {
        name: "watch",
        summary: "Re-run a shell script on a schedule and keep its latest output on screen",
        detail: "\
An always-on dashboard tile. The `script` variable is real shell code, executed through
`--shell` by the platform shell: PowerShell (`pwsh`, falling back to `powershell`) on Windows,
`sh -c` elsewhere — check the platform before assuming syntax. The script runs on a cron
schedule and `--stream` clears the previous output and fills the sticker line by line as the new
run produces it.

Schedules are Quartz-style and start with a seconds field, so every five minutes is
'0 */5 * * * *' rather than '*/5 * * * *'.

The schedule is not tied to the window. Close the sticker and it keeps ticking in the
background, so `rst result <id>` always returns the most recent run.",
        vars: &[
            Var {
                name: "script",
                description: "shell code to run on each tick (PowerShell on Windows, sh elsewhere); may span lines",
                default: Some("git status --short"),
                validate: None,
            },
            Var {
                name: "cron",
                description: "6-field schedule: second minute hour day month weekday",
                default: Some("0 */1 * * * *"),
                validate: Some(is_cron),
            },
            Var {
                name: "dir",
                description: "directory to run the script in",
                default: Some("."),
                validate: None,
            },
        ],
        template: &[
            "command",
            "--shell",
            "${script}",
            "--cron",
            "${cron}",
            "--run-now",
            "--stream",
            "--result",
            "text",
            "--dir",
            "${dir}",
            "--title",
            "watch: ${script}",
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
For commands that already produce a styled document — a coverage report, a generated
dashboard, a rendered template. The `script` variable is shell code run through `--shell`
(PowerShell on Windows, `sh` elsewhere); it has no default because it is code you choose
deliberately. The script must print one complete HTML document; it is loaded into a real
browser view, so CSS and JavaScript work and links open in your normal browser.",
        vars: &[
            Var {
                name: "script",
                description: "shell code that prints an HTML document (platform shell; see detail); may span lines",
                default: None,
                validate: None,
            },
            Var {
                name: "title",
                description: "how the sticker is labelled",
                default: Some("HTML report"),
                validate: Some(is_single_line),
            },
        ],
        template: &[
            "command",
            "--shell",
            "${script}",
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
sparkline. The `script` variable is shell code run through `--shell` (PowerShell on Windows,
`sh` elsewhere) and must print one complete SVG document — anything else renders as nothing at
all, because there is no error to show, just an image that will not decode.",
        vars: &[
            Var {
                name: "script",
                description: "shell code that prints an SVG document (platform shell; see detail); may span lines",
                default: None,
                validate: None,
            },
            Var {
                name: "title",
                description: "how the sticker is labelled",
                default: Some("Chart"),
                validate: Some(is_single_line),
            },
        ],
        template: &[
            "command",
            "--shell",
            "${script}",
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
Not every sticker runs something. This is the plain case: a markdown note that stays on screen
and is edited in place. The text is the positional TEXT argument of `rst note`.

For anything longer than a line, skip the variable and pipe the document in instead:
`rst note --file notes.md`, or `--file -` to read standard input.",
        vars: &[Var {
            name: "text",
            description: "the markdown content",
            default: Some("# Notes\n\n- "),
            validate: None,
        }],
        // `--` keeps text that starts with a dash (even `--json`) as the positional value.
        template: &["note", "--color", "yellow", "--", "${text}"],
    },
];

pub fn find(name: &str) -> Option<&'static Skill> {
    SKILLS.iter().find(|skill| skill.name == name)
}

pub fn names() -> Vec<&'static str> {
    SKILLS.iter().map(|skill| skill.name).collect()
}

impl Skill {
    /// Resolve `overrides` against the declared defaults, then check every resolved value.
    ///
    /// An unknown name is an error rather than a silent no-op: a typo in `--set` would
    /// otherwise produce a sticker that looks right and quietly uses the default.
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
            let Some(value) = overrides.get(var.name).map(String::as_str).or(var.default) else {
                missing.push(var.name);
                continue;
            };
            if let Some(validate) = var.validate
                && let Err(reason) = validate(value)
            {
                bail!(
                    "'{value}' is not a valid '{name}' for the '{skill}' skill: {reason}",
                    name = var.name,
                    skill = self.name,
                );
            }
            resolved.insert(var.name.to_owned(), value.to_owned());
        }

        if !missing.is_empty() {
            bail!(
                "the '{}' skill needs {}. Pass {}.",
                self.name,
                missing.join(" and "),
                missing
                    .iter()
                    .map(|name| format!("--set {name}=..."))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }

        Ok(resolved)
    }

    /// Variables with no default and no override — the reason a skill cannot run yet.
    pub fn missing(&self, overrides: &BTreeMap<String, String>) -> Vec<&'static str> {
        self.vars
            .iter()
            .filter(|var| var.default.is_none() && !overrides.contains_key(var.name))
            .map(|var| var.name)
            .collect()
    }

    pub fn var_names(&self) -> Vec<&'static str> {
        self.vars.iter().map(|var| var.name).collect()
    }

    /// Substitute the resolved variables into the template to get real CLI arguments.
    ///
    /// One pass over each segment: inserted values are never rescanned, so a value containing
    /// `${...}` stays literal data and cannot trigger another expansion.
    pub fn expand(&self, vars: &BTreeMap<String, String>) -> Vec<String> {
        self.template
            .iter()
            .map(|part| substitute_one_pass(part, vars))
            .collect()
    }
}

/// Replace every `${name}` with `vars[name]` exactly once, without re-reading what was
/// inserted. An unknown name is left in place so the undeclared-placeholder test finds it.
fn substitute_one_pass(text: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) if !after[..end].is_empty() && vars.contains_key(&after[..end]) => {
                out.push_str(&vars[&after[..end]]);
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Human display, separate from the canonical argv
// ---------------------------------------------------------------------------

/// Characters that reach a shell unharmed inside a word; anything else (spaces, `$`,
/// backticks, quotes, braces...) forces quoting.
const SHELL_SAFE: &[char] = &[
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L',
    'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '0', '1', '2', '3', '4',
    '5', '6', '7', '8', '9', '-', '_', '.', '/', ':', '=', '@', '+', ',', '%',
];

fn needs_quotes(token: &str) -> bool {
    token.is_empty() || !token.chars().all(|c| SHELL_SAFE.contains(&c))
}

/// Quote one token for POSIX shells: single quotes with the `'\''` dance inside.
pub fn quote_for_posix_shell(token: &str) -> String {
    if !needs_quotes(token) {
        return token.to_owned();
    }
    format!("'{}'", token.replace('\'', r"'\''"))
}

/// Quote one token for PowerShell (`pwsh` and `powershell`): single quotes with `''` doubling.
/// Single quoting also neutralises `$`, which double quoting does not.
pub fn quote_for_powershell(token: &str) -> String {
    if !needs_quotes(token) && !token.starts_with('@') {
        return token.to_owned();
    }
    format!("'{}'", token.replace('\'', "''"))
}

/// Quote one token for the shell of the machine this binary runs on.
pub fn quote_for_host_shell(token: &str) -> String {
    if cfg!(windows) {
        quote_for_powershell(token)
    } else {
        quote_for_posix_shell(token)
    }
}

/// Render an argument list as a command line someone (or some agent) can paste into the host
/// shell. The canonical form for programs is the `argv` JSON array the same reply carries.
pub fn human_command_line(args: &[String]) -> String {
    let quoted: Vec<String> = args.iter().map(|arg| quote_for_host_shell(arg)).collect();
    format!("rst {}", quoted.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::content::SELECTION_PLACEHOLDER;

    fn overrides(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// Fully expand a skill with every variable pinned to a distinctive probe value.
    fn expand_probe(skill: &Skill) -> Vec<String> {
        let vars: BTreeMap<String, String> = skill
            .vars
            .iter()
            .map(|var| (var.name.to_owned(), "PROBE".to_owned()))
            .collect();
        skill.expand(&vars)
    }

    fn expanded_with_defaults(name: &str) -> Vec<String> {
        let skill = find(name).unwrap();
        skill.expand(&skill.resolve_vars(&BTreeMap::new()).unwrap())
    }

    fn program_and_args(argv: &[String]) -> &[String] {
        let start = argv
            .iter()
            .position(|arg| arg == "--")
            .unwrap_or_else(|| panic!("argv has no `--` separator: {argv:?}"));
        &argv[start + 1..]
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
    fn every_template_starts_with_a_current_subcommand() {
        for skill in SKILLS {
            let first = skill.template.first().copied().unwrap_or_default();
            assert!(
                ["command", "note"].contains(&first),
                "skill '{}' expands to '{first}'; the grammar is `command` and `note`",
                skill.name
            );
        }
    }

    #[test]
    fn every_template_placeholder_has_a_declared_variable() {
        for skill in SKILLS {
            for part in expand_probe(skill) {
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
    fn substitution_is_one_pass_and_never_recurses() {
        let skill = find("translate").unwrap();
        let vars = skill
            .resolve_vars(&overrides(&[("language", "${tool}")]))
            .unwrap();
        let prompt = skill.expand(&vars).pop().unwrap();
        assert!(
            prompt.contains("into ${tool}."),
            "an inserted value must stay literal data: {prompt}"
        );
    }

    #[test]
    fn unknown_placeholders_stay_visible_instead_of_vanishing() {
        let vars = overrides(&[("text", "hi")]);
        assert_eq!(substitute_one_pass("a${nope}b", &vars), "a${nope}b");
    }

    #[test]
    fn ai_skills_pass_one_prompt_argument_holding_the_selection() {
        for name in ["ask", "translate", "explain-code"] {
            let argv = expanded_with_defaults(name);
            let with_args = program_and_args(&argv);
            assert_eq!(with_args[0], "copilot", "{name} default tool");
            assert_eq!(with_args[1], "-p", "{name}: -p is its own fixed token");
            assert_eq!(
                with_args.len(),
                3,
                "{name}: program, -p, one prompt: {with_args:?}"
            );
            assert_eq!(
                argv.iter()
                    .filter(|segment| segment.contains(SELECTION_PLACEHOLDER))
                    .count(),
                1,
                "{name}: exactly one argument carries the placeholder"
            );
            assert!(with_args[2].contains(SELECTION_PLACEHOLDER), "{name}");
        }
    }

    #[test]
    fn selection_placeholder_matches_the_runtime_constant() {
        let ask = expanded_with_defaults("ask");
        assert!(program_and_args(&ask)[2].contains(SELECTION_PLACEHOLDER));
    }

    #[test]
    fn script_skills_pass_the_script_as_one_shell_argument() {
        for name in ["watch", "html-report", "svg-chart"] {
            let skill = find(name).unwrap();
            let at = skill
                .template
                .iter()
                .position(|part| *part == "--shell")
                .unwrap_or_else(|| panic!("{name} has no --shell segment"));
            assert_eq!(
                skill.template[at + 1],
                "${script}",
                "{name}: --shell must take exactly the whole script as its one value"
            );
        }
    }

    #[test]
    fn powershell_skills_never_splice_values_into_their_code() {
        for name in ["web-search", "format-json"] {
            let expanded = expand_probe(find(name).unwrap());
            assert_eq!(program_and_args(&expanded)[0], "powershell");
            let code = program_and_args(&expanded).last().unwrap();
            assert!(
                code.contains("$env:"),
                "{name}: code must read data from the environment, not a quoted literal: {code}"
            );
            assert!(
                !code.contains("PROBE"),
                "{name}: code must stay static: {code}"
            );
            let carriers: Vec<&String> = expanded
                .iter()
                .filter(|segment| segment.contains("PROBE"))
                .collect();
            assert!(
                carriers
                    .iter()
                    .all(|segment| segment.starts_with("SEARCH_ENGINE=")
                        || segment.starts_with("JSON_DEPTH=")),
                "{name}: values may only reach --env data: {carriers:?}"
            );
            let env_at = expanded.iter().position(|a| a == "--env").unwrap();
            assert!(expanded[env_at + 1].contains("PROBE"), "{name}");
        }
    }

    #[test]
    fn defaults_are_used_when_nothing_is_passed() {
        let skill = find("translate").unwrap();
        let vars = skill.resolve_vars(&BTreeMap::new()).unwrap();
        assert_eq!(vars["language"], "English");
    }

    #[test]
    fn an_override_wins_over_the_default_and_reaches_the_argv() {
        let skill = find("translate").unwrap();
        let vars = skill
            .resolve_vars(&overrides(&[("language", "Japanese")]))
            .unwrap();
        let expanded = skill.expand(&vars);
        assert!(expanded.contains(&"Translate to Japanese".to_owned()));
        assert!(expanded.last().unwrap().contains("into Japanese."));
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
    fn a_required_variable_must_be_supplied_and_is_listed_as_missing() {
        let skill = find("html-report").unwrap();
        let err = skill.resolve_vars(&BTreeMap::new()).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("--set script="), "{message}");
        assert_eq!(skill.missing(&BTreeMap::new()), vec!["script"]);
        assert!(skill.missing(&overrides(&[("script", "x")])).is_empty());
    }

    #[test]
    fn a_bad_depth_value_is_rejected_before_anything_expands() {
        let skill = find("format-json").unwrap();
        for bad in ["0", "101", "abc", "-1", "2.5"] {
            let err = skill
                .resolve_vars(&overrides(&[("depth", bad)]))
                .unwrap_err();
            assert!(format!("{err:#}").contains("depth"), "{bad}: {err:#}");
        }
        assert!(skill.resolve_vars(&overrides(&[("depth", "7")])).is_ok());
    }

    #[test]
    fn a_crontab_style_schedule_is_rejected_with_the_six_field_fix() {
        let skill = find("watch").unwrap();
        let err = skill
            .resolve_vars(&overrides(&[("cron", "*/5 * * * *")]))
            .unwrap_err();
        assert!(format!("{err:#}").contains("0 */5 * * * *"), "{err:#}");
    }

    #[test]
    fn a_tool_value_must_be_one_argv_segment_naming_a_program() {
        let skill = find("ask").unwrap();
        // Spaces are fine: a real executable path may contain them.
        assert!(
            skill
                .resolve_vars(&overrides(&[("tool", r"C:\Program Files\ai\cli.exe")]))
                .is_ok()
        );
        for bad in ["", "a\nb", "x\0y", "{{RUSTICKERS_SELECTION}}"] {
            assert!(
                skill.resolve_vars(&overrides(&[("tool", bad)])).is_err(),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn the_selection_placeholder_survives_expansion() {
        let expanded = expanded_with_defaults("ask");
        assert!(
            expanded
                .iter()
                .any(|part| part.contains(SELECTION_PLACEHOLDER))
        );
    }

    #[test]
    fn the_note_skill_passes_its_text_as_data_even_when_it_starts_with_a_dash() {
        let expanded = find("note")
            .unwrap()
            .expand(&overrides(&[("text", "--json rm everything")]));
        assert_eq!(
            expanded,
            vec!["note", "--color", "yellow", "--", "--json rm everything"]
        );
    }

    #[test]
    fn a_multiline_script_reaches_the_shell_whole() {
        let skill = find("watch").unwrap();
        let vars = skill
            .resolve_vars(&overrides(&[("script", "git status\ngit log -1")]))
            .unwrap();
        let expanded = skill.expand(&vars);
        let at = expanded.iter().position(|a| a == "--shell").unwrap();
        assert_eq!(expanded[at + 1], "git status\ngit log -1");
    }

    #[test]
    fn powershell_quoting_doubles_single_quotes_and_neutralises_dollars() {
        assert_eq!(quote_for_powershell("a'b"), "'a''b'");
        assert_eq!(quote_for_powershell("$(calc)"), "'$(calc)'");
        assert_eq!(quote_for_powershell("plain"), "plain");
        assert_eq!(quote_for_powershell(""), "''");
    }

    #[test]
    fn host_display_quotes_expansion_and_splatting_prefixes() {
        assert_eq!(quote_for_posix_shell("~/file"), "'~/file'");
        assert_eq!(quote_for_powershell("@args"), "'@args'");
    }

    #[test]
    fn posix_quoting_closes_reopens_around_single_quotes() {
        assert_eq!(quote_for_posix_shell("a'b"), r"'a'\''b'");
        assert_eq!(quote_for_posix_shell("a b"), "'a b'");
        assert_eq!(quote_for_posix_shell("$HOME"), "'$HOME'");
    }

    #[test]
    fn a_human_line_is_prefixed_and_quotes_only_what_needs_it() {
        let line = human_command_line(&[
            "command".into(),
            "git status".into(),
            "--closed".into(),
            "".into(),
        ]);
        assert!(line.starts_with("rst command "), "{line}");
        assert!(
            line.contains("'git status'") || line.contains("\"git status\""),
            "{line}"
        );
        assert!(line.contains("''"), "{line}");
    }
}
