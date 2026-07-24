//! This is the entry file for the CLI application.
//! It parses command-line arguments and dispatches commands accordingly.

use clap::Parser as _;
use std::ffi::OsString;
use std::path::Path;

use rustickers::cli;
use rustickers::storage::paths::AppPaths;

fn main() {
    let app_paths = AppPaths::new().expect("App paths should initialize");

    let cli = cli::Cli::parse_from(normalize_argv_for_view_alias());

    if let Err(err) = cli::run(cli, &app_paths) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn normalize_argv_for_view_alias() -> Vec<OsString> {
    let args: Vec<OsString> = std::env::args_os().collect();

    if args.len() < 2 {
        return args;
    }

    let source = args[1].clone();

    // Don't alias if the first positional arg is a known subcommand or help flag.
    let known_commands = [
        "view", "markdown", "cmd", "list", "show", "open", "close", "help",
    ];
    if source.to_str().is_some_and(|s| known_commands.contains(&s)) {
        return args;
    }

    // Don't alias flags (e.g. --help, --version).
    if source.to_str().is_some_and(|s| s.starts_with('-')) {
        return args;
    }

    let source_path = Path::new(&source);
    let is_existing_path = source_path.is_file() || source_path.is_dir();
    let is_url = source.to_str().is_some_and(rustickers::utils::url::is_url);

    if is_existing_path || is_url {
        let mut result = vec![args[0].clone(), OsString::from("view"), source];
        result.extend(args.into_iter().skip(2));
        result
    } else {
        args
    }
}
