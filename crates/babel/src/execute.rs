//! Babel execution engine — runs source blocks and captures output.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::backend::compiled::CompiledBackend;
use super::backend::LanguageBackend;
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
    /// Datalog blocks need evaluation through the KB store (CozoDB).
    PendingDatalogQuery(String),
}

/// Babel execution engine with session management.
pub struct BabelExecutor {
    pub sessions: SessionManager,
    pub timeout_secs: u64,
    pub max_output_bytes: usize,
    /// Compile-cache-execute backend for compiled languages (rust/go/c/c++).
    /// Its compiler options are set from the editor's babel options.
    pub compiled: CompiledBackend,
    /// Whether to merge the user's resolved shell environment (see
    /// `shell_env`) into spawned processes — set from the editor's
    /// `babel_inherit_shell_env` option.
    pub shell_env_enabled: bool,
}

impl Default for BabelExecutor {
    fn default() -> Self {
        BabelExecutor {
            sessions: SessionManager::new(),
            timeout_secs: 30,
            max_output_bytes: 100 * 1024, // 100KB
            compiled: CompiledBackend::new(),
            shell_env_enabled: true,
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
            "datalog" | "cozodb" => ExecResult::PendingDatalogQuery(body),
            lang => {
                // Compiled languages (rust/go/c/c++/cpp) compile→cache→run via the
                // dedicated backend, ahead of the session/shell paths (they can't
                // pipe-and-run, and `repl_command` errors on them). Uses the raw
                // body — `:var` injection is undefined for compiled sources.
                if self.compiled.can_handle(lang) {
                    self.compiled.timeout_secs = self.timeout_secs;
                    self.compiled.max_output_bytes = self.max_output_bytes;
                    return self.compiled.execute(block, &working_dir, resolved_vars);
                }
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

        let mut command = Command::new(&cmd);
        command
            .args(&cmd_args)
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::shell_env::apply_to(&mut command, self.shell_env_enabled);
        command.env("MAE_BABEL", "1");
        let result = command.spawn();

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
            if let Err(e) = stdin.write_all(body.as_bytes()) {
                eprintln!("babel: failed writing block body to child stdin: {e}");
            }
            // stdin drops here, closing the pipe
        }

        // Drain stdout/stderr on background threads CONCURRENTLY with waiting,
        // not after. wait_timeout() only polls try_wait() (see the WaitTimeout
        // impl below) -- it never touches stdout/stderr -- so a child that
        // writes more than the OS pipe buffer (~64KB) before exiting would
        // otherwise block on write() forever, since nothing drains the pipe
        // until after wait_timeout returns, and wait_timeout never returns
        // until the child (blocked on write) exits. Each drain thread is also
        // bounded to max_output_bytes so it can't buffer unbounded output into
        // memory even for a process that legitimately produces a lot before
        // exiting quickly.
        let limit = self.max_output_bytes as u64;
        let stdout_handle = child.stdout.take().map(|out| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if let Err(e) = out.take(limit).read_to_end(&mut buf) {
                    eprintln!("babel: failed reading child stdout: {e}");
                }
                buf
            })
        });
        let stderr_handle = child.stderr.take().map(|err| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if let Err(e) = err.take(limit).read_to_end(&mut buf) {
                    eprintln!("babel: failed reading child stderr: {e}");
                }
                buf
            })
        });

        let timeout = Duration::from_secs(self.timeout_secs);
        let wait_result = child.wait_timeout(timeout);
        if matches!(wait_result, Ok(None)) {
            if let Err(e) = child.kill() {
                eprintln!("babel: failed to kill timed-out child process: {e}");
            }
        }
        let stdout_bytes = stdout_handle
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default();
        let stderr_bytes = stderr_handle
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default();

        match wait_result {
            Ok(Some(status)) => {
                let truncated = stdout_bytes.len() as u64 >= limit;
                let mut stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
                let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

                if truncated {
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
        "bash" | "sh" => (resolve_posix_shell(), Vec::new()),
        "zsh" => ("zsh".to_string(), Vec::new()),
        "fish" => ("fish".to_string(), Vec::new()),
        "node" | "javascript" | "js" => ("node".to_string(), Vec::new()),
        "lua" => ("lua".to_string(), Vec::new()),
        "R" | "r" => ("Rscript".to_string(), vec!["--vanilla".to_string()]),
        // NOTE: compiled languages (rust/go/c/c++) are handled by CompiledBackend
        // before reaching execute_shell — they never resolve here.
        _ => (language.to_string(), Vec::new()),
    }
}

/// True when `candidate` lives inside the Windows system directory.
///
/// @ai-caution: [cross-platform] `C:\Windows\System32\bash.exe` is **not** a
/// POSIX shell -- it is the Windows Subsystem for Linux launcher. It sits in the
/// system directory, which precedes the Git-for-Windows directories on the
/// default `PATH`, so a bare `Command::new("bash")` finds it before any real
/// shell. On a machine with no WSL distribution installed it never runs the
/// block at all: it prints "Windows Subsystem for Linux has no installed
/// distributions" encoded as **UTF-16**, which `from_utf8_lossy` then turns into
/// NUL-riddled mojibake inside the user's org file. Never resolve `sh`/`bash` to
/// a binary under the system root.
///
/// Kept compiled on every platform (not `#[cfg(windows)]`) so the rule stays
/// unit-testable on the machines MAE is actually developed on -- principle #13:
/// a Windows-only code path nobody can iterate against is exactly how this class
/// of bug survives.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_under_windows_system_root(candidate: &Path, system_root: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().to_lowercase().replace('/', "\\");
    let candidate = norm(candidate);
    let root = norm(system_root);
    let root = root.trim_end_matches('\\');
    !root.is_empty() && (candidate == root || candidate.starts_with(&format!("{root}\\")))
}

