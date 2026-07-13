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
    stderr_rx: mpsc::Receiver<String>,
    language: String,
    name: String,
    _dir: PathBuf,
}

/// Session registry keyed by (language, session_name).
pub struct SessionManager {
    sessions: HashMap<(String, String), Session>,
    /// Whether newly-created sessions inherit the user's resolved shell
    /// environment (see `crate::shell_env`) — set once from the editor's
    /// `babel_inherit_shell_env` option via `BabelExecutor`.
    pub shell_env_enabled: bool,
}

impl Default for SessionManager {
    fn default() -> Self {
        SessionManager {
            sessions: HashMap::new(),
            shell_env_enabled: true,
        }
    }
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
            let session = Session::new(language, name, dir, self.shell_env_enabled)?;
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
    pub fn new(
        language: &str,
        name: &str,
        dir: &Path,
        shell_env_enabled: bool,
    ) -> Result<Self, String> {
        let (cmd, args) = repl_command(language)?;

        let mut command = Command::new(&cmd);
        command
            .args(&args)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::shell_env::apply_to(&mut command, shell_env_enabled);
        let mut child = command
            .env("MAE_BABEL_SESSION", "1")
            .spawn()
            .map_err(|e| format!("Failed to spawn {} REPL: {}", language, e))?;

        // Set up a reader thread for stdout.
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

        // Set up a reader thread for stderr too — a persistent interactive
        // REPL's uncaught-exception tracebacks go to stderr, not stdout
        // (confirmed empirically for `python3 -i`), and every downstream
        // statement in the same block silently fails afterward. Without
        // this, that error is discarded entirely (the OS pipe still fills,
        // just never drained) and `execute()` looks like it "succeeded"
        // with empty output — indistinguishable from a genuinely empty
        // result. Mirrors the one-shot execution path's existing stdout+
        // stderr capture (`execute.rs`), which never had this gap since it
        // runs the process to completion and joins both reader threads.
        let stderr = child.stderr.take().ok_or("Failed to capture stderr")?;
        let (stderr_tx, stderr_rx) = mpsc::channel();

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if stderr_tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Drain the interpreter's one-time startup banner (version string,
        // copyright notice, etc. — e.g. Python's "Python 3.x.y ... Type
        // help...") from stderr now, before any `execute()` call, so it's
        // never mistaken for a block's error output later. Bounded wait
        // (chunked, not a single fixed sleep) that stops as soon as
        // stderr goes quiet for one beat — a few tens of ms in practice,
        // paid once per session creation, not per block execution.
        let banner_deadline = std::time::Instant::now() + Duration::from_millis(300);
        loop {
            let remaining = banner_deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match stderr_rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(_) => continue,
                Err(_) => break,
            }
        }

