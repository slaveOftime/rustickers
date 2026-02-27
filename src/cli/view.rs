use std::io::Write as _;

use super::console::console_writer;

pub fn run(source: String) -> anyhow::Result<()> {
    let mut out = console_writer();
    match crate::ipc::send_ipc_command("rustickers", &format!("PREVIEW_FILE {source}")) {
        Ok(true) => {}
        Ok(false) => {
            writeln!(
                out,
                "Rustickers is not running. Please launch it first, then retry."
            )?;
            std::process::exit(1);
        }
        Err(err) => {
            writeln!(out, "error: {err}")?;
            std::process::exit(1);
        }
    }
    Ok(())
}