/// Derive a Git for Windows install root from the location of `git.exe`.
///
/// Git for Windows puts `git.exe` in `<root>\cmd` (and `<root>\bin`) and its
/// POSIX shell in `<root>\bin\bash.exe`. Deriving the root from wherever `git`
/// actually is covers custom install locations that a hardcoded
/// `C:\Program Files\Git` list would miss.
#[cfg_attr(not(windows), allow(dead_code))]
fn git_install_root(git_exe: &Path) -> Option<PathBuf> {
    let parent = git_exe.parent()?;
    let dir = parent.file_name()?.to_string_lossy().to_lowercase();
    matches!(dir.as_str(), "cmd" | "bin" | "mingw64")
        .then(|| parent.parent())?
        .map(PathBuf::from)
}

/// Pick a real POSIX shell for `sh`/`bash` blocks on Windows.
///
/// Pure over its inputs (the `PATH` entries, where `git.exe` was found, the
/// candidate install roots, the system root, and an `exists` probe) so the
/// ordering rules can be unit-tested off Windows.
///
/// `PATH` is consulted first, minus the WSL stub, so `PATH` remains the user's
/// override mechanism exactly as it is on Unix -- we only refuse the one entry
/// that is a launcher rather than a shell. Git-derived and well-known install
/// roots follow, since Git for Windows does not put `bash.exe` on `PATH` by
/// default even though it ships one.
#[cfg_attr(not(windows), allow(dead_code))]
fn select_windows_posix_shell(
    path_entries: &[PathBuf],
    git_exe: Option<&Path>,
    install_roots: &[PathBuf],
    system_root: Option<&Path>,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    for dir in path_entries {
        let candidate = dir.join("bash.exe");
        if let Some(root) = system_root {
            if is_under_windows_system_root(&candidate, root) {
                continue;
            }
        }
        if exists(&candidate) {
            return Some(candidate);
        }
    }
    let roots = git_exe
        .and_then(git_install_root)
        .into_iter()
        .chain(install_roots.iter().cloned());
    for root in roots {
        for sub in [["bin"].as_slice(), ["usr", "bin"].as_slice()] {
            let candidate = sub
                .iter()
                .fold(root.clone(), |acc, part| acc.join(part))
                .join("bash.exe");
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolve the shell that `sh`/`bash` blocks are executed with.
///
/// On Unix this is plain `bash`, resolved from `PATH` by the OS exactly as
/// before -- this function introduces no behavior change off Windows.
///
/// On Windows it searches for a genuine POSIX shell rather than trusting
/// `PATH`'s first `bash`, which is the WSL launcher (see
/// [`is_under_windows_system_root`]). If nothing is found we still fall back to
/// `bash`: that keeps the previous behavior instead of hard-failing a user who
/// has some shell we did not anticipate, and the WSL launcher's complaint now
/// arrives as legible text rather than NUL corruption thanks to
/// `results::normalize_output`. A per-block `:cmd` remains the explicit override
/// on every platform.
fn resolve_posix_shell() -> String {
    #[cfg(windows)]
    {
        let path_entries: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let git_exe = path_entries
            .iter()
            .map(|dir| dir.join("git.exe"))
            .find(|p| p.is_file());
        let install_roots: Vec<PathBuf> = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|key| std::env::var_os(key))
            .map(|root| PathBuf::from(root).join("Git"))
            .chain(
                std::env::var_os("LOCALAPPDATA")
                    .map(|root| PathBuf::from(root).join("Programs").join("Git")),
            )
            .collect();
        let system_root = std::env::var_os("SystemRoot").map(PathBuf::from);
        if let Some(shell) = select_windows_posix_shell(
            &path_entries,
            git_exe.as_deref(),
            &install_roots,
            system_root.as_deref(),
            &|p| p.is_file(),
        ) {
            return shell.to_string_lossy().into_owned();
        }
    }
    "bash".to_string()
}

#[cfg(test)]
mod posix_shell_tests {
    use super::*;

    /// Windows paths, exercised on whatever platform CI/the developer is on --
    /// the rule is pure string/patch logic, so there is no reason to make it
    /// only observable on a runner we cannot iterate against.
    #[test]
    fn the_wsl_launcher_is_recognized_under_the_system_root() {
        let root = Path::new(r"C:\Windows");
        for stub in [
            r"C:\Windows\System32\bash.exe",
            r"C:\WINDOWS\system32\bash.exe", // case-insensitive filesystem
            r"c:/windows/system32/bash.exe", // forward slashes are legal on Windows
            r"C:\Windows\bash.exe",
        ] {
            assert!(
                is_under_windows_system_root(Path::new(stub), root),
                "{stub} must be refused as the WSL launcher"
            );
        }
        // The negative half: a real shell must NOT be mistaken for the stub.
        for real in [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\msys64\usr\bin\bash.exe",
            r"C:\Windows-Tools\bin\bash.exe", // prefix-only match must not fire
            r"D:\Windows\System32\bash.exe",  // different volume
        ] {
            assert!(
                !is_under_windows_system_root(Path::new(real), root),
                "{real} is a real shell and must not be refused"
            );
        }
    }

    #[test]
    fn git_install_root_is_derived_from_where_git_actually_is() {
        assert_eq!(
            git_install_root(Path::new(r"C:\Program Files\Git\cmd\git.exe")),
            Some(PathBuf::from(r"C:\Program Files\Git"))
        );
        assert_eq!(
            git_install_root(Path::new(r"D:\tools\Git\bin\git.exe")),
            Some(PathBuf::from(r"D:\tools\Git"))
        );
        // A `git.exe` somewhere unrecognized must not invent a root.
        assert_eq!(git_install_root(Path::new(r"C:\odd\git.exe")), None);
    }

    /// The whole point of the fix: given a `PATH` whose first `bash.exe` is the
    /// WSL stub, resolution must skip it and land on the real shell. This is the
    /// exact shape of the GitHub Windows runner that produced the CI failure.
    #[test]
    fn path_resolution_skips_the_wsl_stub_and_finds_the_real_shell() {
        let system32 = PathBuf::from(r"C:\Windows\System32");
        let git_bin = PathBuf::from(r"C:\Program Files\Git\bin");
        let present = [
            PathBuf::from(r"C:\Windows\System32\bash.exe"), // the trap
            git_bin.join("bash.exe"),
        ];
        let exists = |p: &Path| present.iter().any(|q| q == p);

        let picked = select_windows_posix_shell(
            &[system32.clone(), git_bin.clone()],
            None,
            &[],
            Some(Path::new(r"C:\Windows")),
            &exists,
        );
        assert_eq!(picked, Some(git_bin.join("bash.exe")));

        // Without the system-root exclusion the stub would win -- proving the
        // assertion above is actually testing the exclusion and not the
        // incidental ordering of the PATH entries.
        let unguarded = select_windows_posix_shell(&[system32, git_bin], None, &[], None, &exists);
        assert_eq!(
            unguarded,
            Some(PathBuf::from(r"C:\Windows\System32\bash.exe")),
            "precondition: the stub is what a naive PATH scan picks"
        );
    }

    /// Git for Windows does not put `bash.exe` on `PATH`, so the common real
    /// case is: PATH yields only the stub, and the shell has to come from an
    /// install root -- derived from `git.exe`, or from a well-known location.
    #[test]
    fn falls_back_to_install_roots_when_path_has_only_the_stub() {
        let usr_bin_shell = PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe");
        let exists = |p: &Path| p == usr_bin_shell;
        let path_entries = [PathBuf::from(r"C:\Windows\System32")];
        let system_root = Some(Path::new(r"C:\Windows"));

        // Derived from where git.exe lives.
        assert_eq!(
            select_windows_posix_shell(
                &path_entries,
                Some(Path::new(r"C:\Program Files\Git\cmd\git.exe")),
                &[],
                system_root,
                &exists,
            ),
            Some(usr_bin_shell.clone())
        );
        // Or from a well-known install root when git.exe was not on PATH.
        assert_eq!(
            select_windows_posix_shell(
                &path_entries,
                None,
                &[PathBuf::from(r"C:\Program Files\Git")],
                system_root,
                &exists,
            ),
            Some(usr_bin_shell)
        );
        // And nothing is invented when no shell is installed at all.
        assert_eq!(
            select_windows_posix_shell(
                &path_entries,
                None,
                &[PathBuf::from(r"C:\Program Files\Git")],
                system_root,
                &|_| false,
            ),
            None
        );
    }

    /// Off Windows nothing changes: `sh`/`bash` still resolve to plain `bash`
    /// from `PATH`, so this fix cannot regress the platforms it is not for.
    #[cfg(not(windows))]
    #[test]
    fn unix_shell_resolution_is_unchanged() {
        let (cmd, args) = resolve_command("sh", &HeaderArgs::default());
        assert_eq!(cmd, "bash");
        assert!(args.is_empty());
        assert_eq!(resolve_command("bash", &HeaderArgs::default()).0, "bash");
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
pub(crate) trait WaitTimeout {
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
            body_char_range: (0, 10),
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
            body_char_range: (0, 12),
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
    fn execute_shell_output_over_limit_is_bounded_not_just_truncated() {
        // Adversarial: a block producing far more than max_output_bytes must not
        // have all of it buffered into memory before truncation -- the read
        // itself is bounded. Confirmed indirectly: the returned output length is
        // close to the (small, test-configured) limit, not the full ~1MB the
        // shell command actually produces.
        let mut executor = BabelExecutor::new();
        executor.max_output_bytes = 64;
        let block = SrcBlock {
            name: None,
            language: "bash".to_string(),
            header_args: HeaderArgs::default(),
            // `yes` repeats forever; head -c caps the pipe's producer side so the
            // test itself doesn't hang, while still producing far more than the
            // 64-byte limit for the bounded-read path to actually exercise.
            body: "yes x | head -c 1000000".to_string(),
            line_range: (0, 2),
            body_char_range: (0, 20),
        };
        let result = executor.execute_block(&block, Path::new("/tmp"), &[]);
        match result {
            ExecResult::Output(s) => {
                assert!(
                    s.len() < 1_000_000,
                    "output should be bounded well under the 1MB the command produced, got {} bytes",
                    s.len()
                );
                assert!(s.contains("... (output truncated)"));
            }
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    // --- Compiled languages (CompiledBackend) ---
    // Each skips cleanly when the toolchain is absent so CI without g++/rustc
    // stays green (the "not found" arm), but asserts real behavior when present.

    fn compiled_block(language: &str, body: &str) -> SrcBlock {
        SrcBlock {
            name: None,
            language: language.to_string(),
            header_args: HeaderArgs::default(),
            body: body.to_string(),
            line_range: (0, 2),
            body_char_range: (0, body.len()),
        }
    }

    #[test]
    fn execute_cpp_hello() {
        let mut executor = BabelExecutor::new();
        let block = compiled_block(
            "cpp",
            "#include <iostream>\nint main(){ std::cout << \"hi-cpp\"; return 0; }",
        );
        match executor.execute_block(&block, Path::new("/tmp"), &[]) {
            ExecResult::Output(s) => assert_eq!(s.trim(), "hi-cpp"),
            ExecResult::Error(e) if e.contains("not found") => { /* no C++ toolchain, skip */ }
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn execute_cpp_uppercase_alias() {
        // `#+begin_src C++` must route to the compiled backend, not run a
        // program literally named `C++`.
        let mut executor = BabelExecutor::new();
        let block = compiled_block(
            "C++",
            "#include <iostream>\nint main(){ std::cout << \"up\"; }",
        );
        match executor.execute_block(&block, Path::new("/tmp"), &[]) {
            ExecResult::Output(s) => assert_eq!(s.trim(), "up"),
            ExecResult::Error(e) if e.contains("not found") => { /* skip */ }
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn execute_cpp_compile_error_is_reported() {
        // A block that fails to compile must surface a "Compilation failed"
        // error, NOT silently succeed (#14: the failure path).
        let mut executor = BabelExecutor::new();
        let block = compiled_block("cpp", "int main(){ this is not valid c++ }");
        match executor.execute_block(&block, Path::new("/tmp"), &[]) {
            ExecResult::Error(e) if e.contains("Compilation failed") => { /* expected */ }
            ExecResult::Error(e) if e.contains("not found") => { /* no toolchain, skip */ }
            other => panic!("Expected a compilation error, got {:?}", other),
        }
    }

    #[test]
    fn execute_c_hello() {
        let mut executor = BabelExecutor::new();
        let block = compiled_block(
            "c",
            "#include <stdio.h>\nint main(void){ printf(\"hi-c\"); return 0; }",
        );
        match executor.execute_block(&block, Path::new("/tmp"), &[]) {
            ExecResult::Output(s) => assert_eq!(s.trim(), "hi-c"),
            ExecResult::Error(e) if e.contains("not found") => { /* skip */ }
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn execute_rust_now_runs_the_binary() {
        // Regression: Rust babel previously compiled then captured rustc's empty
        // stdout instead of running the binary. It must now print program output.
        let mut executor = BabelExecutor::new();
        let block = compiled_block("rust", "fn main(){ print!(\"hi-rust\"); }");
        match executor.execute_block(&block, Path::new("/tmp"), &[]) {
            ExecResult::Output(s) => assert_eq!(s.trim(), "hi-rust"),
            ExecResult::Error(e) if e.contains("not found") => { /* no rustc, skip */ }
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
            body_char_range: (0, 0),
        };
        let result = executor.execute_block(&block, Path::new("/tmp"), &[]);
        match result {
            ExecResult::Error(e) => assert!(e.contains("blocked")),
            other => panic!("Expected Error, got {:?}", other),
        }
    }
}
