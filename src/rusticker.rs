//! Entry point for the `rusticker` command line binary.

use clap::Parser as _;
use std::ffi::OsString;
use std::path::Path;

use rustickers::cli;
use rustickers::storage::paths::AppPaths;

fn main() {
    let app_paths = match AppPaths::new() {
        Ok(paths) => paths,
        Err(err) => {
            eprintln!("error: could not locate the Rustickers data directory: {err:#}");
            std::process::exit(1);
        }
    };

    let cli = cli::Cli::parse_from(expand_view_shorthand(std::env::args_os().collect()));
    let format = cli.format();

    if let Err(err) = cli::run(cli, &app_paths) {
        format.emit_error(&err);
        std::process::exit(1);
    }
}

/// Let `rusticker <file-or-url>` mean `rusticker view <file-or-url>`.
///
/// The shorthand only applies when the first argument is unambiguous — an existing path or a URL,
/// and not the name of a subcommand. The subcommand list comes from the parser itself, so adding
/// a subcommand can never be shadowed by a file that happens to share its name.
fn expand_view_shorthand(args: Vec<OsString>) -> Vec<OsString> {
    let Some(first) = args.get(1).and_then(|arg| arg.to_str()) else {
        return args;
    };

    if first.starts_with('-') || cli::subcommand_names().iter().any(|name| name == first) {
        return args;
    }

    if !(Path::new(first).exists() || rustickers::utils::url::is_url(first)) {
        return args;
    }

    let mut expanded = vec![args[0].clone(), OsString::from("view")];
    expanded.extend(args.into_iter().skip(1));
    expanded
}
