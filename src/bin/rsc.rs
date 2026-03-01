use std::path::Path;

use clap::Parser as _;
use rustickers::cli;
use rustickers::storage::paths::AppPaths;

fn main() {
    let app_paths = AppPaths::new().expect("App paths should initialize");

    // when args length is 1 and it is a valid file/url then run view command directly
    let cli = if std::env::args().len() == 2 {
        let arg = std::env::args().nth(1).unwrap();
        if rustickers::utils::url::is_url(&arg)
            || Path::new(&arg).is_dir()
            || Path::new(&arg).is_file()
        {
            cli::view::run(arg).unwrap_or_else(|err| {
                eprintln!("error: {err:#}");
                std::process::exit(1);
            });
            return;
        } else {
            cli::Cli::parse()
        }
    } else {
        cli::Cli::parse()
    };

    if let Err(err) = cli::run(cli, &app_paths) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
