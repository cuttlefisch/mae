//! Filesystem sandbox for test execution.
//!
//! Provides path confinement during self-test and model exam runs so that
//! AI tool calls cannot write outside a temporary directory.

use std::path::{Path, PathBuf};

/// A temporary directory used to confine file writes during test execution.
pub struct TestSandbox {
    /// Writable temporary directory (e.g. `/tmp/mae-test-{pid}-{ts}`).
    pub dir: PathBuf,
    /// Read-only reference to the real project root.
    pub project_root: PathBuf,
}

/// Create a fresh sandbox directory under the system temp dir.
pub fn create_test_sandbox(project_root: &Path) -> TestSandbox {
    let pid = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let dir = std::env::temp_dir().join(format!("mae-test-{pid}-{ts}"));
    let _ = std::fs::create_dir_all(&dir);
    TestSandbox {
        dir,
        project_root: project_root.to_path_buf(),
    }
}

/// Validate that a write path resolves to a location under `sandbox_dir`.
/// Returns the canonicalized path on success, or an error message.
pub fn validate_write_path(path: &str, sandbox_dir: &Path) -> Result<PathBuf, String> {
    let expanded = expand_tilde(path);
    let resolved = if expanded.is_absolute() {
        expanded
    } else {
        sandbox_dir.join(&expanded)
    };

    // Canonicalize the parent (the file itself may not exist yet).
    let parent = resolved.parent().unwrap_or(&resolved);
    let canon_parent = if parent.exists() {
        parent
            .canonicalize()
            .map_err(|e| format!("Cannot resolve path: {e}"))?
    } else {
        // Parent doesn't exist — check it's logically under sandbox.
        parent.to_path_buf()
    };

    let canon_sandbox = if sandbox_dir.exists() {
        sandbox_dir
            .canonicalize()
            .unwrap_or_else(|_| sandbox_dir.to_path_buf())
    } else {
        sandbox_dir.to_path_buf()
    };

    if canon_parent.starts_with(&canon_sandbox) {
        Ok(resolved)
    } else {
        Err(format!(
            "Path '{}' is outside the test sandbox ({})",
            path,
            sandbox_dir.display()
        ))
    }
}

/// Filter a shell command for safety during test mode.
/// Blocks path traversal, sudo, chmod, chown. Prepends `cd {sandbox_dir}` for
/// relative commands.
pub fn filter_shell_command(cmd: &str, sandbox_dir: &Path) -> Result<String, String> {
    let blocked = [
        "sudo ", "sudo\t", "chmod ", "chown ", "rm -rf /", "rm -fr /", "mkfs.", "dd if=", ":(){",
        ">(){ :",
    ];
    for pattern in &blocked {
        if cmd.contains(pattern) {
            return Err(format!(
                "Command blocked during test mode: contains '{pattern}'"
            ));
        }
    }

    // Block path traversal that escapes sandbox.
    if cmd.contains("../") || cmd.contains("..\\") {
        // Allow `..` only if it doesn't escape sandbox. Since we can't easily
        // resolve shell expansions, be conservative and block all `..` usage.
        return Err("Command blocked during test mode: path traversal '..' not allowed".into());
    }

    // Prepend cd to sandbox for relative paths.
    Ok(format!("cd {} && {}", sandbox_dir.display(), cmd))
}

/// Remove the sandbox directory. Best-effort — logs errors but doesn't fail.
pub fn cleanup_sandbox(sandbox_dir: &Path) {
    if sandbox_dir.exists() && sandbox_dir.starts_with(std::env::temp_dir()) {
        if let Err(e) = std::fs::remove_dir_all(sandbox_dir) {
            eprintln!("Warning: failed to clean up test sandbox: {e}");
        }
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_path_inside_sandbox() {
        let sandbox = std::env::temp_dir().join("mae-sandbox-test-valid");
        std::fs::create_dir_all(&sandbox).unwrap();
        let result = validate_write_path("test.txt", &sandbox);
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn validate_path_outside_sandbox_rejected() {
        let sandbox = std::env::temp_dir().join("mae-sandbox-test-reject");
        std::fs::create_dir_all(&sandbox).unwrap();
        let result = validate_write_path("/etc/passwd", &sandbox);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("outside the test sandbox"));
        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn validate_path_traversal_rejected() {
        let sandbox = std::env::temp_dir().join("mae-sandbox-test-traversal");
        std::fs::create_dir_all(&sandbox).unwrap();
        let result = validate_write_path("../../../etc/passwd", &sandbox);
        // Should fail because canonicalized parent won't be under sandbox.
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn filter_blocks_sudo() {
        let sandbox = std::env::temp_dir().join("mae-sandbox-test-sudo");
        let result = filter_shell_command("sudo rm -rf /", &sandbox);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blocked"));
    }

    #[test]
    fn filter_blocks_path_traversal() {
        let sandbox = std::env::temp_dir().join("mae-sandbox-test-dotdot");
        let result = filter_shell_command("cat ../../../etc/passwd", &sandbox);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("path traversal"));
    }

    #[test]
    fn filter_blocks_rm_rf_slash() {
        let sandbox = std::env::temp_dir().join("mae-sandbox-test-rmrf");
        let result = filter_shell_command("rm -rf /", &sandbox);
        assert!(result.is_err());
    }

    #[test]
    fn filter_allows_safe_command() {
        let sandbox = std::env::temp_dir().join("mae-sandbox-test-safe");
        let result = filter_shell_command("echo hello", &sandbox);
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert!(cmd.contains("cd"));
        assert!(cmd.contains("echo hello"));
    }

    #[test]
    fn create_and_cleanup_sandbox() {
        let sandbox = create_test_sandbox(Path::new("/tmp"));
        assert!(sandbox.dir.exists());
        cleanup_sandbox(&sandbox.dir);
        assert!(!sandbox.dir.exists());
    }
}
