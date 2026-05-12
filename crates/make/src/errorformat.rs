//! Compiler error output parsing — structured diagnostics from build output.

use regex::Regex;

/// A parsed build error from compiler output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError {
    pub file: String,
    pub line: u32,
    pub col: Option<u32>,
    pub message: String,
    pub severity: ErrorSeverity,
}

/// Error severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorSeverity {
    Error,
    Warning,
    Note,
}

impl std::fmt::Display for ErrorSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error => write!(f, "error"),
            Self::Warning => write!(f, "warning"),
            Self::Note => write!(f, "note"),
        }
    }
}

/// A compiled regex pattern for matching compiler output lines.
struct ErrorPattern {
    regex: Regex,
    severity: ErrorSeverity,
    file_group: usize,
    line_group: usize,
    col_group: Option<usize>,
    msg_group: usize,
}

/// Parse compiler output into structured errors using built-in patterns.
pub fn parse_build_output(output: &str) -> Vec<BuildError> {
    let patterns = default_patterns();
    let mut errors = Vec::new();

    for line in output.lines() {
        for pat in &patterns {
            if let Some(caps) = pat.regex.captures(line) {
                let file = caps
                    .get(pat.file_group)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                let line_num = caps
                    .get(pat.line_group)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0);
                let col = pat
                    .col_group
                    .and_then(|g| caps.get(g))
                    .and_then(|m| m.as_str().parse().ok());
                let message = caps
                    .get(pat.msg_group)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();

                errors.push(BuildError {
                    file,
                    line: line_num,
                    col,
                    message,
                    severity: pat.severity,
                });
                break; // first match wins
            }
        }
    }

    errors
}

fn default_patterns() -> Vec<ErrorPattern> {
    vec![
        // Rust: error[E0308]: mismatched types
        //   --> src/main.rs:10:5
        ErrorPattern {
            regex: Regex::new(r"^\s*--> ([^:]+):(\d+):(\d+)").unwrap(),
            severity: ErrorSeverity::Error, // severity determined from context
            file_group: 1,
            line_group: 2,
            col_group: Some(3),
            msg_group: 1, // no inline message; use file as placeholder
        },
        // GCC/Clang: file.c:10:5: error: undeclared identifier
        ErrorPattern {
            regex: Regex::new(r"^([^:\s]+):(\d+):(\d+): (error|warning|note): (.+)$").unwrap(),
            severity: ErrorSeverity::Error, // overridden by capture group
            file_group: 1,
            line_group: 2,
            col_group: Some(3),
            msg_group: 5,
        },
        // GCC/Clang without column: file.c:10: error: msg
        ErrorPattern {
            regex: Regex::new(r"^([^:\s]+):(\d+): (error|warning|note): (.+)$").unwrap(),
            severity: ErrorSeverity::Error,
            file_group: 1,
            line_group: 2,
            col_group: None,
            msg_group: 4,
        },
        // TypeScript: file.ts(10,5): error TS2304: Cannot find name
        ErrorPattern {
            regex: Regex::new(r"^([^(]+)\((\d+),(\d+)\): (error|warning) TS\d+: (.+)$").unwrap(),
            severity: ErrorSeverity::Error,
            file_group: 1,
            line_group: 2,
            col_group: Some(3),
            msg_group: 5,
        },
        // Python traceback: File "test.py", line 10
        ErrorPattern {
            regex: Regex::new(r#"^\s*File "([^"]+)", line (\d+)"#).unwrap(),
            severity: ErrorSeverity::Error,
            file_group: 1,
            line_group: 2,
            col_group: None,
            msg_group: 1,
        },
        // Go: ./main.go:10:5: undefined: foo
        ErrorPattern {
            regex: Regex::new(r"^([^\s:]+):(\d+):(\d+): (.+)$").unwrap(),
            severity: ErrorSeverity::Error,
            file_group: 1,
            line_group: 2,
            col_group: Some(3),
            msg_group: 4,
        },
    ]
}

/// Infer severity from a GCC/Clang-style severity keyword.
pub fn parse_severity(s: &str) -> ErrorSeverity {
    match s.to_lowercase().as_str() {
        "warning" => ErrorSeverity::Warning,
        "note" | "info" | "hint" => ErrorSeverity::Note,
        _ => ErrorSeverity::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gcc_error() {
        let output = "main.c:10:5: error: use of undeclared identifier 'x'";
        let errors = parse_build_output(output);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "main.c");
        assert_eq!(errors[0].line, 10);
        assert_eq!(errors[0].col, Some(5));
        assert!(errors[0].message.contains("undeclared"));
    }

    #[test]
    fn parse_gcc_warning() {
        let output = "test.c:20:3: warning: unused variable 'y'";
        let errors = parse_build_output(output);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "test.c");
        assert_eq!(errors[0].line, 20);
    }

    #[test]
    fn parse_rust_location() {
        let output = "error[E0308]: mismatched types\n  --> src/main.rs:10:5";
        let errors = parse_build_output(output);
        assert!(!errors.is_empty());
        let e = errors.iter().find(|e| e.file == "src/main.rs").unwrap();
        assert_eq!(e.line, 10);
        assert_eq!(e.col, Some(5));
    }

    #[test]
    fn parse_typescript_error() {
        let output = "src/app.ts(15,10): error TS2304: Cannot find name 'foo'";
        let errors = parse_build_output(output);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "src/app.ts");
        assert_eq!(errors[0].line, 15);
        assert_eq!(errors[0].col, Some(10));
        assert!(errors[0].message.contains("Cannot find name"));
    }

    #[test]
    fn parse_python_traceback() {
        let output = r#"  File "test.py", line 42"#;
        let errors = parse_build_output(output);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "test.py");
        assert_eq!(errors[0].line, 42);
    }

    #[test]
    fn parse_go_error() {
        let output = "./main.go:10:5: undefined: foo";
        let errors = parse_build_output(output);
        assert!(!errors.is_empty());
        let e = &errors[0];
        assert_eq!(e.file, "./main.go");
        assert_eq!(e.line, 10);
    }

    #[test]
    fn parse_multiline_output() {
        let output = "main.c:1:1: error: first\nmain.c:2:1: warning: second\nsome other line\nmain.c:3:1: error: third";
        let errors = parse_build_output(output);
        assert_eq!(errors.len(), 3);
    }

    #[test]
    fn parse_empty_output() {
        let errors = parse_build_output("");
        assert!(errors.is_empty());
    }

    #[test]
    fn severity_parsing() {
        assert_eq!(parse_severity("error"), ErrorSeverity::Error);
        assert_eq!(parse_severity("warning"), ErrorSeverity::Warning);
        assert_eq!(parse_severity("note"), ErrorSeverity::Note);
        assert_eq!(parse_severity("Warning"), ErrorSeverity::Warning);
    }
}
