//! `rst vault` — the master password behind encrypted clipboard history.
//!
//! The vault itself lives in the same `clipboard.db` the desktop app uses, so a password created
//! here works there and vice versa. CLI authorization is deliberately per-invocation: nothing
//! unlocked here unlocks the desktop app or any later command.

use anyhow::{Result, bail};
use serde_json::json;
use zeroize::Zeroizing;

use super::clipboard_terminal::password;
use super::cp::{ExitCode, open_store};
use crate::cli::{
    output::Format,
    runtime::block_on,
};
use crate::clipboard::vault_controller::{VaultController, VaultState};
use crate::storage::paths::AppPaths;

#[derive(clap::Args, Debug)]
#[command(
    about = "Manage the encrypted clipboard vault",
    after_help = "Examples:\n  rst vault create                       choose a master password (hidden prompts)\n  rst vault status                     what exists, how CLI authorization works\n\nThe password is only ever typed on the terminal: never argv, stdin, or the environment.\nProtected commands prompt for it when they need it, for that command only.\nThe master password cannot be reset — forgetting it makes protected items unreadable."
)]
pub struct VaultArgs {
    #[command(subcommand)]
    pub action: VaultAction,
}

#[derive(clap::Subcommand, Debug)]
pub enum VaultAction {
    /// Choose a master password; never overwrites an existing vault
    Create,

    /// Show whether a vault exists and how CLI authorization works
    Status,
}

/// Prompt for a password on the controlling terminal; cancellation reports exit code 130.
pub(super) fn require_password(prompt: &str) -> Result<Zeroizing<String>> {
    password(prompt)?.ok_or_else(|| ExitCode(130).into())
}

/// Unlock `controller` for this invocation, prompting once. The shared gate for every command
/// that needs to read or write protected history (`cp add --protect`, `cp show --id`, ...).
pub(super) fn unlock(controller: &VaultController) -> Result<()> {
    if block_on(controller.envelope())?.is_none() {
        bail!("no master password configured; run `rst vault create` first");
    }
    let password = require_password("Master password: ")?;
    if !block_on(controller.unlock(&password))? {
        bail!("incorrect master password; nothing was changed");
    }
    Ok(())
}

pub fn run(paths: &AppPaths, args: VaultArgs, format: Format) -> Result<()> {
    let controller = VaultController::new(open_store(paths)?);
    match args.action {
        VaultAction::Status => {
            let configured = block_on(controller.envelope())?.is_some();
            let state = if configured { "locked" } else { "unconfigured" };
            format.emit(
                json!({"state": state, "has_master_password": configured, "scope": "this CLI invocation"}),
                || {
                    if configured {
                        println!(
                            "Vault: configured. Commands that touch protected history prompt \
                             for the password, for that run only."
                        );
                    } else {
                        println!(
                            "Vault: not configured. Run `rst vault create`, or protect an item \
                             in the desktop app."
                        );
                    }
                },
            );
        }
        VaultAction::Create => {
            if !matches!(block_on(controller.snapshot()), VaultState::Unconfigured) {
                bail!(
                    "master password already configured; change it in the desktop app (existing \
                     ciphertext must keep its data key)"
                );
            }
            let password = require_password("New master password: ")?;
            if password.is_empty() {
                bail!("master password must not be empty");
            }
            let confirm = require_password("Confirm password: ")?;
            if password != confirm {
                bail!("passwords do not match; nothing changed");
            }
            block_on(controller.create(&password))?;
            format.emit(json!({"state": "locked"}), || {
                println!(
                    "Master password created. Protected commands will prompt for it when needed; \
                     it cannot be recovered or reset."
                )
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(subcommand)]
        command: TestCommand,
    }

    #[derive(clap::Subcommand, Debug)]
    enum TestCommand {
        Vault(VaultArgs),
    }

    #[test]
    fn every_action_parses() {
        // First element is argv[0], then `vault <ACTION>`.
        for action in ["create", "status"] {
            let parsed = TestCli::try_parse_from(["rst", "vault", action]).unwrap();
            let TestCommand::Vault(args) = parsed.command else {
                panic!()
            };
            let _ = args.action;
        }
        assert!(TestCli::try_parse_from(["rst", "vault"]).is_err());
        assert!(TestCli::try_parse_from(["rst", "vault", "delete-everything"]).is_err());
        // Removed actions must no longer parse; unlocking happens per command, not on demand.
        assert!(TestCli::try_parse_from(["rst", "vault", "unlock"]).is_err());
        assert!(TestCli::try_parse_from(["rst", "vault", "lock"]).is_err());
    }

    #[test]
    fn status_works_on_an_unconfigured_vault() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::storage::paths::AppPaths {
            db_path: tmp.path().join("stickers.db"),
        };
        run(
            &paths,
            VaultArgs {
                action: VaultAction::Status,
            },
            Format::Json,
        )
        .unwrap();
    }

    #[test]
    fn create_refuses_to_overwrite_an_existing_vault() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::storage::paths::AppPaths {
            db_path: tmp.path().join("stickers.db"),
        };
        let controller = VaultController::new(open_store(&paths).unwrap());
        block_on(controller.create("someone-elses-password")).unwrap();
        // The refusal must happen before any terminal prompt.
        let error = run(
            &paths,
            VaultArgs {
                action: VaultAction::Create,
            },
            Format::Json,
        )
        .unwrap_err();
        assert!(error.to_string().contains("already configured"));
    }

    #[test]
    fn unlock_without_a_vault_points_at_create() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::storage::paths::AppPaths {
            db_path: tmp.path().join("stickers.db"),
        };
        let controller = VaultController::new(open_store(&paths).unwrap());
        let error = unlock(&controller).unwrap_err();
        assert!(error.to_string().contains("rst vault create"));
    }
}
