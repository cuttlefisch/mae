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
}

impl LispError {
    pub fn read(msg: impl Into<String>) -> Self {
        LispError {
            kind: ErrorKind::Read(msg.into()),
            location: None,
            stack_trace: Vec::new(),
        }
    }

    pub fn read_at(msg: impl Into<String>, loc: SourceLocation) -> Self {
        LispError {
            kind: ErrorKind::Read(msg.into()),
            location: Some(loc),
            stack_trace: Vec::new(),
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
        }
    }

    pub fn undefined(name: impl Into<String>) -> Self {
        LispError {
            kind: ErrorKind::Undefined { name: name.into() },
            location: None,
            stack_trace: Vec::new(),
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
        }
    }

    pub fn division_by_zero() -> Self {
        LispError {
            kind: ErrorKind::DivisionByZero,
            location: None,
            stack_trace: Vec::new(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        LispError {
            kind: ErrorKind::Internal(msg.into()),
            location: None,
            stack_trace: Vec::new(),
        }
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
}
