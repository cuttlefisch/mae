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

/// Validate that a write path lands under `sandbox_dir`, judged on where the
/// write really goes (`crate::path_confine`): no `..`, the deepest existing
/// ancestor resolved through every symlink, and never through a link at the
/// path itself. Returns the path to write on success, or an error message.
pub fn validate_write_path(path: &str, sandbox_dir: &Path) -> Result<PathBuf, String> {
    use crate::path_confine::{contained, is_symlink, Escape};
    let outside = |why: String| {
        Err(format!(
            "Path '{}' is outside the test sandbox ({}): {why}",
            path,
            sandbox_dir.display()
        ))
    };
    match contained(sandbox_dir, &expand_tilde(path)) {
        Ok(out) if is_symlink(&out) => outside("it is a symbolic link".to_string()),
        Ok(out) => Ok(out),
        Err(Escape::ParentDir) => outside("'..' is not allowed".to_string()),
        Err(Escape::Outside(p)) => outside(format!("it resolves to {}", p.display())),
        Err(Escape::Unresolvable(e)) => outside(format!("cannot resolve {e}")),
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

    /// A link at the path itself is never written through, even one to a
    /// file inside the sandbox: the write would land wherever it points.
    #[cfg(unix)]
    #[test]
    fn validate_path_through_a_final_symlink_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let sandbox = tmp.path().join("sandbox");
        std::fs::create_dir_all(&sandbox).unwrap();
        std::fs::write(sandbox.join("real.txt"), "x").unwrap();
        std::os::unix::fs::symlink(sandbox.join("real.txt"), sandbox.join("link.txt")).unwrap();
        assert!(validate_write_path("real.txt", &sandbox).is_ok());
        let err = validate_write_path("link.txt", &sandbox).unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
    }

    #[test]
    fn validate_path_traversal_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let sandbox = tmp.path().join("sandbox");
        std::fs::create_dir_all(&sandbox).unwrap();
        // Through a directory that exists nowhere, so only the check decides.
        // A traversal that happened to land on a real directory (`/etc` from
        // `/tmp`) was refused by luck, whatever TMPDIR was (#967).
        let absent = format!("absent-{}", uuid::Uuid::new_v4().simple());
        for p in [
            format!("../{absent}/file"),
            format!("{}/../{absent}/file", sandbox.display()),
        ] {
            let result = validate_write_path(&p, &sandbox);
            assert!(result.is_err(), "{p} must be refused: {result:?}");
            assert!(!tmp.path().join(&absent).exists());
        }
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
