//! Entry point for the explicit `rst` command interface.
fn main() {
    std::process::exit(rustickers::cli::main_entry(std::env::args_os()));
}
