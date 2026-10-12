//! Executable documentation: every literal rst example must match the current grammar.
#![cfg(feature = "cli")]
use rustickers::cli::Cli;

fn check_examples(name: &str, document: &str) -> usize {
    let mut count = 0;
    let mut fenced = false;
    for line in document.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let line = line.trim();
        if !fenced || !line.starts_with("rst ") {
            continue;
        }
        let argv =
            shlex::split(line).unwrap_or_else(|| panic!("{name}: invalid shell example: {line}"));
        Cli::try_parse_from(&argv).unwrap_or_else(|error| panic!("{name}: {line}\n{error}"));
        count += 1;
    }
    count
}
#[test]
fn readme_and_exported_agent_guide_use_the_current_cli() {
    let readme = check_examples("README.md", include_str!("../README.md"));
    let guide = check_examples("agent-guide.md", include_str!("../src/cli/agent-guide.md"));
    assert!(readme >= 25, "keep real human examples exercised");
    assert!(
        guide >= 5,
        "guide needs executable workflows, not only prose"
    );
}
