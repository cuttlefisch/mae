//! Grep-based definition finding — fallback when no LSP is available.

use std::path::Path;
use std::process::{Command, Stdio};

/// A definition match found by grep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumbJumpResult {
    pub file: String,
    pub line: u32,
    pub kind: &'static str,
    pub context: String,
}

/// Find definitions of `word` in a project root using grep.
///
/// Uses language-specific regex patterns to find likely definition sites.
/// Falls back to `rg` (ripgrep) if available, otherwise `grep -rn`.
pub fn dumb_jump(word: &str, language: &str, root: &Path) -> Vec<DumbJumpResult> {
    let patterns = definition_patterns(word, language);
    if patterns.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();
    for (pattern, kind) in &patterns {
        let output = run_grep(pattern, root);
        for line in output.lines() {
            if let Some(result) = parse_grep_line(line, kind) {
                // Deduplicate by file+line
                if !results
                    .iter()
                    .any(|r: &DumbJumpResult| r.file == result.file && r.line == result.line)
                {
                    results.push(result);
                }
            }
        }
    }
    results
}

/// Language-specific definition patterns. Returns (regex, kind) pairs.
fn definition_patterns(word: &str, language: &str) -> Vec<(String, &'static str)> {
    let w = regex::escape(word);
    match language {
        "rust" => vec![
            (
                format!(
                    r"(pub\s+)?(fn|struct|enum|trait|type|const|static|mod)\s+{}\b",
                    w
                ),
                "definition",
            ),
            (format!(r"impl\b.*\b{}\b", w), "impl"),
        ],
        "python" => vec![
            (format!(r"(def|class)\s+{}\b", w), "definition"),
            (format!(r"{}\s*=", w), "variable"),
        ],
        "javascript" | "typescript" | "javascriptreact" | "typescriptreact" => vec![
            (
                format!(
                    r"(function|class|const|let|var|type|interface|enum)\s+{}\b",
                    w
                ),
                "definition",
            ),
            (
                format!(r"(export\s+)?(default\s+)?(function|class)\s+{}\b", w),
                "export",
            ),
        ],
        "go" => vec![
            (format!(r"func\s+(\([^)]*\)\s+)?{}\b", w), "function"),
            (format!(r"type\s+{}\s+(struct|interface)", w), "type"),
            (format!(r"(var|const)\s+{}\b", w), "variable"),
        ],
        "c" | "cpp" => vec![
            (
                format!(r"(struct|class|enum|typedef|union)\s+{}\b", w),
                "type",
            ),
            (
                format!(
                    r"(void|int|char|bool|auto|unsigned|long|float|double)\s+\*?\s*{}\s*\(",
                    w
                ),
                "function",
            ),
            (format!(r"#define\s+{}\b", w), "macro"),
        ],
        "java" => vec![
            (
                format!(
                    r"(public|private|protected)?\s*(static\s+)?(class|interface|enum)\s+{}\b",
                    w
                ),
                "type",
            ),
            (
                format!(r"(public|private|protected)\s+\S+\s+{}\s*\(", w),
                "method",
            ),
        ],
        "ruby" => vec![(format!(r"(def|class|module)\s+{}\b", w), "definition")],
        "scheme" | "lisp" | "elisp" => {
            vec![(format!(r"\(def(ine|un|macro|var)\s+{}\b", w), "definition")]
        }
        _ => vec![
            // Generic: any "keyword word" pattern
            (format!(r"\b{}\b", w), "reference"),
        ],
    }
}

fn run_grep(pattern: &str, root: &Path) -> String {
    // Try ripgrep first
    if let Ok(output) = Command::new("rg")
        .args([
            "--no-heading",
            "--line-number",
            "--color",
            "never",
            "-e",
            pattern,
        ])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).to_string();
        }
    }

    // Fall back to grep
    if let Ok(output) = Command::new("grep")
        .args(["-rn", "-E", pattern])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        return String::from_utf8_lossy(&output.stdout).to_string();
    }

    String::new()
}

fn parse_grep_line(line: &str, kind: &'static str) -> Option<DumbJumpResult> {
    // Format: file:line:context (from rg --no-heading or grep -n)
    let (file, rest) = line.split_once(':')?;
    let (line_str, context) = rest.split_once(':')?;
    let line_num: u32 = line_str.parse().ok()?;
    Some(DumbJumpResult {
        file: file.to_string(),
        line: line_num,
        kind,
        context: context.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_grep_output() {
        let line = "src/main.rs:42:    fn hello_world() {";
        let result = parse_grep_line(line, "function").unwrap();
        assert_eq!(result.file, "src/main.rs");
        assert_eq!(result.line, 42);
        assert_eq!(result.kind, "function");
        assert!(result.context.contains("fn hello_world"));
    }

    #[test]
    fn parse_grep_invalid() {
        assert!(parse_grep_line("no colon here", "ref").is_none());
        assert!(parse_grep_line("file:notanumber:ctx", "ref").is_none());
    }

    #[test]
    fn rust_patterns() {
        let patterns = definition_patterns("MyStruct", "rust");
        assert!(!patterns.is_empty());
        assert!(patterns[0].0.contains("MyStruct"));
    }

    #[test]
    fn python_patterns() {
        let patterns = definition_patterns("my_func", "python");
        assert!(!patterns.is_empty());
        assert!(patterns[0].0.contains("def|class"));
    }

    #[test]
    fn js_patterns() {
        let patterns = definition_patterns("Component", "javascript");
        assert!(!patterns.is_empty());
    }

    #[test]
    fn go_patterns() {
        let patterns = definition_patterns("Handler", "go");
        assert!(patterns.len() >= 2);
    }

    #[test]
    fn unknown_language_generic() {
        let patterns = definition_patterns("foo", "brainfuck");
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].1, "reference");
    }

    #[test]
    fn special_chars_escaped() {
        let patterns = definition_patterns("my.func", "rust");
        // The dot should be escaped
        assert!(patterns[0].0.contains(r"my\.func"));
    }
}
