//! Scheme introspection — function documentation, apropos, profiling.
//!
//! Provides `(describe)`, `(apropos)`, `(gc-stats)`, `(gc-collect!)`,
//! `(procedure-arity)`, `(procedure-documentation)`, `(procedure-name)`
//! and related Scheme-callable introspection primitives.
//!
//! @stability: unstable (Phase 13h)
//! @since: 0.12.0

use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

/// Structured function documentation extracted from the live VM.
#[derive(Clone, Debug)]
pub struct FunctionDoc {
    pub name: String,
    pub doc: String,
    pub arity: Arity,
    pub kind: FunctionKind,
    /// Source file if available (closures only).
    pub source_file: Option<String>,
    /// Source line if available (closures only).
    pub source_line: Option<u32>,
}

/// What kind of function this is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FunctionKind {
    Foreign,
    Closure,
    Macro,
}

impl std::fmt::Display for FunctionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FunctionKind::Foreign => write!(f, "built-in"),
            FunctionKind::Closure => write!(f, "user-defined"),
            FunctionKind::Macro => write!(f, "macro"),
        }
    }
}

/// Collect documentation for all named functions/macros in the VM.
pub fn function_registry(vm: &Vm) -> Vec<FunctionDoc> {
    let mut docs = Vec::new();

    for (name, value) in vm.globals.iter() {
        match value {
            Value::Foreign(ff) => {
                docs.push(FunctionDoc {
                    name: name.clone(),
                    doc: ff.doc.clone(),
                    arity: ff.arity.clone(),
                    kind: FunctionKind::Foreign,
                    source_file: None,
                    source_line: None,
                });
            }
            Value::Closure(c) => {
                let (file, line) = closure_source(vm, c.code_id);
                docs.push(FunctionDoc {
                    name: name.clone(),
                    doc: c.doc.clone().unwrap_or_default(),
                    arity: c.arity.clone(),
                    kind: FunctionKind::Closure,
                    source_file: file,
                    source_line: line,
                });
            }
            _ => {}
        }
    }

    // Add macros
    for name in vm.macros().keys() {
        docs.push(FunctionDoc {
            name: name.clone(),
            doc: String::new(),
            arity: Arity::Variadic(0),
            kind: FunctionKind::Macro,
            source_file: None,
            source_line: None,
        });
    }

    docs.sort_by(|a, b| a.name.cmp(&b.name));
    docs
}

/// Look up a single function's documentation by name.
pub fn describe_function(vm: &Vm, name: &str) -> Option<FunctionDoc> {
    if let Some(value) = vm.globals.get(name) {
        match value {
            Value::Foreign(ff) => {
                return Some(FunctionDoc {
                    name: name.to_string(),
                    doc: ff.doc.clone(),
                    arity: ff.arity.clone(),
                    kind: FunctionKind::Foreign,
                    source_file: None,
                    source_line: None,
                });
            }
            Value::Closure(c) => {
                let (file, line) = closure_source(vm, c.code_id);
                return Some(FunctionDoc {
                    name: name.to_string(),
                    doc: c.doc.clone().unwrap_or_default(),
                    arity: c.arity.clone(),
                    kind: FunctionKind::Closure,
                    source_file: file,
                    source_line: line,
                });
            }
            _ => {}
        }
    }

    // Check macros
    if vm.macros().contains_key(name) {
        return Some(FunctionDoc {
            name: name.to_string(),
            doc: String::new(),
            arity: Arity::Variadic(0),
            kind: FunctionKind::Macro,
            source_file: None,
            source_line: None,
        });
    }

    None
}

/// Search for functions matching a pattern (substring match).
pub fn apropos(vm: &Vm, pattern: &str) -> Vec<FunctionDoc> {
    let pattern_lower = pattern.to_lowercase();
    function_registry(vm)
        .into_iter()
        .filter(|d| d.name.to_lowercase().contains(&pattern_lower))
        .collect()
}

