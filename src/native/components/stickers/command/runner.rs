//! Spawning a command sticker's child process and collecting its output.
//!
//! Nothing here touches GPUI, so the same code runs both behind an open sticker window and inside
//! the headless background scheduler.

use std::{
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crate::model::content::{CommandContent, SELECTION_ENV_VAR, SELECTION_PLACEHOLDER};

/// How often the supervisor thread asks the child whether it has exited.
const REAP_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A line of output, or the exit notification.
pub enum CmdEvent {
    Output(String),
    Error(String),
    Done { success: bool },
}

/// A resolved, ready-to-spawn command.
///
/// Resolution happens up front so that "command not found" is reported before any process exists,
/// and so the caller can log exactly what is about to run.
pub struct RunSpec {
    pub program: std::path::PathBuf,
    pub display_name: String,
    pub args: Vec<String>,
    pub working_dir: String,
    pub envs: Vec<(String, String)>,
    pub selection: Option<String>,
}

impl RunSpec {
    /// Resolve `content` into something spawnable, or explain why it cannot be.
    pub fn resolve(content: &CommandContent, selection: Option<&str>) -> Result<Self, String> {
        let (display_name, args) = split_command(&content.command, selection)?;

        let program = which::which(&display_name)
            .map_err(|_| format!("Command not found: {display_name}"))?;

        Ok(Self {
            program,
            display_name,
            args,
            working_dir: content.working_dir.trim().to_string(),
            envs: parse_environments(&content.environments),
            selection: selection.map(str::to_owned),
        })
    }

    pub fn spawn(&self) -> Result<Child, String> {
        let mut cmd = Command::new(&self.program);

        #[cfg(target_os = "windows")]
        {
            // Otherwise every run flashes a console window.
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        if !self.args.is_empty() {
            cmd.args(&self.args);
        }

        if !self.working_dir.is_empty() {
            cmd.current_dir(&self.working_dir);
        }

        for (key, value) in &self.envs {
            cmd.env(key, value);
        }

        if let Some(selection) = &self.selection {
            cmd.env(SELECTION_ENV_VAR, selection);
        }

        tracing::debug!(program = %self.display_name, args = ?self.args, "Running command");

        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("Failed to start command: {err}"))
    }
}

/// Split a command line into program and arguments, substituting the selection.
///
/// The command is never handed to a shell: it is split with Windows quoting rules, so arguments
/// cannot be reinterpreted as shell syntax. Only arguments get the selection substituted, never
/// the program, so a selection can never redirect the command at a different executable.
pub fn split_command(
    command: &str,
    selection: Option<&str>,
) -> Result<(String, Vec<String>), String> {
    let mut parts = winsplit::split(command);
    if parts.is_empty() {
        return Err("Command cannot be empty".to_string());
    }

    let program = parts.remove(0);
    replace_selection_args(&mut parts, selection);
    Ok((program, parts))
}

/// Start reading a spawned child's pipes.
///
/// Returns the shared child handle (so it can still be killed) and a receiver that yields every
/// output line followed by exactly one [`CmdEvent::Done`].
pub fn pump(mut child: Child) -> (Arc<Mutex<Child>>, Receiver<CmdEvent>) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child = Arc::new(Mutex::new(child));

    let (tx, rx) = mpsc::channel();
    let supervised = child.clone();

    thread::spawn(move || {
        let out_handle = spawn_line_reader(stdout, tx.clone(), CmdEvent::Output);
        let err_handle = spawn_line_reader(stderr, tx.clone(), CmdEvent::Error);

        let success = wait_for_exit(&supervised);

        let _ = tx.send(CmdEvent::Done { success });
        let _ = out_handle.join();
        let _ = err_handle.join();
    });

    (child, rx)
}

fn spawn_line_reader<R: std::io::Read + Send + 'static>(
    source: Option<R>,
    tx: Sender<CmdEvent>,
    wrap: fn(String) -> CmdEvent,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let Some(source) = source else {
            return;
        };
        let reader = std::io::BufReader::new(source);
        for line in std::io::BufRead::lines(reader).map_while(Result::ok) {
            let _ = tx.send(wrap(line));
        }
    })
}

