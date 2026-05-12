//! Persistent REPL sessions for babel source blocks.
//!
//! Each session holds a child process with piped stdin/stdout. Output extraction
//! uses a sentinel-based approach (similar to Jupyter kernels): we wrap the user's
//! code with unique marker outputs and read between the sentinels.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// A persistent REPL session.
pub struct Session {
    child: Child,
    stdout_rx: mpsc::Receiver<String>,
    language: String,
    name: String,
    _dir: PathBuf,
}

/// Session registry keyed by (language, session_name).
#[derive(Default)]
pub struct SessionManager {
    sessions: HashMap<(String, String), Session>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or create a session for the given language and name.
    pub fn get_or_create(
        &mut self,
        language: &str,
        name: &str,
        dir: &Path,
    ) -> Result<&mut Session, String> {
        let key = (language.to_string(), name.to_string());
        if !self.sessions.contains_key(&key) {
            let session = Session::new(language, name, dir)?;
            self.sessions.insert(key.clone(), session);
        }
        Ok(self.sessions.get_mut(&key).unwrap())
    }

    /// Kill all active sessions.
    pub fn kill_all(&mut self) {
        for (_, mut session) in self.sessions.drain() {
            let _ = session.child.kill();
        }
    }

    /// Number of active sessions.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

impl Session {
    /// Spawn a new REPL process for the given language.
    pub fn new(language: &str, name: &str, dir: &Path) -> Result<Self, String> {
        let (cmd, args) = repl_command(language)?;

        let mut child = Command::new(&cmd)
            .args(&args)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("MAE_BABEL_SESSION", "1")
            .spawn()
            .map_err(|e| format!("Failed to spawn {} REPL: {}", language, e))?;

        // Set up a reader thread for stdout
        let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
        let (tx, rx) = mpsc::channel();

        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Session {
            child,
            stdout_rx: rx,
            language: language.to_string(),
            name: name.to_string(),
            _dir: dir.to_path_buf(),
        })
    }

