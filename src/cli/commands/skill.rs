//! `rst skill` — discover, preview and create the built-in recipes.
//!
//! Discovery (`list`, `show`, `guide`) never touches the database and never depends on what is
//! installed on the machine: `show` parser-validates its expansion, but does not resolve
//! programs on PATH, so a missing `copilot` cannot break browsing the catalogue. Creating
//! (`run`) re-parses the expansion into the ordinary [`Cli`] and executes it on the caller's
//! [`Runtime`], which is what keeps `--offline` and `--data-dir` isolation intact: a skill can
//! only do things the CLI can already do, exactly as `skill show` advertised them.

use anyhow::bail;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

use crate::cli::Cli;
use crate::cli::output::{Format, table};
use crate::cli::runtime::Runtime;
use crate::cli::skills::{self, Skill};

/// The operational guide, shipped inside the binary so `skill guide` works with no database,
/// no desktop app and no network.
pub const GUIDE: &str = include_str!("../agent-guide.md");

#[derive(clap::Args, Debug)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub action: Option<SkillAction>,
}

#[derive(clap::Subcommand, Debug)]
pub enum SkillAction {
    /// List every skill with a one-line summary
    List,

    /// Explain one skill, its variables and the command it stands for
    Show {
        /// Skill name, as listed by `rst skill list`
        name: String,
    },

    /// Create the sticker a skill describes
    Run {
        /// Skill name, as listed by `rst skill list`
        name: String,

        /// Override one of the skill's variables, repeatable
        #[arg(long, value_name = "KEY=VALUE")]
        set: Vec<String>,

        /// Validate the expansion, including command semantics, without creating anything
        #[arg(long)]
        dry_run: bool,
    },

    /// Print the operational guide for agents (Markdown, no database needed)
    Guide,
}

/// Turn `--set KEY=VALUE` arguments into overrides.
///
/// Split on the *first* `=` so values may contain `=`. A repeated key is an error: silently
/// keeping one of two contradictory values is the worst option available.
fn parse_sets(pairs: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    let mut vars = BTreeMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            bail!("--set expects KEY=VALUE, got '{pair}'");
        };
        let key = key.trim();
        if key.is_empty() {
            bail!("--set has an empty variable name in '{pair}'");
        }
        if !seen.insert(key.to_owned()) {
            bail!("--set '{key}' was given more than once");
        }
        vars.insert(key.to_owned(), value.to_owned());
    }
    Ok(vars)
}

fn lookup(name: &str) -> anyhow::Result<&'static Skill> {
    skills::find(name).ok_or_else(|| {
        anyhow::anyhow!(
            "there is no '{name}' skill. Available: {}",
            skills::names().join(", ")
        )
    })
}

/// Rebuild a full CLI invocation from a skill's expansion, exactly as `run` would.
///
/// Global flags (`--json`, `--data-dir`, `--offline`) stay at the outer level; the runtime
/// carries isolation, so only `--json` is replayed here.
fn build_cli(skill: &Skill, argv: &[String], json: bool) -> anyhow::Result<Cli> {
    let mut full: Vec<String> = vec!["rst".to_owned()];
    if json {
        full.push("--json".to_owned());
    }
    full.extend_from_slice(argv);
    Cli::try_parse_from(&full).map_err(|err| {
        anyhow::anyhow!(
            "the '{}' skill produced arguments the parser rejects: {err}",
            skill.name
        )
    })
}

fn skill_json(skill: &Skill) -> serde_json::Value {
    json!({
        "name": skill.name,
        "summary": skill.summary,
        "detail": skill.detail,
        "vars": skill.vars.iter().map(|var| json!({
            "name": var.name,
            "description": var.description,
            "default": var.default,
            "required": var.default.is_none(),
        })).collect::<Vec<_>>(),
    })
}

fn list(format: Format) {
    format.emit(
        json!({ "skills": skills::SKILLS.iter().map(skill_json).collect::<Vec<_>>() }),
        || {
            let rows: Vec<Vec<String>> = skills::SKILLS
                .iter()
                .map(|skill| vec![skill.name.to_owned(), skill.summary.to_owned()])
                .collect();
            table(&["SKILL", "WHAT IT DOES"], &rows);
            println!(
                "\n`rst skill show <name>` explains one; `rst skill run <name>` creates it; \
                 `rst skill guide` is the operational handbook."
            );
        },
    );
}