/// Poll the child until it exits.
///
/// IMPORTANT: the mutex is released between polls. Blocking in `wait()` while holding it would
/// stop [`kill`] from ever locking the child, making the stop button useless.
fn wait_for_exit(child: &Arc<Mutex<Child>>) -> bool {
    loop {
        let done = match child.lock() {
            Ok(mut child) => match child.try_wait() {
                Ok(Some(status)) => Some(status.success()),
                Ok(None) => None,
                Err(_) => Some(false),
            },
            Err(_) => Some(false),
        };

        if let Some(success) = done {
            return success;
        }

        thread::sleep(REAP_POLL_INTERVAL);
    }
}

/// Terminate a running command, including anything it spawned.
pub fn kill(child: &mut Child) {
    #[cfg(windows)]
    {
        // `Child::kill()` only terminates the direct process. Grandchildren inherit the stdout and
        // stderr handles, so the pipes stay open and output keeps arriving. `taskkill /T` takes
        // down the whole tree.
        let status = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .status();

        if status.is_err() {
            let _ = child.kill();
        }
    }

    #[cfg(not(windows))]
    {
        let _ = child.kill();
    }
}

/// Kill a shared child handle on a scratch thread, so the caller never blocks on `taskkill`.
pub fn kill_detached(child: Arc<Mutex<Child>>) {
    thread::spawn(move || match child.lock() {
        Ok(mut child) => kill(&mut child),
        Err(err) => {
            tracing::warn!(error = %err, "Failed to lock command process for killing");
        }
    });
}

/// Parse the `KEY=VALUE` per line environment block.
///
/// Splits on the *first* `=` only, so values may contain `=`. A line without one becomes an
/// empty-valued variable, which is how a command is told a flag is present.
pub fn parse_environments(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| match line.split_once('=') {
            Some((key, value)) => (key.trim().to_string(), value.trim().to_string()),
            None => (line.to_string(), String::new()),
        })
        .collect()
}

/// Substitute the captured selection into every argument that mentions it.
pub fn replace_selection_args(args: &mut [String], selection: Option<&str>) {
    let Some(selection) = selection else {
        return;
    };
    for arg in args {
        if arg.contains(SELECTION_PLACEHOLDER) {
            *arg = arg.replace(SELECTION_PLACEHOLDER, selection);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_selection_placeholder_in_arguments() {
        let mut args = vec![
            "--input={{RUSTICKERS_SELECTION}}".to_string(),
            "{{RUSTICKERS_SELECTION}}".to_string(),
            "unchanged".to_string(),
        ];

        replace_selection_args(&mut args, Some("selected text"));

        assert_eq!(
            args,
            ["--input=selected text", "selected text", "unchanged"]
        );
    }

    #[test]
    fn leaves_placeholder_without_selection() {
        let mut args = vec![SELECTION_PLACEHOLDER.to_string()];

        replace_selection_args(&mut args, None);

        assert_eq!(args, [SELECTION_PLACEHOLDER]);
    }

    #[test]
    fn parses_environment_lines() {
        let envs = parse_environments("  FOO = bar \n\n BAZ\nURL=https://x/?a=1\n");

        assert_eq!(
            envs,
            [
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), String::new()),
                ("URL".to_string(), "https://x/?a=1".to_string()),
            ]
        );
    }

    #[test]
    fn splitting_rejects_an_empty_command() {
        assert!(split_command("   ", None).is_err());
    }

    #[test]
    fn splitting_never_substitutes_the_program() {
        let (program, args) = split_command(
            &format!("{SELECTION_PLACEHOLDER} --flag={SELECTION_PLACEHOLDER}"),
            Some("evil.exe"),
        )
        .expect("non empty");

        assert_eq!(program, SELECTION_PLACEHOLDER);
        assert_eq!(args, ["--flag=evil.exe"]);
    }
}