/// Format a FunctionDoc as a human-readable string.
pub fn format_doc(doc: &FunctionDoc) -> String {
    let mut out = String::new();

    out.push_str(&format!("  {} — {}\n", doc.name, doc.kind));
    out.push_str(&format!("  Arity: {}\n", doc.arity));

    if !doc.doc.is_empty() {
        out.push_str(&format!("  {}\n", doc.doc));
    }

    if let Some(ref file) = doc.source_file {
        if let Some(line) = doc.source_line {
            out.push_str(&format!("  Defined in {}:{}\n", file, line));
        } else {
            out.push_str(&format!("  Defined in {}\n", file));
        }
    }

    out
}

/// Extract source file and line for a closure's code object.
fn closure_source(vm: &Vm, code_id: usize) -> (Option<String>, Option<u32>) {
    if let Some(code) = vm.code_pool().get(code_id) {
        // Try CodeObject.source first, then fall back to first source_map entry
        let loc = code
            .source
            .as_ref()
            .or_else(|| code.source_map.iter().flatten().next());
        if let Some(loc) = loc {
            let file = if loc.file == "<eval>" {
                None
            } else {
                Some(loc.file.clone())
            };
            (file, Some(loc.line))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    }
}

/// Register introspection primitives in the VM.
pub fn register_introspection(vm: &mut Vm) {
    // (procedure-arity proc) → string representation of arity
    vm.register_fn(
        "procedure-arity",
        "Return the arity of a procedure as a string.",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Foreign(ff) => Ok(Value::string(ff.arity.to_string())),
            Value::Closure(c) => Ok(Value::string(c.arity.to_string())),
            _ => Err(LispError::type_error("procedure", args[0].type_name())),
        },
    );

    // (procedure-documentation proc) → string or #f
    vm.register_fn(
        "procedure-documentation",
        "Return the documentation string of a procedure, or #f if none.",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Foreign(ff) => {
                if ff.doc.is_empty() {
                    Ok(Value::Bool(false))
                } else {
                    Ok(Value::string(&ff.doc))
                }
            }
            Value::Closure(c) => match &c.doc {
                Some(d) if !d.is_empty() => Ok(Value::string(d)),
                _ => Ok(Value::Bool(false)),
            },
            _ => Err(LispError::type_error("procedure", args[0].type_name())),
        },
    );

    // (procedure-name proc) → string or #f
    vm.register_fn(
        "procedure-name",
        "Return the name of a procedure, or #f if anonymous.",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Foreign(ff) => Ok(Value::string(&ff.name)),
            Value::Closure(c) => match &c.name {
                Some(n) => Ok(Value::string(n)),
                None => Ok(Value::Bool(false)),
            },
            _ => Err(LispError::type_error("procedure", args[0].type_name())),
        },
    );

    // (gc-collect!) → void (triggers Rc cleanup / future GC cycle)
    vm.register_fn(
        "gc-collect!",
        "Trigger garbage collection (currently increments counter in Rc stage).",
        Arity::Fixed(0),
        |_args| {
            // In Stage 1 (Rc), there's no real GC to trigger.
            // The VM increments gc_stats.collections_count when this runs.
            Ok(Value::Void)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::register_stdlib;

    fn test_vm() -> Vm {
        let mut vm = Vm::new();
        register_stdlib(&mut vm);
        register_introspection(&mut vm);
        vm
    }

    #[test]
    fn function_registry_contains_builtins() {
        let vm = test_vm();
        let docs = function_registry(&vm);
        assert!(
            docs.len() > 50,
            "should have many functions: {}",
            docs.len()
        );
        assert!(docs.iter().any(|d| d.name == "car"), "should contain car");
        assert!(docs.iter().any(|d| d.name == "cdr"), "should contain cdr");
    }

    #[test]
    fn function_registry_includes_user_defined() {
        let mut vm = test_vm();
        vm.eval("(define (my-fn x) \"my doc\" x)").unwrap();
        let docs = function_registry(&vm);
        let my_fn = docs.iter().find(|d| d.name == "my-fn");
        assert!(my_fn.is_some(), "should contain user-defined function");
        let doc = my_fn.unwrap();
        assert_eq!(doc.kind, FunctionKind::Closure);
        assert_eq!(doc.doc, "my doc");
    }

    #[test]
    fn function_registry_includes_macros() {
        let mut vm = test_vm();
        vm.eval("(define-syntax my-mac (syntax-rules () ((my-mac) 42)))")
            .unwrap();
        let docs = function_registry(&vm);
        assert!(
            docs.iter()
                .any(|d| d.name == "my-mac" && d.kind == FunctionKind::Macro),
            "should contain user-defined macro"
        );
    }

    #[test]
    fn describe_function_foreign() {
        let vm = test_vm();
        let doc = describe_function(&vm, "car");
        assert!(doc.is_some());
        let doc = doc.unwrap();
        assert_eq!(doc.kind, FunctionKind::Foreign);
        assert!(!doc.doc.is_empty(), "car should have documentation");
    }

    #[test]
    fn describe_function_closure_with_source() {
        let mut vm = test_vm();
        vm.eval_with_file("(define (greet name) name)", "hello.scm")
            .unwrap();
        let doc = describe_function(&vm, "greet");
        assert!(doc.is_some());
        let doc = doc.unwrap();
        assert_eq!(doc.kind, FunctionKind::Closure);
        assert_eq!(doc.source_file.as_deref(), Some("hello.scm"));
        assert_eq!(doc.source_line, Some(1));
    }

    #[test]
    fn describe_function_not_found() {
        let vm = test_vm();
        assert!(describe_function(&vm, "nonexistent-fn-xyz").is_none());
    }

    #[test]
    fn describe_function_macro() {
        let mut vm = test_vm();
        vm.eval("(define-syntax my-when (syntax-rules () ((my-when t b) (if t b #f))))")
            .unwrap();
        let doc = describe_function(&vm, "my-when");
        assert!(doc.is_some());
        assert_eq!(doc.unwrap().kind, FunctionKind::Macro);
    }

    #[test]
    fn describe_function_variable_returns_none() {
        let mut vm = test_vm();
        vm.eval("(define my-var 42)").unwrap();
        // Variables (non-procedures) should not be described as functions
        assert!(describe_function(&vm, "my-var").is_none());
    }

    #[test]
    fn apropos_filters_by_pattern() {
        let vm = test_vm();
        let results = apropos(&vm, "car");
        assert!(!results.is_empty(), "should find car-related functions");
        for r in &results {
            assert!(
                r.name.to_lowercase().contains("car"),
                "apropos result '{}' should contain 'car'",
                r.name
            );
        }
    }

    #[test]
    fn apropos_case_insensitive() {
        let vm = test_vm();
        let lower = apropos(&vm, "car");
        let upper = apropos(&vm, "CAR");
        assert_eq!(lower.len(), upper.len());
    }

    #[test]
    fn apropos_empty_pattern_returns_all() {
        let vm = test_vm();
        let all = apropos(&vm, "");
        let registry = function_registry(&vm);
        assert_eq!(all.len(), registry.len());
    }

    #[test]
    fn apropos_no_matches() {
        let vm = test_vm();
        let results = apropos(&vm, "zzz-nonexistent-xyz");
        assert!(results.is_empty());
    }

    #[test]
    fn format_doc_includes_all_fields() {
        let doc = FunctionDoc {
            name: "test-fn".to_string(),
            doc: "A test function.".to_string(),
            arity: Arity::Fixed(2),
            kind: FunctionKind::Foreign,
            source_file: None,
            source_line: None,
        };
        let formatted = format_doc(&doc);
        assert!(formatted.contains("test-fn"));
        assert!(formatted.contains("built-in"));
        assert!(formatted.contains("2"));
        assert!(formatted.contains("A test function."));
    }

    #[test]
    fn format_doc_with_source_location() {
        let doc = FunctionDoc {
            name: "user-fn".to_string(),
            doc: String::new(),
            arity: Arity::Variadic(1),
            kind: FunctionKind::Closure,
            source_file: Some("init.scm".to_string()),
            source_line: Some(42),
        };
        let formatted = format_doc(&doc);
        assert!(formatted.contains("init.scm:42"));
        assert!(formatted.contains("1+"));
    }

    #[test]
    fn format_doc_no_doc_no_source() {
        let doc = FunctionDoc {
            name: "bare-fn".to_string(),
            doc: String::new(),
            arity: Arity::Fixed(0),
            kind: FunctionKind::Foreign,
            source_file: None,
            source_line: None,
        };
        let formatted = format_doc(&doc);
        assert!(formatted.contains("bare-fn"));
        assert!(!formatted.contains("Defined in"));
    }

    #[test]
    fn procedure_arity_foreign() {
        let mut vm = test_vm();
        let result = vm.eval("(procedure-arity car)").unwrap();
        assert_eq!(result.to_string(), "\"1\"");
    }

    #[test]
    fn procedure_arity_closure() {
        let mut vm = test_vm();
        vm.eval("(define (f x y) x)").unwrap();
        let result = vm.eval("(procedure-arity f)").unwrap();
        assert_eq!(result.to_string(), "\"2\"");
    }

    #[test]
    fn procedure_arity_variadic() {
        let mut vm = test_vm();
        vm.eval("(define (g x . rest) x)").unwrap();
        let result = vm.eval("(procedure-arity g)").unwrap();
        assert_eq!(result.to_string(), "\"1+\"");
    }

    #[test]
    fn procedure_documentation_foreign() {
        let mut vm = test_vm();
        let result = vm.eval("(procedure-documentation car)").unwrap();
        // car should have a doc string
        assert_ne!(result, Value::Bool(false), "car should have documentation");
    }

    #[test]
    fn procedure_documentation_with_docstring() {
        let mut vm = test_vm();
        vm.eval("(define (documented x) \"This is my doc.\" x)")
            .unwrap();
        let result = vm.eval("(procedure-documentation documented)").unwrap();
        assert_eq!(result.to_string(), "\"This is my doc.\"");
    }

    #[test]
    fn procedure_documentation_none() {
        let mut vm = test_vm();
        vm.eval("(define (bare x) x)").unwrap();
        let result = vm.eval("(procedure-documentation bare)").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn procedure_name_foreign() {
        let mut vm = test_vm();
        let result = vm.eval("(procedure-name car)").unwrap();
        assert_eq!(result.to_string(), "\"car\"");
    }

    #[test]
    fn procedure_name_closure() {
        let mut vm = test_vm();
        vm.eval("(define (my-proc x) x)").unwrap();
        let result = vm.eval("(procedure-name my-proc)").unwrap();
        assert_eq!(result.to_string(), "\"my-proc\"");
    }

    #[test]
    fn procedure_name_anonymous() {
        let mut vm = test_vm();
        vm.eval("(define anon (lambda (x) x))").unwrap();
        let result = vm.eval("(procedure-name anon)").unwrap();
        // Lambda without explicit name
        // Could be #f or "anon" depending on compiler
        assert!(
            result == Value::Bool(false) || result.to_string().contains("anon"),
            "anonymous lambda name: {}",
            result
        );
    }

    #[test]
    fn procedure_arity_type_error() {
        let mut vm = test_vm();
        let result = vm.eval("(procedure-arity 42)");
        assert!(result.is_err());
    }

    #[test]
    fn procedure_documentation_type_error() {
        let mut vm = test_vm();
        let result = vm.eval("(procedure-documentation \"not-a-proc\")");
        assert!(result.is_err());
    }

    #[test]
    fn gc_collect_runs() {
        let mut vm = test_vm();
        let result = vm.eval("(gc-collect!)");
        assert!(result.is_ok());
    }

    #[test]
    fn function_registry_sorted() {
        let vm = test_vm();
        let docs = function_registry(&vm);
        for w in docs.windows(2) {
            assert!(w[0].name <= w[1].name, "registry should be sorted");
        }
    }
}
