//! `rusticker skill` — browse and run the worked examples.

use anyhow::bail;
use serde_json::json;
use std::collections::BTreeMap;

use crate::storage::paths::AppPaths;

use crate::cli::output::{Format, table};
use crate::cli::skills::{self, Skill};

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
        /// Skill name, as listed by `rusticker skill list`
        name: String,
    },

    /// Create the sticker a skill describes
    Run {
        /// Skill name, as listed by `rusticker skill list`
        name: String,

        /// Override one of the skill's variables, repeatable
        #[arg(long, value_name = "KEY=VALUE")]
        var: Vec<String>,

        /// Print the command that would run, without creating anything
        #[arg(long)]
        dry_run: bool,
    },
}

fn parse_vars(pairs: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    let mut vars = BTreeMap::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            bail!("--var expects KEY=VALUE, got '{pair}'");
        };
        vars.insert(key.trim().to_owned(), value.to_owned());
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
                "\n`rusticker skill show <name>` explains one; `rusticker skill run <name>` \
                 creates it."
            );
        },
    );
}

fn show(name: &str, format: Format) -> anyhow::Result<()> {
    let skill = lookup(name)?;
    let vars = skill.resolve_vars(&BTreeMap::new()).unwrap_or_default();
    // A skill with a required variable cannot be fully expanded, so show the template as written.
    let expanded = if vars.len() == skill.vars.len() {
        skill.expand(&vars)
    } else {
        skill
            .template
            .iter()
            .map(|part| (*part).to_owned())
            .collect()
    };

    let mut payload = skill_json(skill);
    payload["command"] = json!(expanded);
    payload["command_line"] = json!(skills::command_line(&expanded));

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

        println!("Equivalent command:");
        println!("  {}", skills::command_line(&expanded));
        println!("\nRun it with:");
        println!("  rusticker skill run {}", skill.name);
    });

    Ok(())
}

fn run_skill(
    app_paths: &AppPaths,
    name: &str,
    var: &[String],
    dry_run: bool,
    format: Format,
) -> anyhow::Result<()> {
    let skill = lookup(name)?;
    let vars = skill.resolve_vars(&parse_vars(var)?)?;
    let expanded = skill.expand(&vars);

    if dry_run {
        format.emit(
            json!({
                "skill": skill.name,
                "vars": vars,
                "command": expanded,
                "command_line": skills::command_line(&expanded),
                "dry_run": true,
            }),
            || println!("{}", skills::command_line(&expanded)),
        );
        return Ok(());
    }

    format.note(format!("Running: {}", skills::command_line(&expanded)));

    // Feeding the expansion back through the ordinary parser is what keeps a skill honest: it can
    // only do things the CLI can already do, and it does them exactly as `skill show` advertised.
    let mut argv = vec!["rusticker".to_owned()];
    if format.is_json() {
        argv.push("--json".to_owned());
    }
    argv.extend(expanded);

    let cli = <crate::cli::Cli as clap::Parser>::try_parse_from(&argv).map_err(|err| {
        anyhow::anyhow!(
            "the '{}' skill produced invalid arguments: {err}",
            skill.name
        )
    })?;

    crate::cli::run(cli, app_paths)
}

pub fn run(app_paths: &AppPaths, args: SkillArgs, format: Format) -> anyhow::Result<()> {
    match args.action {
        None | Some(SkillAction::List) => {
            list(format);
            Ok(())
        }
        Some(SkillAction::Show { name }) => show(&name, format),
        Some(SkillAction::Run { name, var, dry_run }) => {
            run_skill(app_paths, &name, &var, dry_run, format)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_pairs_are_split_on_the_first_equals_sign() {
        let vars = parse_vars(&["engine=https://x/?q=".into()]).unwrap();
        assert_eq!(vars["engine"], "https://x/?q=");
    }

    #[test]
    fn a_var_without_an_equals_sign_is_rejected() {
        assert!(parse_vars(&["engine".into()]).is_err());
    }

    #[test]
    fn an_unknown_skill_lists_the_real_ones() {
        let err = lookup("nope").unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("no 'nope' skill"), "{message}");
        assert!(message.contains("ask"), "{message}");
    }
}