    /// Execute code in the session and return captured output.
    /// Uses sentinel-based output capture: wraps code with unique markers.
    pub fn execute(&mut self, code: &str, timeout: Duration) -> Result<String, String> {
        let sentinel = format!("__MAE_SENTINEL_{}__", std::process::id());

        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or("Session stdin not available")?;

        // Build the wrapped code with sentinel markers
        let wrapped = wrap_with_sentinels(&self.language, code, &sentinel);

        stdin
            .write_all(wrapped.as_bytes())
            .map_err(|e| format!("Failed to write to session: {}", e))?;
        stdin
            .flush()
            .map_err(|e| format!("Failed to flush session: {}", e))?;

        // Read output until we see the end sentinel
        let end_sentinel = format!("{}_END", sentinel);
        let start_sentinel = format!("{}_START", sentinel);
        let deadline = std::time::Instant::now() + timeout;
        let mut output_lines = Vec::new();
        let mut capturing = false;

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "Session execution timed out after {}s",
                    timeout.as_secs()
                ));
            }

            match self
                .stdout_rx
                .recv_timeout(remaining.min(Duration::from_millis(100)))
            {
                Ok(line) => {
                    if line.contains(&end_sentinel) {
                        break;
                    }
                    if line.contains(&start_sentinel) {
                        capturing = true;
                        continue;
                    }
                    if capturing {
                        output_lines.push(line);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Session process terminated unexpectedly".to_string());
                }
            }
        }

        Ok(output_lines.join("\n"))
    }

    /// Get the session name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the session language.
    pub fn language(&self) -> &str {
        &self.language
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Resolve the REPL command for a language.
fn repl_command(language: &str) -> Result<(String, Vec<String>), String> {
    match language {
        "python" | "python3" => Ok((
            "python3".to_string(),
            vec!["-u".to_string(), "-i".to_string()],
        )),
        "python2" => Ok((
            "python2".to_string(),
            vec!["-u".to_string(), "-i".to_string()],
        )),
        "ruby" => Ok(("irb".to_string(), vec!["--noecho".to_string()])),
        "node" | "javascript" | "js" => Ok(("node".to_string(), vec!["-i".to_string()])),
        "bash" | "sh" => Ok((
            "bash".to_string(),
            vec!["--norc".to_string(), "-i".to_string()],
        )),
        "zsh" => Ok(("zsh".to_string(), vec!["-i".to_string()])),
        "lua" => Ok(("lua".to_string(), vec!["-i".to_string()])),
        "R" | "r" => Ok((
            "R".to_string(),
            vec!["--vanilla".to_string(), "--interactive".to_string()],
        )),
        _ => Err(format!("No REPL command known for language: {}", language)),
    }
}

/// Wrap code with sentinel markers for the given language.
fn wrap_with_sentinels(language: &str, code: &str, sentinel: &str) -> String {
    let start = format!("{}_START", sentinel);
    let end = format!("{}_END", sentinel);

    match language {
        "python" | "python3" | "python2" => {
            format!("print(\"{}\")\n{}\nprint(\"{}\")\n", start, code, end)
        }
        "ruby" => {
            format!("puts \"{}\"\n{}\nputs \"{}\"\n", start, code, end)
        }
        "node" | "javascript" | "js" => {
            format!(
                "console.log(\"{}\")\n{}\nconsole.log(\"{}\")\n",
                start, code, end
            )
        }
        "bash" | "sh" | "zsh" => {
            format!("echo '{}'\n{}\necho '{}'\n", start, code, end)
        }
        "lua" => {
            format!("print(\"{}\")\n{}\nprint(\"{}\")\n", start, code, end)
        }
        "R" | "r" => {
            format!("cat(\"{}\\n\")\n{}\ncat(\"{}\\n\")\n", start, code, end)
        }
        _ => {
            // Fallback: use echo (shell-like)
            format!("echo '{}'\n{}\necho '{}'\n", start, code, end)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repl_command_python() {
        let (cmd, args) = repl_command("python").unwrap();
        assert_eq!(cmd, "python3");
        assert!(args.contains(&"-i".to_string()));
    }

    #[test]
    fn repl_command_bash() {
        let (cmd, _) = repl_command("bash").unwrap();
        assert_eq!(cmd, "bash");
    }

    #[test]
    fn repl_command_unknown() {
        assert!(repl_command("cobol").is_err());
    }

    #[test]
    fn wrap_python_sentinels() {
        let wrapped = wrap_with_sentinels("python", "print(42)", "SENT");
        assert!(wrapped.contains("print(\"SENT_START\")"));
        assert!(wrapped.contains("print(42)"));
        assert!(wrapped.contains("print(\"SENT_END\")"));
    }

    #[test]
    fn wrap_bash_sentinels() {
        let wrapped = wrap_with_sentinels("bash", "echo hello", "SENT");
        assert!(wrapped.contains("echo 'SENT_START'"));
        assert!(wrapped.contains("echo hello"));
        assert!(wrapped.contains("echo 'SENT_END'"));
    }

    #[test]
    fn session_manager_empty() {
        let mgr = SessionManager::new();
        assert!(mgr.is_empty());
        assert_eq!(mgr.len(), 0);
    }

    #[test]
    fn bash_session_create_and_execute() {
        let mut session = match Session::new("bash", "test", Path::new("/tmp")) {
            Ok(s) => s,
            Err(_) => return, // bash not available, skip
        };

        let result = session.execute("echo hello_world", Duration::from_secs(5));
        match result {
            Ok(output) => assert!(output.contains("hello_world"), "output: {}", output),
            Err(e) => panic!("Session execute failed: {}", e),
        }
    }

    #[test]
    fn bash_session_state_persists() {
        let mut session = match Session::new("bash", "state-test", Path::new("/tmp")) {
            Ok(s) => s,
            Err(_) => return,
        };

        // Set a variable in first execution
        let _ = session.execute("MY_VAR=persistent_value", Duration::from_secs(5));

        // Read it in second execution
        let result = session.execute("echo $MY_VAR", Duration::from_secs(5));
        match result {
            Ok(output) => assert!(
                output.contains("persistent_value"),
                "State not persisted: {}",
                output
            ),
            Err(e) => panic!("Session execute failed: {}", e),
        }
    }

    #[test]
    fn session_manager_get_or_create() {
        let mut mgr = SessionManager::new();
        let result = mgr.get_or_create("bash", "test-mgr", Path::new("/tmp"));
        match result {
            Ok(session) => {
                assert_eq!(session.name(), "test-mgr");
                assert_eq!(session.language(), "bash");
            }
            Err(_) => return, // bash not available
        }
        assert_eq!(mgr.len(), 1);

        // Getting same session should reuse
        let _ = mgr.get_or_create("bash", "test-mgr", Path::new("/tmp"));
        assert_eq!(mgr.len(), 1);
    }
}