        Ok(Session {
            child,
            stdout_rx: rx,
            stderr_rx,
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

        // Drain whatever stderr this execution produced. An interactive
        // REPL is unbuffered here (`-u` for Python), and any exception a
        // statement raised was written to stderr *before* the interpreter
        // moved on to execute the next statement (which is what allowed
        // the stdout end-sentinel above to be reached at all) — so by the
        // time END has arrived on stdout, this execution's stderr is
        // already sitting in the pipe. Bounded chunked wait (not a single
        // fixed sleep) to give the reader thread a moment to catch up,
        // stopping as soon as stderr goes quiet for one beat.
        let mut stderr_lines = Vec::new();
        let stderr_deadline = std::time::Instant::now() + Duration::from_millis(100);
        loop {
            let remaining = stderr_deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self
                .stderr_rx
                .recv_timeout(remaining.min(Duration::from_millis(25)))
            {
                Ok(line) => {
                    let cleaned = strip_repl_prompts(&line);
                    if !cleaned.is_empty() {
                        stderr_lines.push(cleaned);
                    }
                }
                Err(_) => break,
            }
        }

        let stdout_output = output_lines.join("\n");
        if stderr_lines.is_empty() {
            Ok(stdout_output)
        } else {
            let stderr_output = stderr_lines.join("\n");
            if stdout_output.is_empty() {
                Ok(stderr_output)
            } else {
                Ok(format!("{}\n{}", stdout_output, stderr_output))
            }
        }
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
///
/// A blank line always separates `code` from the end-sentinel print. This
/// is required, not cosmetic: when a REPL's stdin is a pipe rather than a
/// real TTY (exactly this `Session`'s setup — `Stdio::piped()`), an
/// interactive interpreter's line-buffered compiler needs an explicit
/// empty line to know a multi-line compound statement (Python `for`/`if`/
/// `def`, etc.) has ended — the same signal a human would give by pressing
/// Enter on a blank line in a real terminal. Without it, the dedented
/// end-sentinel line gets fed to the compiler as if it were itself the
/// next statement, which for Python throws a `SyntaxError` on that line
/// and the sentinel is never printed — silently hanging until `execute`'s
/// timeout fires. Confirmed empirically: `for i in range(3): print(i)`
/// piped into `python3 -u -i` without a trailing blank line fails with
/// `SyntaxError: invalid syntax` on the dedented follow-up line; the exact
/// same input WITH a trailing blank line executes correctly. Applied
/// uniformly across every language here (not just Python) since a blank
/// line is syntactically inert in all of them at top level, and the same
/// class of bug is plausible for any block-structured REPL language
/// (Ruby's `end`, JS's `}`, etc.) — cheap insurance, not just a python fix.
fn wrap_with_sentinels(language: &str, code: &str, sentinel: &str) -> String {
    let start = format!("{}_START", sentinel);
    let end = format!("{}_END", sentinel);

    match language {
        "python" | "python3" | "python2" => {
            format!("print(\"{}\")\n{}\n\nprint(\"{}\")\n", start, code, end)
        }
        "ruby" => {
            format!("puts \"{}\"\n{}\n\nputs \"{}\"\n", start, code, end)
        }
        "node" | "javascript" | "js" => {
            format!(
                "console.log(\"{}\")\n{}\n\nconsole.log(\"{}\")\n",
                start, code, end
            )
        }
        "bash" | "sh" | "zsh" => {
            format!("echo '{}'\n{}\n\necho '{}'\n", start, code, end)
        }
        "lua" => {
            format!("print(\"{}\")\n{}\n\nprint(\"{}\")\n", start, code, end)
        }
        "R" | "r" => {
            format!("cat(\"{}\\n\")\n{}\n\ncat(\"{}\\n\")\n", start, code, end)
        }
        _ => {
            // Fallback: use echo (shell-like)
            format!("echo '{}'\n{}\n\necho '{}'\n", start, code, end)
        }
    }
}

/// Strip leading interactive-REPL prompt tokens (`>>> `, `... ` — Python's
/// `sys.ps1`/`sys.ps2`, echoed to stderr for every statement whether or not
/// it errors) from a stderr line, repeatedly (a line can carry several in a
/// row, e.g. `>>> >>> >>> Traceback (most recent call last):`). Returns the
/// remaining content trimmed; empty for a line that was pure prompt noise
/// (the common case — most stderr lines on a successful execution are
/// nothing but echoed prompts), non-empty for real error/warning content
/// worth surfacing (a traceback, an exception message, ...).
fn strip_repl_prompts(line: &str) -> String {
    let mut s = line.trim_start();
    loop {
        if let Some(rest) = s.strip_prefix(">>>") {
            s = rest.trim_start();
        } else if let Some(rest) = s.strip_prefix("...") {
            s = rest.trim_start();
        } else {
            break;
        }
    }
    s.trim_end().to_string()
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
        let mut session = match Session::new("bash", "test", Path::new("/tmp"), true) {
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
        let mut session = match Session::new("bash", "state-test", Path::new("/tmp"), true) {
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
    fn python_session_executes_a_for_loop() {
        // Regression guard: without the trailing blank line in
        // `wrap_with_sentinels`, this hangs until `execute`'s timeout
        // fires (the compound `for` statement's dedented end-sentinel
        // line gets fed to Python's piped-stdin REPL as a syntax error
        // instead of being recognized as the end of the block).
        let mut session = match Session::new("python", "for-loop-test", Path::new("/tmp"), true) {
            Ok(s) => s,
            Err(_) => return, // python3 not available, skip
        };

        let result = session.execute(
            "for i in range(3):\n    print(f\"i={i}\")",
            Duration::from_secs(10),
        );
        match result {
            Ok(output) => {
                assert!(output.contains("i=0"), "output: {output}");
                assert!(output.contains("i=1"), "output: {output}");
                assert!(output.contains("i=2"), "output: {output}");
            }
            Err(e) => panic!("Session execute failed (likely the sentinel/for-loop bug): {e}"),
        }
    }

    #[test]
    fn python_session_state_persists_across_a_compound_statement() {
        // Combines both bug axes: state persistence across executions
        // AND a compound statement in the block that reads that state.
        let mut session =
            match Session::new("python", "for-loop-state-test", Path::new("/tmp"), true) {
                Ok(s) => s,
                Err(_) => return,
            };

        session
            .execute("values = [10, 20, 30]", Duration::from_secs(10))
            .unwrap();

        let result = session.execute(
            "for v in values:\n    print(f\"v={v}\")",
            Duration::from_secs(10),
        );
        match result {
            Ok(output) => {
                assert!(output.contains("v=10"), "output: {output}");
                assert!(output.contains("v=20"), "output: {output}");
                assert!(output.contains("v=30"), "output: {output}");
            }
            Err(e) => panic!("Session execute failed: {e}"),
        }
    }

    #[test]
    fn strip_repl_prompts_removes_pure_prompt_noise() {
        assert_eq!(strip_repl_prompts(">>> >>> >>>"), "");
        assert_eq!(strip_repl_prompts("... ..."), "");
        assert_eq!(strip_repl_prompts(">>>"), "");
        assert_eq!(strip_repl_prompts(""), "");
    }

    #[test]
    fn strip_repl_prompts_preserves_real_content_after_prompts() {
        assert_eq!(
            strip_repl_prompts(">>> >>> >>> Traceback (most recent call last):"),
            "Traceback (most recent call last):"
        );
        assert_eq!(
            strip_repl_prompts("KeyError: 'JENKINS_URL'"),
            "KeyError: 'JENKINS_URL'"
        );
    }

    #[test]
    fn python_session_uncaught_exception_surfaces_in_output_instead_of_vanishing() {
        // Regression guard for the silent-failure bug: before capturing
        // stderr, this returned `Ok("")` — the exception vanished into an
        // unread stderr pipe, producing an empty #+RESULTS: block with
        // zero indication anything went wrong. Mirrors the real shape of
        // the bug (an org-babel block referencing an unset env var, then a
        // later statement that depends on the resulting failed
        // assignment): the interactive REPL executes each top-level
        // statement independently (confirmed empirically — a statement
        // with no dependency on the failed one still runs), so the
        // downstream statement here fails with its OWN (also-swallowed-
        // without-the-fix) NameError rather than never running at all.
        let mut session = match Session::new("python", "exception-test", Path::new("/tmp"), true) {
            Ok(s) => s,
            Err(_) => return,
        };

        let result = session.execute(
            "import os\nval = os.environ[\"MAE_TEST_DEFINITELY_UNSET_VAR\"]\nprint(f\"value is {val}\")",
            Duration::from_secs(10),
        );
        match result {
            Ok(output) => {
                assert!(
                    output.contains("KeyError"),
                    "expected the first statement's KeyError to surface: {output:?}"
                );
                assert!(
                    output.contains("NameError"),
                    "expected the downstream statement's own NameError (referencing the \
                     never-assigned variable) to ALSO surface, not just the first exception: \
                     {output:?}"
                );
                // The traceback echoes the failing *source line* itself
                // (`print(f"value is {val}")`) as part of its normal
                // formatting — that's expected and fine. What must NOT
                // appear is the *interpolated, successfully-printed*
                // form, which would start with the f-string already
                // substituted (impossible here since `val` never existed).
                assert!(
                    !output.contains("value is None") && !output.contains("value is <"),
                    "the print must never have actually executed: {output:?}"
                );
            }
            Err(e) => panic!("Session execute failed: {e}"),
        }
    }

    #[test]
    fn python_session_clean_execution_has_no_stray_prompt_noise() {
        // The flip side of the exception test: a successful execution's
        // stderr is nothing but echoed `>>> `/`... ` prompts (confirmed
        // empirically) — those must never leak into the returned output,
        // or every successful block would show garbage prompt characters.
        let mut session = match Session::new("python", "clean-test", Path::new("/tmp"), true) {
            Ok(s) => s,
            Err(_) => return,
        };

        let result = session.execute("print(\"clean output\")", Duration::from_secs(10));
        match result {
            Ok(output) => {
                assert_eq!(output, "clean output");
            }
            Err(e) => panic!("Session execute failed: {e}"),
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
