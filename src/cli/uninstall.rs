//! `zeptoclaw uninstall` command — remove ZeptoClaw state and optional binary.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use zeptoclaw::config::Config;

#[derive(Debug, Clone, PartialEq, Eq)]
enum BinaryRemovalPlan {
    Keep,
    Remove(PathBuf),
    Manual {
        reason: String,
        command: Option<String>,
    },
}

pub(crate) async fn cmd_uninstall(remove_binary: bool, yes: bool) -> Result<()> {
    let state_dir = Config::dir();
    let state_exists = state_dir.exists();
    let binary_plan = if remove_binary {
        current_binary_removal_plan()?
    } else {
        BinaryRemovalPlan::Keep
    };

    print_uninstall_plan(&state_dir, state_exists, &binary_plan, remove_binary);

    let will_remove_binary = matches!(&binary_plan, BinaryRemovalPlan::Remove(_));
    let has_destructive_action = state_exists || will_remove_binary;

    if !has_destructive_action {
        println!();
        match binary_plan {
            BinaryRemovalPlan::Keep => {
                println!("Nothing to uninstall.");
            }
            BinaryRemovalPlan::Manual { reason, command } => {
                println!("Binary was not removed automatically: {reason}");
                if let Some(command) = command {
                    println!("Remove it with: {command}");
                }
            }
            BinaryRemovalPlan::Remove(_) => {}
        }
        return Ok(());
    }

    if !yes && !confirm_uninstall()? {
        println!("Uninstall cancelled.");
        return Ok(());
    }

    if state_exists {
        tokio::fs::remove_dir_all(&state_dir)
            .await
            .with_context(|| format!("failed to remove state directory {}", state_dir.display()))?;
        println!("Removed state directory: {}", state_dir.display());
    } else {
        println!("State directory not found: {}", state_dir.display());
    }

    match binary_plan {
        BinaryRemovalPlan::Keep => {
            println!("Kept current binary. Re-run with --remove-binary to delete direct installs.");
        }
        BinaryRemovalPlan::Remove(path) => {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove binary {}", path.display()))?;
            println!("Removed binary: {}", path.display());
        }
        BinaryRemovalPlan::Manual { reason, command } => {
            println!("Binary was not removed automatically: {reason}");
            if let Some(command) = command {
                println!("Remove it with: {command}");
            }
        }
    }

    println!("ZeptoClaw uninstall complete.");
    Ok(())
}

fn confirm_uninstall() -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!("refusing to uninstall in non-interactive mode without --yes");
    }

    print!("Proceed with uninstall? [y/N]: ");
    io::stdout().flush().context("failed to flush stdout")?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read confirmation response")?;

    let input = input.trim();
    Ok(input.eq_ignore_ascii_case("y") || input.eq_ignore_ascii_case("yes"))
}

fn print_uninstall_plan(
    state_dir: &Path,
    state_exists: bool,
    binary_plan: &BinaryRemovalPlan,
    remove_binary: bool,
) {
    println!("ZeptoClaw uninstall");
    println!();
    if state_exists {
        println!("State directory to remove: {}", state_dir.display());
    } else {
        println!("State directory not found: {}", state_dir.display());
    }

    match binary_plan {
        BinaryRemovalPlan::Keep => {
            if remove_binary {
                println!("Binary removal skipped.");
            } else {
                println!("Binary will be kept. Use --remove-binary for direct installs.");
            }
        }
        BinaryRemovalPlan::Remove(path) => {
            println!("Binary to remove: {}", path.display());
        }
        BinaryRemovalPlan::Manual { reason, command } => {
            println!("Binary will not be removed automatically: {reason}");
            if let Some(command) = command {
                println!("Suggested manual command: {command}");
            }
        }
    }
}

fn current_binary_removal_plan() -> Result<BinaryRemovalPlan> {
    let raw = std::env::current_exe().context("failed to determine current executable path")?;
    let resolved = raw.canonicalize().unwrap_or_else(|_| raw.clone());
    Ok(binary_removal_plan_for_paths(&raw, &resolved))
}