/// Show one skill. A skill with a required variable cannot be fully expanded, so its template
/// is shown as written and clearly marked as not runnable — no fabricated, misleading
/// invocation. Only the parser is consulted; PATH and the database are not.
fn show(name: &str, format: Format) -> anyhow::Result<()> {
    let skill = lookup(name)?;
    let missing = skill.missing(&BTreeMap::new());
    let complete = missing.is_empty();

    let argv = if complete {
        let expanded = skill.expand(&skill.resolve_vars(&BTreeMap::new())?);
        build_cli(skill, &expanded, false)?;
        expanded
    } else {
        skill
            .template
            .iter()
            .map(|part| (*part).to_owned())
            .collect()
    };
    let line = skills::human_command_line(&argv);

    let mut payload = skill_json(skill);
    payload["complete"] = json!(complete);
    payload["missing"] = json!(missing);
    payload["argv"] = json!(argv);
    payload["command_line"] = json!(line);

    format.emit(payload, || {
        println!("{}  —  {}\n", skill.name, skill.summary);
        println!("{}\n", skill.detail);

        if !skill.vars.is_empty() {
            println!("Variables:");
            let rows: Vec<Vec<String>> = skill
                .vars
                .iter()
                .map(|var| {
                    vec![
                        var.name.to_owned(),
                        var.default
                            .map(|d| format!("{d:?}"))
                            .unwrap_or_else(|| "(required)".to_owned()),
                        var.description.to_owned(),
                    ]
                })
                .collect();
            table(&["NAME", "DEFAULT", "MEANING"], &rows);
            println!();
        }

        if complete {
            println!("Equivalent command (host-quoted for pasting; --json carries the canonical argv array):");
            println!("  {line}");
            println!("\nRun it with:");
            println!("  rst skill run {}", skill.name);
        } else {
            println!("Template — NOT runnable until every required variable is set:");
            println!("  {line}");
            println!("\nSet them with:");
            println!(
                "  rst skill run {} {}",
                skill.name,
                missing
                    .iter()
                    .map(|name| format!("--set {name}=..."))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    });

    Ok(())
}

fn run_skill(
    runtime: &Runtime,
    name: &str,
    set: &[String],
    dry_run: bool,
    format: Format,
) -> anyhow::Result<()> {
    let skill = lookup(name)?;
    let vars = skill.resolve_vars(&parse_sets(set)?)?;
    let expanded = skill.expand(&vars);
    let cli = build_cli(skill, &expanded, format.is_json())?;

    if dry_run {
        // The same semantic checks `command` performs before storing (program on PATH,
        // selection contract, cron) — through `Cli::validate`, which reaches `CommandArgs::validate`
        // and writes nothing anywhere.
        cli.validate()?;
        let line = skills::human_command_line(&expanded);
        format.emit(
            json!({
                "skill": skill.name,
                "vars": vars,
                "argv": expanded,
                "command_line": line,
                "dry_run": true,
            }),
            || {
                println!("{line}");
                println!("note: dry run — validated, nothing created");
            },
        );
        return Ok(());
    }

    format.note(format!(
        "Creating: {}",
        skills::human_command_line(&expanded)
    ));

    // Executing the re-parsed CLI on the caller's runtime keeps the skill inside the ordinary
    // command path and inside whatever isolation (--offline, --data-dir) that runtime has.
    cli.execute(runtime)
}

pub(crate) fn run(runtime: &Runtime, args: SkillArgs, format: Format) -> anyhow::Result<()> {
    match args.action {
        None | Some(SkillAction::List) => {
            list(format);
            Ok(())
        }
        Some(SkillAction::Show { name }) => show(&name, format),
        Some(SkillAction::Run { name, set, dry_run }) => {
            run_skill(runtime, &name, &set, dry_run, format)
        }
        Some(SkillAction::Guide) => {
            format.emit(json!({ "guide": GUIDE }), || print!("{GUIDE}"));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_pairs_split_on_the_first_equals_sign() {
        let vars = parse_sets(&["engine=https://x/?q=".into()]).unwrap();
        assert_eq!(vars["engine"], "https://x/?q=");
    }

    #[test]
    fn a_set_without_an_equals_sign_is_rejected() {
        assert!(parse_sets(&["engine".into()]).is_err());
    }

    #[test]
    fn a_repeated_set_key_is_rejected() {
        let err = parse_sets(&["a=1".into(), "a=2".into()]).unwrap_err();
        assert!(format!("{err:#}").contains("more than once"));
    }

    #[test]
    fn an_empty_set_key_is_rejected() {
        assert!(parse_sets(&["=1".into()]).is_err());
    }

    #[test]
    fn an_unknown_skill_lists_the_real_ones() {
        let err = lookup("nope").unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("no 'nope' skill"), "{message}");
        assert!(message.contains("ask"), "{message}");
    }

    fn guide_rst_lines() -> Vec<&'static str> {
        let mut inside_fence = false;
        GUIDE
            .lines()
            .filter(|line| {
                if line.trim_start().starts_with("```") {
                    inside_fence = !inside_fence;
                    return false;
                }
                inside_fence && line.starts_with("rst ")
            })
            .collect()
    }

    #[test]
    fn the_guide_ships_the_whole_catalogue_and_the_safety_contract() {
        for skill in skills::SKILLS {
            assert!(
                GUIDE.contains(skill.name),
                "guide never mentions {}",
                skill.name
            );
        }
        for anchor in [
            "{{RUSTICKERS_SELECTION}}",
            "{{txt}}",
            "--set",
            "--shell",
            "--data-dir",
            "--non-interactive",
            "PowerShell",
            "0 */5 * * * *",
        ] {
            assert!(GUIDE.contains(anchor), "guide is missing {anchor}");
        }
    }

    #[test]
    fn every_guide_command_line_parses() {
        let lines = guide_rst_lines();
        assert!(
            lines.len() >= 5,
            "the guide needs at least 5 rst lines, found {}",
            lines.len()
        );
        for line in lines {
            let parts =
                shlex::split(line).unwrap_or_else(|| panic!("guide line must split:\n  {line}"));
            assert_eq!(parts.first().map(String::as_str), Some("rst"), "{line}");
            Cli::try_parse_from(&parts)
                .unwrap_or_else(|err| panic!("guide line does not parse:\n  {line}\n  {err}"));
        }
    }
}
