//! Compiled backend — compile-cache-execute for compiled languages
//! (`rust`, `go`, `c`, `c++`/`cpp`).
//!
//! Source is compiled to a content-hashed binary under `$XDG_CACHE_HOME/mae/babel`
//! (so identical blocks re-run without recompiling), then executed with the same
//! timeout + output-truncation discipline as the shell path. Compiler binaries are
//! configurable per principle #8: resolution order is the block's `:cmd` header arg
//! → the `MAE_BABEL_{CXX,CC}` env var → the executor option (default `c++` / `cc`).
//!
//! Variables (`:var`) are NOT injected for compiled languages: the generic
//! `format_var_binding` fallback emits `# var: …`, which is an invalid preprocessor
//! directive in C/C++ and would break compilation. Compiled blocks use the raw body.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::LanguageBackend;
use crate::execute::{ExecResult, WaitTimeout};
use crate::SrcBlock;

/// Default C++ compiler when no `:cmd`/env/option override is present.
pub const DEFAULT_CXX: &str = "c++";
/// Default C compiler when no `:cmd`/env/option override is present.
pub const DEFAULT_CC: &str = "cc";
/// Default C++ standard passed as `-std=<value>` (empty = omit the flag).
pub const DEFAULT_CXX_STD: &str = "c++17";

pub struct CompiledBackend {
    cache_dir: PathBuf,
    cached: HashMap<u64, PathBuf>,
    /// Wall-clock cap on the compiled program's own execution.
    pub timeout_secs: u64,
    /// Truncation ceiling for captured stdout.
    pub max_output_bytes: usize,
    /// C++ compiler (option value; default [`DEFAULT_CXX`]).
    pub cxx: String,
    /// C compiler (option value; default [`DEFAULT_CC`]).
    pub cc: String,
    /// C++ standard, passed as `-std=<cxx_std>`; empty omits the flag.
    pub cxx_std: String,
}

impl CompiledBackend {
    pub fn new() -> Self {
        let cache_dir = std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                PathBuf::from(home).join(".cache")
            })
            .join("mae")
            .join("babel");

        CompiledBackend {
            cache_dir,
            cached: HashMap::new(),
            timeout_secs: 30,
            max_output_bytes: 100 * 1024,
            cxx: DEFAULT_CXX.to_string(),
            cc: DEFAULT_CC.to_string(),
            cxx_std: DEFAULT_CXX_STD.to_string(),
        }
    }

    fn hash_source(source: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        hasher.finish()
    }

    /// Resolve the C++ compiler: `:cmd` header → `MAE_BABEL_CXX` env → option.
    fn resolve_cxx(&self, block: &SrcBlock) -> String {
        block
            .header_args
            .cmd
            .clone()
            .or_else(|| {
                std::env::var("MAE_BABEL_CXX")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| self.cxx.clone())
    }

    /// Resolve the C compiler: `:cmd` header → `MAE_BABEL_CC` env → option.
    fn resolve_cc(&self, block: &SrcBlock) -> String {
        block
            .header_args
            .cmd
            .clone()
            .or_else(|| std::env::var("MAE_BABEL_CC").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| self.cc.clone())
    }
}

impl Default for CompiledBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageBackend for CompiledBackend {
    fn name(&self) -> &str {
        "compiled"
    }

    fn can_handle(&self, language: &str) -> bool {
        matches!(
            language.to_ascii_lowercase().as_str(),
            "rust" | "go" | "c" | "c++" | "cpp"
        )
    }

    fn execute(&mut self, block: &SrcBlock, dir: &Path, _vars: &[(String, String)]) -> ExecResult {
        let lang = block.language.to_ascii_lowercase();
        let hash = Self::hash_source(&block.body);

        // Cache hit — re-run the previously compiled binary.
        if let Some(binary_path) = self.cached.get(&hash) {
            if binary_path.exists() {
                return run_binary(binary_path, dir, self.timeout_secs, self.max_output_bytes);
            }
        }

        if let Err(e) = std::fs::create_dir_all(&self.cache_dir) {
            return ExecResult::Error(format!("Failed to create cache dir: {}", e));
        }

        let binary_path = self.cache_dir.join(format!("babel-{:016x}", hash));

        // `:cmd` overrides the compiler for every compiled language; c/c++ also
        // honor the MAE_BABEL_* env + option (via resolve_*).
        let cmd_override = block.header_args.cmd.clone();
        let compile_result = match lang.as_str() {
            "rust" => compile_rust(
                &cmd_override.unwrap_or_else(|| "rustc".to_string()),
                &block.body,
                &binary_path,
            ),
            "go" => compile_go(
                &cmd_override.unwrap_or_else(|| "go".to_string()),
                &block.body,
                &binary_path,
                dir,
            ),
            "c" => compile_c(&self.resolve_cc(block), &block.body, &binary_path),
            "c++" | "cpp" => compile_cpp(
                &self.resolve_cxx(block),
                &self.cxx_std,
                &block.body,
                &binary_path,
            ),
            other => Err(format!("No compiler for {}", other)),
        };

        match compile_result {
            Ok(()) => {
                self.cached.insert(hash, binary_path.clone());
                run_binary(&binary_path, dir, self.timeout_secs, self.max_output_bytes)
            }
            Err(e) => ExecResult::Error(e),
        }
    }
}