fn binary_removal_plan_for_paths(raw: &Path, resolved: &Path) -> BinaryRemovalPlan {
    if is_homebrew_install(raw) || is_homebrew_install(resolved) {
        return BinaryRemovalPlan::Manual {
            reason: format!(
                "the current binary appears to be managed by Homebrew at {}",
                resolved.display()
            ),
            command: Some("brew uninstall qhkm/tap/zeptoclaw".to_string()),
        };
    }

    if is_cargo_install(raw) || is_cargo_install(resolved) {
        return BinaryRemovalPlan::Manual {
            reason: format!(
                "the current binary appears to be managed by cargo-install at {}",
                resolved.display()
            ),
            command: Some("cargo uninstall zeptoclaw".to_string()),
        };
    }

    if is_direct_install_path(raw) {
        return BinaryRemovalPlan::Remove(raw.to_path_buf());
    }

    if is_direct_install_path(resolved) {
        return BinaryRemovalPlan::Remove(resolved.to_path_buf());
    }

    BinaryRemovalPlan::Manual {
        reason: format!(
            "automatic binary removal only supports direct installs in ~/.local/bin or /usr/local/bin (current path: {})",
            resolved.display()
        ),
        command: None,
    }
}

fn is_direct_install_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new("zeptoclaw"))
        && supported_direct_install_dirs()
            .iter()
            .any(|dir| path.starts_with(dir))
}

fn is_cargo_install(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new("zeptoclaw"))
        && dirs::home_dir()
            .map(|home| path.starts_with(home.join(".cargo/bin")))
            .unwrap_or(false)
}

fn is_homebrew_install(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/Cellar/")
        || text.starts_with("/opt/homebrew/bin/zeptoclaw")
        || text.starts_with("/home/linuxbrew/.linuxbrew/bin/zeptoclaw")
}

fn supported_direct_install_dirs() -> Vec<PathBuf> {
    let mut install_dirs = vec![PathBuf::from("/usr/local/bin")];
    if let Some(home) = dirs::home_dir() {
        install_dirs.push(home.join(".local/bin"));
    }
    install_dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_direct_install_in_local_bin_is_removable() {
        let home = dirs::home_dir().expect("home dir");
        let path = home.join(".local/bin/zeptoclaw");
        let plan = binary_removal_plan_for_paths(&path, &path);
        assert_eq!(plan, BinaryRemovalPlan::Remove(path));
    }

    #[test]
    fn plan_direct_install_in_usr_local_bin_is_removable() {
        let path = PathBuf::from("/usr/local/bin/zeptoclaw");
        let plan = binary_removal_plan_for_paths(&path, &path);
        assert_eq!(plan, BinaryRemovalPlan::Remove(path));
    }

    #[test]
    fn plan_homebrew_install_requires_manual_uninstall() {
        let raw = PathBuf::from("/opt/homebrew/bin/zeptoclaw");
        let resolved = PathBuf::from("/opt/homebrew/Cellar/zeptoclaw/0.5.0/bin/zeptoclaw");
        let plan = binary_removal_plan_for_paths(&raw, &resolved);
        match plan {
            BinaryRemovalPlan::Manual { reason, command } => {
                assert!(reason.contains("Homebrew"));
                assert_eq!(
                    command.as_deref(),
                    Some("brew uninstall qhkm/tap/zeptoclaw")
                );
            }
            other => panic!("expected manual plan, got {other:?}"),
        }
    }

    #[test]
    fn plan_cargo_install_requires_manual_uninstall() {
        let home = dirs::home_dir().expect("home dir");
        let path = home.join(".cargo/bin/zeptoclaw");
        let plan = binary_removal_plan_for_paths(&path, &path);
        match plan {
            BinaryRemovalPlan::Manual { reason, command } => {
                assert!(reason.contains("cargo-install"));
                assert_eq!(command.as_deref(), Some("cargo uninstall zeptoclaw"));
            }
            other => panic!("expected manual plan, got {other:?}"),
        }
    }

    #[test]
    fn plan_unknown_binary_path_stays_manual() {
        let path = PathBuf::from("/tmp/zeptoclaw");
        let plan = binary_removal_plan_for_paths(&path, &path);
        match plan {
            BinaryRemovalPlan::Manual { reason, command } => {
                assert!(reason.contains("automatic binary removal only supports"));
                assert!(command.is_none());
            }
            other => panic!("expected manual plan, got {other:?}"),
        }
    }
}
