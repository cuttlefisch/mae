//! Babel execution engine — runs source blocks and captures output.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::session::SessionManager;
use super::{expand_tilde, EvalPolicy, HeaderArgs, SrcBlock};

/// Result of executing a source block.
#[derive(Debug, Clone)]
pub enum ExecResult {
    Output(String),
    Value(String),
    File(PathBuf),
    Error(String),
    /// Scheme blocks need evaluation through the editor runtime.
    PendingSchemeEval(String),
}

/// Babel execution engine with session management.
pub struct BabelExecutor {
    pub sessions: SessionManager,
    pub timeout_secs: u64,
    pub max_output_bytes: usize,
}

impl Default for BabelExecutor {
    fn default() -> Self {
        BabelExecutor {
            sessions: SessionManager::new(),
            timeout_secs: 30,
            max_output_bytes: 100 * 1024, // 100KB
        }
    }
}

impl BabelExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Execute a source block and return the result.
    pub fn execute_block(
        &mut self,
        block: &SrcBlock,
        buf_dir: &Path,
        resolved_vars: &[(String, String)],
    ) -> ExecResult {
        if block.eval_policy() == &EvalPolicy::Never {
            return ExecResult::Error("Execution blocked by :eval never".to_string());
        }

        let working_dir = block
            .header_args
            .dir
            .as_ref()
            .map(|d| PathBuf::from(expand_tilde(d)))
            .unwrap_or_else(|| buf_dir.to_path_buf());

        let body = self.prepare_body(block, resolved_vars);

        match block.language.as_str() {
            "scheme" | "elisp" => ExecResult::PendingSchemeEval(body),
            lang => {
                // Route through session if `:session` header arg is set
                if let Some(session_name) = &block.header_args.session {
                    match self
                        .sessions
                        .get_or_create(lang, session_name, &working_dir)
                    {
                        Ok(session) => {
                            let timeout = Duration::from_secs(self.timeout_secs);
                            match session.execute(&body, timeout) {
                                Ok(output) => ExecResult::Output(output),
                                Err(e) => ExecResult::Error(e),
                            }
                        }
                        Err(e) => ExecResult::Error(e),
                    }
                } else {
                    self.execute_shell(lang, &body, &working_dir, &block.header_args)
                }
            }
        }
    }

    /// Prepare the block body with variable bindings prepended.
    fn prepare_body(&self, block: &SrcBlock, resolved_vars: &[(String, String)]) -> String {
        if resolved_vars.is_empty() {
            return block.body.clone();
        }

        let mut body = String::new();
        for (name, value) in resolved_vars {
            let binding = format_var_binding(&block.language, name, value);
            body.push_str(&binding);
            body.push('\n');
        }
        body.push_str(&block.body);
        body
    }

    /// Execute via shell subprocess.
    fn execute_shell(
        &self,
        language: &str,
        body: &str,
        working_dir: &Path,
        args: &HeaderArgs,
    ) -> ExecResult {
        let (cmd, cmd_args) = resolve_command(language, args);

        let result = Command::new(&cmd)
            .args(&cmd_args)
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("MAE_BABEL", "1")
            .spawn();

        let mut child = match result {
            Ok(c) => c,
            Err(e) => {
                return ExecResult::Error(format!(
                    "{} not found in PATH: {}. Install {} or set :cmd to override.",
                    cmd, e, language
                ));
            }
        };

        // Write body to stdin
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(body.as_bytes());
            // stdin drops here, closing the pipe
        }

        let timeout = Duration::from_secs(self.timeout_secs);
        match child.wait_timeout(timeout) {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(ref mut out) = child.stdout {
                    let _ = out.read_to_string(&mut stdout);
                }
                if let Some(ref mut err) = child.stderr {
                    let _ = err.read_to_string(&mut stderr);
                }

                // Truncate if too large
                if stdout.len() > self.max_output_bytes {
                    stdout.truncate(self.max_output_bytes);
                    stdout.push_str("\n... (output truncated)");
                }

                if !status.success() && !stderr.is_empty() {
                    ExecResult::Error(format!("{}\n{}", stdout, stderr))
                } else if !stderr.is_empty() {
                    // Some programs write to stderr for warnings
                    ExecResult::Output(format!("{}{}", stdout, stderr))
                } else {
                    ExecResult::Output(stdout)
                }
            }
            Ok(None) => {
                let _ = child.kill();
                ExecResult::Error(format!("Execution timed out after {}s", self.timeout_secs))
            }
            Err(e) => ExecResult::Error(format!("Failed to wait for process: {}", e)),
        }
    }

    /// Kill all active sessions.
    pub fn kill_sessions(&mut self) {
        self.sessions.kill_all();
    }
}

