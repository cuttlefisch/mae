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
    let outside = |why: String| {
        Err(format!(
            "Path '{}' is outside the test sandbox ({}): {why}",
            path,
            sandbox_dir.display()
        ))
    };
    // The same expansion the effect applies (`execute_create_file`,
    // `rename_file`): a path the guard reads differently from the write is not
    // confined. And absolute only: the effect resolves a relative path against
    // the process's working directory, not the sandbox, so a relative path the
    // guard judged as `<sandbox>/x` was written to `<cwd>/x`.
    let expanded = PathBuf::from(mae_core::file_picker::expand_tilde(path));
    if !expanded.is_absolute() {
        return outside(format!(
            "give an absolute path under {}; a relative path is written outside it",
            sandbox_dir.display()
        ));
    }
    crate::path_confine::confined_write(sandbox_dir, &expanded).or_else(outside)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_path_inside_sandbox() {
        let sandbox = std::env::temp_dir().join("mae-sandbox-test-valid");
        std::fs::create_dir_all(&sandbox).unwrap();
        let inside = sandbox.join("test.txt");
        let result = validate_write_path(inside.to_str().unwrap(), &sandbox);
        assert!(result.is_ok(), "{result:?}");
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
        let real = sandbox.join("real.txt").display().to_string();
        assert!(validate_write_path(&real, &sandbox).is_ok());
        let link = sandbox.join("link.txt").display().to_string();
        let err = validate_write_path(&link, &sandbox).unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
    }

    /// The effect writes a relative path against the process's working
    /// directory, not the sandbox, and expands a bare `~` to `$HOME`. Neither
    /// may pass the guard, or the write lands outside it.
    #[test]
    fn validate_path_relative_or_home_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let sandbox = tmp.path().join("sandbox");
        std::fs::create_dir_all(&sandbox).unwrap();
        for p in [
            "test.txt",
            "./test.txt",
            "sub/test.txt",
            "~",
            "~other/test.txt",
        ] {
            let err = validate_write_path(p, &sandbox).unwrap_err();
            assert!(err.contains("outside the test sandbox"), "{p}: {err}");
        }
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
