use clap::Parser as _;
use clap::error::ErrorKind;
use rustickers::cli;
use rustickers::storage::paths::AppPaths;

fn main() {
    let app_paths = AppPaths::new().expect("App paths should initialize");

    let cli = match cli::Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let code = match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 2,
            };
            let _ = err.print();
            std::process::exit(code);
        }
    };

    if let Err(err) = cli::run(cli, &app_paths) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
