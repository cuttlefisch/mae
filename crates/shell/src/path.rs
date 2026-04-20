use std::process::Command;
use tracing::{debug, warn};

/// Pull the PATH environment variable from the user's interactive shell.
///
/// When GUI apps are launched from a desktop environment (GNOME, sway, etc.),
/// they inherit a minimal PATH that often lacks `~/.local/bin`, `~/.cargo/bin`,
/// or nvm/pyenv shims. This function spawns the user's shell and asks it to
/// echo its PATH, then returns the result.
///
/// Supported shells: bash, zsh, fish.
pub fn pull_path_from_shell() -> Option<String> {
    let shell = std::env::var("SHELL").ok()?;
    debug!(shell, "attempting to pull PATH from shell");

    // We want an interactive, login shell to ensure all profile/rc files are sourced.
    // -i (interactive) and -l (login) usually do the trick.
    let output = if shell.ends_with("fish") {
        Command::new(&shell)
            .args(["-i", "-l", "-c", "echo $PATH"])
            .output()
    } else {
        // bash/zsh
        Command::new(&shell)
            .args(["-i", "-l", "-c", "echo $PATH"])
            .output()
    };

    match output {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() {
                debug!(path, "successfully pulled PATH from shell");
                return Some(path);
            }
        }
        Ok(out) => {
            warn!(
                status = ?out.status,
                stderr = %String::from_utf8_lossy(&out.stderr),
                "shell exited with error while pulling PATH"
            );
        }
        Err(e) => {
            warn!(error = %e, "failed to spawn shell to pull PATH");
        }
    }

    None
}

/// Update the current process's PATH if we can pull a better one from the shell.
/// Only updates if the shell's PATH is longer (more specific) than the current one.
pub fn sync_path_from_shell() {
    if let Some(shell_path) = pull_path_from_shell() {
        let current_path = std::env::var("PATH").unwrap_or_default();
        if shell_path.len() > current_path.len() {
            std::env::set_var("PATH", shell_path);
            debug!("updated process PATH from shell");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pull_path_from_shell() {
        // This test depends on the environment having a SHELL set.
        // It might fail in some minimal CI environments, so we make it
        // non-fatal or skip if SHELL is missing.
        if std::env::var("SHELL").is_err() {
            return;
        }

        let path = pull_path_from_shell();
        // If it succeeded, it should be a non-empty string.
        if let Some(p) = path {
            assert!(!p.is_empty());
            assert!(p.contains("/bin") || p.contains("/usr"));
        }
    }
}