/// Resolve the command and arguments for a language.
fn resolve_command(language: &str, args: &HeaderArgs) -> (String, Vec<String>) {
    if let Some(cmd) = &args.cmd {
        return (cmd.clone(), Vec::new());
    }

    match language {
        "python" | "python3" => ("python3".to_string(), Vec::new()),
        "python2" => ("python2".to_string(), Vec::new()),
        "ruby" => ("ruby".to_string(), Vec::new()),
        "perl" => ("perl".to_string(), Vec::new()),
        "bash" | "sh" => ("bash".to_string(), Vec::new()),
        "zsh" => ("zsh".to_string(), Vec::new()),
        "fish" => ("fish".to_string(), Vec::new()),
        "node" | "javascript" | "js" => ("node".to_string(), Vec::new()),
        "lua" => ("lua".to_string(), Vec::new()),
        "R" | "r" => ("Rscript".to_string(), vec!["--vanilla".to_string()]),
        "go" => (
            "go".to_string(),
            vec!["run".to_string(), "/dev/stdin".to_string()],
        ),
        "rust" => {
            // For Rust, we'd need to write to temp file — handled specially
            (
                "rustc".to_string(),
                vec![
                    "-".to_string(),
                    "-o".to_string(),
                    "/tmp/mae-babel-rust".to_string(),
                ],
            )
        }
        _ => (language.to_string(), Vec::new()),
    }
}

/// Format a variable binding in the target language.
fn format_var_binding(language: &str, name: &str, value: &str) -> String {
    match language {
        "python" | "python3" | "python2" => {
            if value.parse::<f64>().is_ok() {
                format!("{} = {}", name, value)
            } else {
                format!("{} = \"{}\"", name, value.replace('\"', "\\\""))
            }
        }
        "ruby" => {
            if value.parse::<f64>().is_ok() {
                format!("{} = {}", name, value)
            } else {
                format!("{} = \"{}\"", name, value.replace('\"', "\\\""))
            }
        }
        "bash" | "sh" | "zsh" | "fish" => {
            format!("{}=\"{}\"", name, value.replace('\"', "\\\""))
        }
        "node" | "javascript" | "js" => {
            if value.parse::<f64>().is_ok() {
                format!("const {} = {};", name, value)
            } else {
                format!("const {} = \"{}\";", name, value.replace('\"', "\\\""))
            }
        }
        _ => format!("# var: {} = {}", name, value),
    }
}

impl SrcBlock {
    pub fn eval_policy(&self) -> &EvalPolicy {
        &self.header_args.eval
    }
}

/// Trait for `wait_timeout` on Child (mirrors wait-timeout crate).
trait WaitTimeout {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl WaitTimeout for std::process::Child {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let start = std::time::Instant::now();
        loop {
            match self.try_wait()? {
                Some(status) => return Ok(Some(status)),
                None => {
                    if start.elapsed() >= timeout {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_python_command() {
        let args = HeaderArgs::default();
        let (cmd, _) = resolve_command("python", &args);
        assert_eq!(cmd, "python3");
    }

    #[test]
    fn resolve_custom_cmd() {
        let args = HeaderArgs {
            cmd: Some("/usr/local/bin/python3.11".to_string()),
            ..HeaderArgs::default()
        };
        let (cmd, _) = resolve_command("python", &args);
        assert_eq!(cmd, "/usr/local/bin/python3.11");
    }

    #[test]
    fn format_python_var_number() {
        let result = format_var_binding("python", "x", "42");
        assert_eq!(result, "x = 42");
    }

    #[test]
    fn format_python_var_string() {
        let result = format_var_binding("python", "name", "hello");
        assert_eq!(result, "name = \"hello\"");
    }

    #[test]
    fn format_bash_var() {
        let result = format_var_binding("bash", "DIR", "/tmp");
        assert_eq!(result, "DIR=\"/tmp\"");
    }

    #[test]
    fn execute_echo() {
        let mut executor = BabelExecutor::new();
        let block = SrcBlock {
            name: None,
            language: "bash".to_string(),
            header_args: HeaderArgs::default(),
            body: "echo hello".to_string(),
            line_range: (0, 2),
            body_byte_range: (0, 10),
        };
        let result = executor.execute_block(&block, Path::new("/tmp"), &[]);
        match result {
            ExecResult::Output(s) => assert_eq!(s.trim(), "hello"),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn execute_python_print() {
        let mut executor = BabelExecutor::new();
        let block = SrcBlock {
            name: None,
            language: "python".to_string(),
            header_args: HeaderArgs::default(),
            body: "print(2 + 2)".to_string(),
            line_range: (0, 2),
            body_byte_range: (0, 12),
        };
        let result = executor.execute_block(&block, Path::new("/tmp"), &[]);
        match result {
            ExecResult::Output(s) => assert_eq!(s.trim(), "4"),
            ExecResult::Error(e) if e.contains("not found") => {
                // python3 not installed, skip
            }
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn execute_eval_never_blocked() {
        let mut executor = BabelExecutor::new();
        let args = HeaderArgs {
            eval: EvalPolicy::Never,
            ..HeaderArgs::default()
        };
        let block = SrcBlock {
            name: None,
            language: "bash".to_string(),
            header_args: args,
            body: "echo should not run".to_string(),
            line_range: (0, 2),
            body_byte_range: (0, 0),
        };
        let result = executor.execute_block(&block, Path::new("/tmp"), &[]);
        match result {
            ExecResult::Error(e) => assert!(e.contains("blocked")),
            other => panic!("Expected Error, got {:?}", other),
        }
    }
}
