//! mae-scheme error types and condition system.
//!
//! Provides structured error reporting with source locations,
//! inspired by Racket's error messages. Errors carry source spans
//! for precise diagnostics.
//!
//! @stability: unstable (Phase 13)
//! @since: 0.12.0

use std::fmt;
use std::path::PathBuf;

/// Source location for error reporting and debugging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

/// Arity specification for functions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arity {
    /// Exactly N arguments required.
    Fixed(usize),
    /// At least N arguments, rest collected in a list.
    Variadic(usize),
    /// Multiple accepted arities (case-lambda).
    Multi(Vec<usize>),
}

impl fmt::Display for Arity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arity::Fixed(n) => write!(f, "{n}"),
            Arity::Variadic(n) => write!(f, "{n}+"),
            Arity::Multi(ns) => {
                let parts: Vec<String> = ns.iter().map(|n| n.to_string()).collect();
                write!(f, "{}", parts.join(" or "))
            }
        }
    }
}

/// Structured Scheme error with source location and condition type.
#[derive(Clone, Debug)]
pub struct LispError {
    pub kind: ErrorKind,
    pub location: Option<SourceLocation>,
    pub stack_trace: Vec<StackFrame>,
    /// Optional Scheme value representing this error (for guard handlers).
    /// When `error` or `raise` creates a Scheme-level exception, the actual
    /// value is stored here so `handle_exception` can push it on the stack.
    pub error_value: Option<Box<crate::value::Value>>,
}

/// Stack frame for error reporting.
#[derive(Clone, Debug)]
pub struct StackFrame {
    pub function: String,
    pub location: Option<SourceLocation>,
}

/// Categorized error kinds for precise diagnostics.
#[derive(Clone, Debug)]
pub enum ErrorKind {
    /// Reader/parser errors (malformed input).
    Read(String),

    /// Syntax errors (invalid special forms).
    Syntax { message: String, form: String },

    /// Type mismatch (expected vs got).
    Type { expected: String, got: String },

    /// Wrong number of arguments.
    ArityMismatch {
        function: String,
        expected: Arity,
        got: usize,
    },

    /// Undefined variable reference.
    Undefined { name: String },

    /// I/O errors.
    Io {
        message: String,
        path: Option<PathBuf>,
    },

    /// User-raised error via (error msg irritants...).
    User {
        message: String,
        irritants: Vec<String>,
    },

    /// Division by zero.
    DivisionByZero,

    /// Attempt to mutate immutable data.
    Immutable { what: String },

    /// Internal VM error (should not reach users).
    Internal(String),

    /// Yield request from a foreign function.
    /// The VM catches this and returns `EvalResult::Yield` to the host.
    /// Not a real error — it's a cooperative suspension point.
    Yield(YieldReason),
}

/// Why a foreign function wants to yield control to the host.
#[derive(Clone, Debug)]
pub enum YieldReason {
    /// Sleep for the given duration.
    Sleep(std::time::Duration),
    /// Wait for a file to appear on disk (path, timeout).
    WaitForFile(PathBuf, std::time::Duration),
}

