//! External formatter execution via subprocess.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::{ExternalFormatter, FormatResult};

/// Run an external formatter on content.
pub fn run_formatter(
    formatter: &ExternalFormatter,
    content: &str,
    file_path: &Path,
) -> Result<FormatResult, String> {
    let file_str = file_path.to_string_lossy();

    let args: Vec<String> = formatter
        .args
        .iter()
        .map(|a| a.replace("{file}", &file_str))
        .collect();

    let mut cmd = Command::new(&formatter.command);
    cmd.args(&args);

    if formatter.stdin {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn {}: {}", formatter.command, e))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(content.as_bytes())
                .map_err(|e| format!("failed to write stdin: {}", e))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| format!("failed to wait for {}: {}", formatter.command, e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "{} exited with {}: {}",
                formatter.command, output.status, stderr
            ));
        }

        let formatted =
            String::from_utf8(output.stdout).map_err(|e| format!("invalid UTF-8: {}", e))?;

        let changed = formatted != content;
        Ok(FormatResult { formatted, changed })
    } else {
        // File mode: formatter reads/writes the file directly.
        // We write content to a temp file, run the formatter, read back.
        let dir = file_path.parent().unwrap_or(Path::new("."));
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("tmp");
        let tmp_path = dir.join(format!(".mae-format-tmp.{}", ext));

        std::fs::write(&tmp_path, content)
            .map_err(|e| format!("failed to write temp file: {}", e))?;

        // Add the temp file path as last argument
        cmd.arg(&tmp_path);

        let output = cmd
            .output()
            .map_err(|e| format!("failed to run {}: {}", formatter.command, e))?;

        if !output.status.success() {
            let _ = std::fs::remove_file(&tmp_path);
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "{} exited with {}: {}",
                formatter.command, output.status, stderr
            ));
        }

        let formatted = std::fs::read_to_string(&tmp_path)
            .map_err(|e| format!("failed to read formatted file: {}", e))?;
        let _ = std::fs::remove_file(&tmp_path);

        let changed = formatted != content;
        Ok(FormatResult { formatted, changed })
    }
}

/// Check if a formatter command is available on PATH.
pub fn is_available(command: &str) -> bool {
    Command::new("which")
        .arg(command)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Default formatters for common languages.
pub fn default_formatters() -> Vec<(&'static str, ExternalFormatter)> {
    vec![
        (
            "rust",
            ExternalFormatter {
                command: "rustfmt".into(),
                args: vec![],
                stdin: true,
            },
        ),
        (
            "python",
            ExternalFormatter {
                command: "black".into(),
                args: vec!["-".into(), "-q".into()],
                stdin: true,
            },
        ),
        (
            "javascript",
            ExternalFormatter {
                command: "prettier".into(),
                args: vec!["--stdin-filepath".into(), "{file}".into()],
                stdin: true,
            },
        ),
        (
            "typescript",
            ExternalFormatter {
                command: "prettier".into(),
                args: vec!["--stdin-filepath".into(), "{file}".into()],
                stdin: true,
            },
        ),
        (
            "go",
            ExternalFormatter {
                command: "gofmt".into(),
                args: vec![],
                stdin: true,
            },
        ),
        (
            "c",
            ExternalFormatter {
                command: "clang-format".into(),
                args: vec![],
                stdin: true,
            },
        ),
        (
            "cpp",
            ExternalFormatter {
                command: "clang-format".into(),
                args: vec![],
                stdin: true,
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_formatters_not_empty() {
        let fmts = default_formatters();
        assert!(fmts.len() >= 5);
    }

    #[test]
    fn file_placeholder_replacement() {
        let formatter = ExternalFormatter {
            command: "prettier".into(),
            args: vec!["--stdin-filepath".into(), "{file}".into()],
            stdin: true,
        };
        let args: Vec<String> = formatter
            .args
            .iter()
            .map(|a| a.replace("{file}", "/tmp/test.js"))
            .collect();
        assert_eq!(args[1], "/tmp/test.js");
    }

    #[test]
    fn is_available_echo() {
        // `echo` should always be available
        assert!(is_available("echo"));
    }

    #[test]
    fn is_available_nonexistent() {
        assert!(!is_available("mae_nonexistent_command_12345"));
    }
}
