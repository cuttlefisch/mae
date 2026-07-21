use std::process::Command;
use tracing::{debug, warn};

/// Pull the PATH environment variable from the user's interactive shell.
///
/// When GUI apps are launched from a desktop environment (GNOME, sway, etc.),
/// they inherit a minimal PATH that often lacks `~/.local/bin`, `~/.cargo/bin`,
/// or nvm/pyenv shims. This function spawns the user's shell and asks it to
/// echo its PATH, then returns the result.
///
/// Routed through the crate's single rc-sourcing implementation
/// (`shell_invocation::login_shell_script_argv`, #291) — this used to
/// hand-roll its own `-i -l -c` invocation with no startup-file guard for
/// any shell, and a bash/zsh-vs-fish branch that ran byte-identical code in
/// both arms despite the doc comment implying real per-shell handling.
pub fn pull_path_from_shell() -> Option<String> {
    let shell = std::env::var("SHELL").ok()?;
    debug!(shell, "attempting to pull PATH from shell");

    let (shell, args) = crate::shell_invocation::login_shell_script_argv(&shell, "echo $PATH");
    let output = Command::new(&shell).args(&args).output();

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
