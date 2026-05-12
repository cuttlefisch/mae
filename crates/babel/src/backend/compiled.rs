//! Compiled backend — compile-cache-execute for compiled languages.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::LanguageBackend;
use crate::execute::ExecResult;
use crate::SrcBlock;

pub struct CompiledBackend {
    cache_dir: PathBuf,
    cached: HashMap<u64, PathBuf>,
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
        }
    }

    fn hash_source(source: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        hasher.finish()
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
        matches!(language, "rust" | "go" | "c" | "c++" | "cpp")
    }

    fn execute(&mut self, block: &SrcBlock, dir: &Path, _vars: &[(String, String)]) -> ExecResult {
        let hash = Self::hash_source(&block.body);

        // Check cache
        if let Some(binary_path) = self.cached.get(&hash) {
            if binary_path.exists() {
                return run_binary(binary_path, dir);
            }
        }

        // Ensure cache directory exists
        if let Err(e) = std::fs::create_dir_all(&self.cache_dir) {
            return ExecResult::Error(format!("Failed to create cache dir: {}", e));
        }

        let binary_path = self.cache_dir.join(format!("babel-{:016x}", hash));

        // Compile
        let compile_result = match block.language.as_str() {
            "rust" => compile_rust(&block.body, &binary_path),
            "go" => compile_go(&block.body, &binary_path, dir),
            "c" => compile_c(&block.body, &binary_path),
            "c++" | "cpp" => compile_cpp(&block.body, &binary_path),
            _ => Err(format!("No compiler for {}", block.language)),
        };

        match compile_result {
            Ok(()) => {
                self.cached.insert(hash, binary_path.clone());
                run_binary(&binary_path, dir)
            }
            Err(e) => ExecResult::Error(e),
        }
    }
}

fn compile_rust(source: &str, output: &Path) -> Result<(), String> {
    let mut child = Command::new("rustc")
        .args(["-", "-o", &output.to_string_lossy()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("rustc not found: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(source.as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("rustc failed: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Compilation failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn compile_go(source: &str, output: &Path, dir: &Path) -> Result<(), String> {
    // Go requires a file, not stdin
    let tmp = dir.join(".mae-babel-tmp.go");
    std::fs::write(&tmp, source).map_err(|e| format!("Failed to write temp file: {}", e))?;

    let result = Command::new("go")
        .args([
            "build",
            "-o",
            &output.to_string_lossy(),
            &tmp.to_string_lossy(),
        ])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("go not found: {}", e))?;

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

fn compile_c(source: &str, output: &Path) -> Result<(), String> {
    let mut child = Command::new("cc")
        .args(["-x", "c", "-", "-o", &output.to_string_lossy()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cc not found: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(source.as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("cc failed: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Compilation failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn compile_cpp(source: &str, output: &Path) -> Result<(), String> {
    let mut child = Command::new("c++")
        .args(["-x", "c++", "-", "-o", &output.to_string_lossy()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("c++ not found: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(source.as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("c++ failed: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Compilation failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn run_binary(path: &Path, dir: &Path) -> ExecResult {
    match Command::new(path)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(output) => {
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !output.status.success() && !stderr.is_empty() {
                ExecResult::Error(format!("{}\n{}", stdout, stderr))
            } else {
                if !stderr.is_empty() {
                    stdout.push_str(&stderr);
                }
                ExecResult::Output(stdout)
            }
        }
        Err(e) => ExecResult::Error(format!("Failed to run binary: {}", e)),
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
}