impl LispError {
    pub fn read(msg: impl Into<String>) -> Self {
        LispError {
            kind: ErrorKind::Read(msg.into()),
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    pub fn read_at(msg: impl Into<String>, loc: SourceLocation) -> Self {
        LispError {
            kind: ErrorKind::Read(msg.into()),
            location: Some(loc),
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    pub fn syntax(message: impl Into<String>, form: impl Into<String>) -> Self {
        LispError {
            kind: ErrorKind::Syntax {
                message: message.into(),
                form: form.into(),
            },
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    pub fn type_error(expected: impl Into<String>, got: impl Into<String>) -> Self {
        LispError {
            kind: ErrorKind::Type {
                expected: expected.into(),
                got: got.into(),
            },
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    pub fn arity(function: impl Into<String>, expected: Arity, got: usize) -> Self {
        LispError {
            kind: ErrorKind::ArityMismatch {
                function: function.into(),
                expected,
                got,
            },
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    pub fn undefined(name: impl Into<String>) -> Self {
        LispError {
            kind: ErrorKind::Undefined { name: name.into() },
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    pub fn io(message: impl Into<String>, path: Option<PathBuf>) -> Self {
        LispError {
            kind: ErrorKind::Io {
                message: message.into(),
                path,
            },
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    pub fn user(message: impl Into<String>, irritants: Vec<String>) -> Self {
        LispError {
            kind: ErrorKind::User {
                message: message.into(),
                irritants,
            },
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    pub fn immutable(what: impl Into<String>) -> Self {
        LispError {
            kind: ErrorKind::Immutable { what: what.into() },
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    pub fn division_by_zero() -> Self {
        LispError {
            kind: ErrorKind::DivisionByZero,
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        LispError {
            kind: ErrorKind::Internal(msg.into()),
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    /// Create a yield request (not a real error — cooperative suspension).
    pub fn yield_sleep(duration: std::time::Duration) -> Self {
        LispError {
            kind: ErrorKind::Yield(YieldReason::Sleep(duration)),
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    /// Create a yield request for file waiting.
    pub fn yield_wait_for_file(path: PathBuf, timeout: std::time::Duration) -> Self {
        LispError {
            kind: ErrorKind::Yield(YieldReason::WaitForFile(path, timeout)),
            location: None,
            stack_trace: Vec::new(),
            error_value: None,
        }
    }

    /// Returns true if this is a yield request, not a real error.
    pub fn is_yield(&self) -> bool {
        matches!(self.kind, ErrorKind::Yield(_))
    }

    /// Attach a source location to this error.
    pub fn at(mut self, loc: SourceLocation) -> Self {
        self.location = Some(loc);
        self
    }

    /// Message string for the error (for condition-message).
    pub fn message(&self) -> String {
        match &self.kind {
            ErrorKind::Read(msg) => format!("read error: {msg}"),
            ErrorKind::Syntax { message, form } => {
                format!("syntax error: {message}\n  in: {form}")
            }
            ErrorKind::Type { expected, got } => {
                format!("type error: expected {expected}, got {got}")
            }
            ErrorKind::ArityMismatch {
                function,
                expected,
                got,
            } => {
                format!("{function}: expected {expected} arguments, got {got}")
            }
            ErrorKind::Undefined { name } => format!("undefined variable: {name}"),
            ErrorKind::Io { message, path } => {
                if let Some(p) = path {
                    format!("I/O error: {message} ({})", p.display())
                } else {
                    format!("I/O error: {message}")
                }
            }
            ErrorKind::User { message, irritants } => {
                if irritants.is_empty() {
                    message.clone()
                } else {
                    format!("{message}: {}", irritants.join(" "))
                }
            }
            ErrorKind::DivisionByZero => "division by zero".to_string(),
            ErrorKind::Immutable { what } => format!("attempt to mutate immutable {what}"),
            ErrorKind::Internal(msg) => format!("internal error: {msg}"),
            ErrorKind::Yield(reason) => match reason {
                YieldReason::Sleep(d) => format!("yield: sleep {}ms", d.as_millis()),
                YieldReason::WaitForFile(p, t) => {
                    format!("yield: wait-for-file {} ({}ms)", p.display(), t.as_millis())
                }
            },
        }
    }
}

impl fmt::Display for LispError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(loc) = &self.location {
            write!(f, "{}: ", loc)?;
        }
        write!(f, "{}", self.message())?;

        if !self.stack_trace.is_empty() {
            writeln!(f, "\n\n  Stack trace:")?;
            for frame in &self.stack_trace {
                if let Some(loc) = &frame.location {
                    writeln!(f, "    {} ({})", loc, frame.function)?;
                } else {
                    writeln!(f, "    <builtin> ({})", frame.function)?;
                }
            }
        }
        Ok(())
    }
}

impl std::error::Error for LispError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = LispError::type_error("number", "string \"hello\"");
        assert_eq!(
            err.message(),
            "type error: expected number, got string \"hello\""
        );
    }

    #[test]
    fn test_error_with_location() {
        let err = LispError::undefined("foo").at(SourceLocation {
            file: "test.scm".into(),
            line: 42,
            column: 8,
        });
        let s = format!("{err}");
        assert!(s.starts_with("test.scm:42:8:"));
        assert!(s.contains("undefined variable: foo"));
    }

    #[test]
    fn test_arity_display() {
        assert_eq!(format!("{}", Arity::Fixed(2)), "2");
        assert_eq!(format!("{}", Arity::Variadic(1)), "1+");
        assert_eq!(format!("{}", Arity::Multi(vec![1, 3])), "1 or 3");
    }

    #[test]
    fn test_arity_mismatch() {
        let err = LispError::arity("map", Arity::Variadic(2), 1);
        assert_eq!(err.message(), "map: expected 2+ arguments, got 1");
    }

    #[test]
    fn test_user_error_with_irritants() {
        let err = LispError::user("bad value", vec!["42".into(), "\"x\"".into()]);
        assert_eq!(err.message(), "bad value: 42 \"x\"");
    }

    #[test]
    fn test_stack_trace_display() {
        let err = LispError {
            kind: ErrorKind::Undefined { name: "x".into() },
            location: Some(SourceLocation {
                file: "init.scm".into(),
                line: 15,
                column: 1,
            }),
            error_value: None,
            stack_trace: vec![
                StackFrame {
                    function: "compute".into(),
                    location: Some(SourceLocation {
                        file: "init.scm".into(),
                        line: 15,
                        column: 1,
                    }),
                },
                StackFrame {
                    function: "main".into(),
                    location: Some(SourceLocation {
                        file: "init.scm".into(),
                        line: 3,
                        column: 1,
                    }),
                },
            ],
        };
        let s = format!("{err}");
        assert!(s.contains("Stack trace:"));
        assert!(s.contains("compute"));
    }

    #[test]
    fn test_yield_sleep_constructor() {
        let err = LispError::yield_sleep(std::time::Duration::from_millis(100));
        assert!(err.is_yield());
        assert_eq!(err.message(), "yield: sleep 100ms");
    }

    #[test]
    fn test_yield_wait_for_file_constructor() {
        let err = LispError::yield_wait_for_file(
            PathBuf::from("/tmp/test"),
            std::time::Duration::from_millis(5000),
        );
        assert!(err.is_yield());
        assert_eq!(err.message(), "yield: wait-for-file /tmp/test (5000ms)");
    }

    #[test]
    fn test_is_yield_false_for_normal_errors() {
        assert!(!LispError::user("err", vec![]).is_yield());
        assert!(!LispError::type_error("a", "b").is_yield());
        assert!(!LispError::undefined("x").is_yield());
        assert!(!LispError::internal("bug").is_yield());
        assert!(!LispError::division_by_zero().is_yield());
        assert!(!LispError::immutable("pair").is_yield());
        assert!(!LispError::read("bad").is_yield());
        assert!(!LispError::io("fail", None).is_yield());
        assert!(!LispError::arity("f", Arity::Fixed(1), 2).is_yield());
    }
}