/// Compile `source` (fed on stdin) to `output`; map a non-zero exit to a
/// "Compilation failed" error carrying the compiler's stderr.
fn compile_via_stdin(
    compiler: &str,
    args: &[String],
    source: &str,
    kind: &str,
) -> Result<(), String> {
    let mut child = Command::new(compiler)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!("{compiler} not found in PATH: {e}. Install it or set :cmd/MAE_BABEL_{kind}.")
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(source.as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("{compiler} failed: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Compilation failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn compile_rust(compiler: &str, source: &str, output: &Path) -> Result<(), String> {
    let args = vec![
        "-".to_string(),
        "-o".to_string(),
        output.to_string_lossy().into_owned(),
    ];
    compile_via_stdin(compiler, &args, source, "RUST")
}

fn compile_go(compiler: &str, source: &str, output: &Path, dir: &Path) -> Result<(), String> {
    // Go requires a file, not stdin.
    let tmp = dir.join(".mae-babel-tmp.go");
    std::fs::write(&tmp, source).map_err(|e| format!("Failed to write temp file: {}", e))?;

    let result = Command::new(compiler)
        .args([
            "build",
            "-o",
            &output.to_string_lossy(),
            &tmp.to_string_lossy(),
        ])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("{compiler} not found: {e}"))?;

    let _ = std::fs::remove_file(&tmp);

    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Compilation failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        ))
    }
}

fn compile_c(compiler: &str, source: &str, output: &Path) -> Result<(), String> {
    let args = vec![
        "-x".to_string(),
        "c".to_string(),
        "-".to_string(),
        "-o".to_string(),
        output.to_string_lossy().into_owned(),
    ];
    compile_via_stdin(compiler, &args, source, "CC")
}

fn compile_cpp(compiler: &str, std_flag: &str, source: &str, output: &Path) -> Result<(), String> {
    let mut args = Vec::new();
    if !std_flag.is_empty() {
        args.push(format!("-std={}", std_flag));
    }
    args.extend([
        "-x".to_string(),
        "c++".to_string(),
        "-".to_string(),
        "-o".to_string(),
        output.to_string_lossy().into_owned(),
    ]);
    compile_via_stdin(compiler, &args, source, "CXX")
}

/// Run a compiled binary with the same timeout + truncation discipline as the
/// shell path (`execute_shell`). A binary that never exits is killed at
/// `timeout_secs`, so a runaway compiled block can't hang the editor.
fn run_binary(path: &Path, dir: &Path, timeout_secs: u64, max_output_bytes: usize) -> ExecResult {
    let mut child = match Command::new(path)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ExecResult::Error(format!("Failed to run binary: {}", e)),
    };

    match child.wait_timeout(Duration::from_secs(timeout_secs)) {
        Ok(Some(status)) => {
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(ref mut out) = child.stdout {
                let _ = out.read_to_string(&mut stdout);
            }
            if let Some(ref mut err) = child.stderr {
                let _ = err.read_to_string(&mut stderr);
            }

            if stdout.len() > max_output_bytes {
                stdout.truncate(max_output_bytes);
                stdout.push_str("\n... (output truncated)");
            }

            if !status.success() && !stderr.is_empty() {
                ExecResult::Error(format!("{}\n{}", stdout, stderr))
            } else if !stderr.is_empty() {
                stdout.push_str(&stderr);
                ExecResult::Output(stdout)
            } else {
                ExecResult::Output(stdout)
            }
        }
        Ok(None) => {
            let _ = child.kill();
            ExecResult::Error(format!("Execution timed out after {}s", timeout_secs))
        }
        Err(e) => ExecResult::Error(format!("Failed to wait for process: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_compiled_languages() {
        let b = CompiledBackend::new();
        assert!(b.can_handle("rust"));
        assert!(b.can_handle("go"));
        assert!(b.can_handle("c"));
        assert!(b.can_handle("c++"));
        assert!(b.can_handle("cpp"));
        // Case-insensitive: `#+begin_src C++` must still route here.
        assert!(b.can_handle("C++"));
        assert!(b.can_handle("Cpp"));
        assert!(!b.can_handle("python"));
    }

    #[test]
    fn hash_deterministic() {
        let h1 = CompiledBackend::hash_source("fn main() {}");
        let h2 = CompiledBackend::hash_source("fn main() {}");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_different_source() {
        let h1 = CompiledBackend::hash_source("fn main() {}");
        let h2 = CompiledBackend::hash_source("fn main() { println!(\"hi\"); }");
        assert_ne!(h1, h2);
    }

    #[test]
    fn default_compilers_are_portable() {
        let b = CompiledBackend::new();
        // `c++`/`cc` resolve to whatever the platform provides (g++/clang++),
        // portable across Linux + macOS.
        assert_eq!(b.cxx, "c++");
        assert_eq!(b.cc, "cc");
        assert_eq!(b.cxx_std, "c++17");
    }
}
