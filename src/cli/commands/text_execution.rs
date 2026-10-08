//! Scoped text authorization and safe direct child execution.
use super::{text_argv, text_terminal};
use crate::{
    cli::runtime::block_on,
    text::{TextEntryId, TextRepository, TextSources},
};
use anyhow::{Context, Result, ensure};
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct ExitCode(pub i32);
impl std::fmt::Display for ExitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "command exited with status {}", self.0)
    }
}
impl std::error::Error for ExitCode {}

pub(super) fn run(
    repository: &TextRepository,
    sources: TextSources,
    state: Option<crate::model::sticker::StickerState>,
    id: Option<TextEntryId>,
    query: &str,
    command: &[String],
) -> Result<()> {
    text_argv::validate_template(command)?;
    let rows = block_on(repository.snapshot(sources, ""))?
        .into_iter()
        .filter(|e| state.is_none() || e.state().is_none() || e.state() == state)
        .collect::<Vec<_>>();
    let id = if let Some(id) = id {
        ensure!(sources.includes(id), "ID is outside selected text sources");
        id
    } else {
        text_terminal::choose(&rows, query)?.ok_or(ExitCode(130))?
    };
    let selected = rows
        .iter()
        .find(|e| e.id() == id)
        .context("text not available in selected sources/state")?;
    let password = if selected.is_encrypted() {
        Some(text_terminal::password("Master password: ")?.ok_or(ExitCode(130))?)
    } else {
        None
    };
    let text = block_on(repository.resolve(selected, password.as_deref().map(|s| s.as_str())))?;
    let args = Zeroizing::new(text_argv::build(command, &text)?);
    let mut child = match std::process::Command::new(&args[0])
        .args(&args[1..])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let code = match error.kind() {
                std::io::ErrorKind::NotFound => 127,
                std::io::ErrorKind::PermissionDenied => 126,
                _ => 1,
            };
            // Do not print expanded arguments, which can contain decrypted secrets.
            eprintln!("error: could not launch selected command: {error}");
            return Err(ExitCode(code).into());
        }
    };
    if block_on(repository.mark_used(id, crate::utils::time::now_unix_millis())).is_err() {
        eprintln!("warning: could not update text recency");
    }
    let status = child.wait().context("wait for child command")?;
    #[cfg(unix)]
    let code = {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
    };
    #[cfg(not(unix))]
    let code = status.code().unwrap_or(1);
    if code == 0 {
        Ok(())
    } else {
        Err(ExitCode(code).into())
    }
}
