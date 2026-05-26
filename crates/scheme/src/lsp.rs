//! In-process Scheme LSP — Swank-style introspection for mae-scheme.
//!
//! Unlike external LSP servers, this queries the live VM's globals,
//! code pool, and library registry directly. No subprocess needed.
//!
//! Architecture inspired by SLIME/Swank (Common Lisp): the runtime is
//! embedded in the editor, so we get live symbol table completion,
//! docstring hover, and check-syntax diagnostics without a separate process.
//!
//! Prior art survey: no Scheme implementation has this level of integration.
//! - scheme-lsp-server (rgherdt): external, REPL-based completion
//! - scheme-langserver (Chez): external, static analysis
//! - racket-langserver: external, check-syntax expansion
//! - SLIME/Swank: in-process (our model) — gold standard
//!
//! @stability: unstable (Phase 13g)
//! @since: 0.12.0

use crate::compiler::Compiler;
use crate::lisp_error::Arity;
use crate::reader;
use crate::value::Value;
use crate::vm::Vm;

/// A completion candidate returned by the Scheme LSP.
#[derive(Debug, Clone)]
pub struct SchemeCompletion {
    /// The symbol name.
    pub label: String,
    /// The kind of symbol (function, variable, keyword, etc.).
    pub kind: SchemeSymbolKind,
    /// Short documentation string.
    pub detail: Option<String>,
    /// Arity information (for functions).
    pub arity: Option<String>,
}

/// A hover result returned by the Scheme LSP.
#[derive(Debug, Clone)]
pub struct SchemeHover {
    /// Formatted documentation text (markdown).
    pub contents: String,
}

/// A diagnostic from check-syntax (compile without executing).
#[derive(Debug, Clone)]
pub struct SchemeDiagnostic {
    /// 0-indexed line number.
    pub line: u32,
    /// 0-indexed column.
    pub column: u32,
    /// Error message.
    pub message: String,
    /// Severity level.
    pub severity: SchemeDiagnosticSeverity,
}

#[derive(Debug, Clone)]
pub enum SchemeDiagnosticSeverity {
    Error,
    Warning,
}

/// A document symbol (top-level define).
#[derive(Debug, Clone)]
pub struct SchemeDocumentSymbol {
    /// The symbol name.
    pub name: String,
    /// The kind of symbol.
    pub kind: SchemeSymbolKind,
    /// 0-indexed line number where the symbol is defined.
    pub line: u32,
}

/// Symbol kinds for completion and document symbols.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SchemeSymbolKind {
    Function,
    Variable,
    Keyword,
    Macro,
}

/// Signature help information.
#[derive(Debug, Clone)]
pub struct SchemeSignatureHelp {
    /// Formatted signature string (e.g., "(map proc list1 list2 ...)").
    pub label: String,
    /// Documentation string.
    pub documentation: Option<String>,
    /// Parameter labels.
    pub parameters: Vec<String>,
}

/// R7RS special form keywords for completion.
const R7RS_KEYWORDS: &[&str] = &[
    "begin",
    "case",
    "cond",
    "define",
    "define-library",
    "define-record-type",
    "define-syntax",
    "define-values",
    "do",
    "else",
    "guard",
    "if",
    "import",
    "include",
    "include-ci",
    "lambda",
    "let",
    "let*",
    "let-syntax",
    "let-values",
    "letrec",
    "letrec*",
    "letrec-syntax",
    "or",
    "and",
    "not",
    "quasiquote",
    "quote",
    "set!",
    "syntax-rules",
    "unless",
    "unquote",
    "unquote-splicing",
    "when",
    "with-exception-handler",
];

/// Format an arity specification for display.
fn format_arity(arity: &Arity) -> String {
    match arity {
        Arity::Fixed(n) => format!("{n} args"),
        Arity::Variadic(n) => format!("{n}+ args"),
        Arity::Multi(ns) => {
            let parts: Vec<String> = ns.iter().map(|n| n.to_string()).collect();
            format!("{} args", parts.join(" or "))
        }
    }
}

/// Extract the word (symbol) at or before a given column in a line of text.
/// Returns (word, start_col).
fn word_at_position(line_text: &str, col: u32) -> (String, u32) {
    let col = col as usize;
    let bytes = line_text.as_bytes();

    // Find start of word (scan backwards from col)
    let mut start = col.min(bytes.len());
    while start > 0 {
        let c = bytes[start - 1] as char;
        if c.is_whitespace() || c == '(' || c == ')' || c == '[' || c == ']' || c == '"' {
            break;
        }
        start -= 1;
    }

    // Find end of word (scan forwards)
    let mut end = col.min(bytes.len());
    while end < bytes.len() {
        let c = bytes[end] as char;
        if c.is_whitespace() || c == '(' || c == ')' || c == '[' || c == ']' || c == '"' {
            break;
        }
        end += 1;
    }

    let word = &line_text[start..end];
    (word.to_string(), start as u32)
}

/// Get completion candidates from the live VM state.
///
/// Queries the global environment for all defined symbols, plus R7RS keywords.
/// Like SLIME/Swank: completion against the live symbol table, always
/// complete and current — no index staleness.
pub fn completions(vm: &Vm, prefix: &str) -> Vec<SchemeCompletion> {
    let mut results = Vec::new();
    let prefix_lower = prefix.to_lowercase();

    // R7RS keywords
    for kw in R7RS_KEYWORDS {
        if kw.starts_with(&prefix_lower) {
            results.push(SchemeCompletion {
                label: kw.to_string(),
                kind: SchemeSymbolKind::Keyword,
                detail: Some("R7RS keyword".into()),
                arity: None,
            });
        }
    }

    // Globals from the live VM
    for (name, value) in vm.globals.iter() {
        if !name.starts_with(&prefix_lower) && !name.contains(&prefix_lower) {
            continue;
        }

        let (kind, detail, arity) = match value {
            Value::Foreign(f) => (
                SchemeSymbolKind::Function,
                if f.doc.is_empty() {
                    None
                } else {
                    Some(f.doc.clone())
                },
                Some(format_arity(&f.arity)),
            ),
            Value::Closure(c) => (
                SchemeSymbolKind::Function,
                c.doc.clone(),
                Some(format_arity(&c.arity)),
            ),
            _ => (SchemeSymbolKind::Variable, None, None),
        };

        results.push(SchemeCompletion {
            label: name.clone(),
            kind,
            detail,
            arity,
        });
    }

    // Macros
    for name in vm.macros().keys() {
        if name.starts_with(&prefix_lower) || name.contains(&prefix_lower) {
            results.push(SchemeCompletion {
                label: name.clone(),
                kind: SchemeSymbolKind::Macro,
                detail: Some("macro".into()),
                arity: None,
            });
        }
    }

    // Sort: exact prefix matches first, then by name
    results.sort_by(|a, b| {
        let a_prefix = a.label.starts_with(&prefix_lower);
        let b_prefix = b.label.starts_with(&prefix_lower);
        b_prefix.cmp(&a_prefix).then(a.label.cmp(&b.label))
    });

    results
}

/// Get hover information for a symbol.
///
/// Returns the docstring, arity, and type for any symbol visible in the VM.
/// Like SLIME's `describe-symbol`: queries the live runtime for documentation.
pub fn hover(vm: &Vm, symbol: &str) -> Option<SchemeHover> {
    // Check globals
    if let Some(value) = vm.globals.get(symbol) {
        let contents = match value {
            Value::Foreign(f) => {
                let mut s = format!("**{}** — foreign function\n\n", f.name);
                s.push_str(&format!("Arity: {}\n\n", format_arity(&f.arity)));
                if !f.doc.is_empty() {
                    s.push_str(&f.doc);
                }
                s
            }
            Value::Closure(c) => {
                let name = c.name.as_deref().unwrap_or(symbol);
                let mut s = format!("**{}** — procedure\n\n", name);
                s.push_str(&format!("Arity: {}\n\n", format_arity(&c.arity)));
                if let Some(doc) = &c.doc {
                    s.push_str(doc);
                }
                s
            }
            _ => {
                format!("**{}** — {}\n\nValue: {}", symbol, value.type_name(), value)
            }
        };
        return Some(SchemeHover { contents });
    }

    // Check macros
    if vm.macros().contains_key(symbol) {
        return Some(SchemeHover {
            contents: format!("**{}** — syntax (macro)", symbol),
        });
    }

    // Check R7RS keywords
    if R7RS_KEYWORDS.contains(&symbol) {
        return Some(SchemeHover {
            contents: format!("**{}** — R7RS special form", symbol),
        });
    }

    None
}

/// Check-syntax: compile without executing, report diagnostics.
///
/// Follows the racket-langserver pattern: expand/compile the source but
/// don't evaluate. Captures syntax errors, undefined variables, and
/// arity mismatches from the compiler.
pub fn diagnostics(vm: &Vm, source: &str, file: &str) -> Vec<SchemeDiagnostic> {
    let mut results = Vec::new();

    // Try to read (parse) the source
    let mut rdr = reader::Reader::new(source, file);
    let datums = match rdr.read_all() {
        Ok(d) => d,
        Err(e) => {
            let loc = e.location.as_ref();
            results.push(SchemeDiagnostic {
                line: loc.map(|l| l.line.saturating_sub(1)).unwrap_or(0),
                column: loc.map(|l| l.column.saturating_sub(1)).unwrap_or(0),
                message: e.message(),
                severity: SchemeDiagnosticSeverity::Error,
            });
            return results;
        }
    };

    if datums.is_empty() {
        return results;
    }

    // Try to compile (without executing)
    let mut compiler = Compiler::new();
    compiler.macros = vm.macros().clone();
    compiler.load_paths = vm.load_paths.clone();

    // Filter out imports/define-library for compilation
    let to_compile: Vec<_> = datums
        .iter()
        .filter(|d| !is_import_form(d) && !is_define_library_form(d))
        .cloned()
        .collect();

    if to_compile.is_empty() {
        return results;
    }

    if let Err(e) = compiler.compile_top_level(&to_compile) {
        let loc = e.location.as_ref();
        results.push(SchemeDiagnostic {
            line: loc.map(|l| l.line.saturating_sub(1)).unwrap_or(0),
            column: loc.map(|l| l.column.saturating_sub(1)).unwrap_or(0),
            message: e.message(),
            severity: SchemeDiagnosticSeverity::Error,
        });
    }

    results
}

/// Extract document symbols (top-level defines) from source text.
///
/// Parses the source and identifies `define`, `define-syntax`,
/// `define-record-type`, and `define-library` forms.
pub fn document_symbols(source: &str, _file: &str) -> Vec<SchemeDocumentSymbol> {
    let mut symbols = Vec::new();
    let lines: Vec<&str> = source.lines().collect();

    for (line_no, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // (define name ...)
        // (define (name args...) ...)
        if let Some(rest) = trimmed.strip_prefix("(define ") {
            if let Some(name) = extract_define_name(rest) {
                let kind = if rest.starts_with('(') {
                    SchemeSymbolKind::Function
                } else {
                    SchemeSymbolKind::Variable
                };
                symbols.push(SchemeDocumentSymbol {
                    name,
                    kind,
                    line: line_no as u32,
                });
            }
        }
        // (define-syntax name ...)
        else if let Some(rest) = trimmed.strip_prefix("(define-syntax ") {
            if let Some(name) = extract_first_symbol(rest) {
                symbols.push(SchemeDocumentSymbol {
                    name,
                    kind: SchemeSymbolKind::Macro,
                    line: line_no as u32,
                });
            }
        }
        // (define-record-type name ...)
        else if let Some(rest) = trimmed.strip_prefix("(define-record-type ") {
            if let Some(name) = extract_first_symbol(rest) {
                symbols.push(SchemeDocumentSymbol {
                    name,
                    kind: SchemeSymbolKind::Variable,
                    line: line_no as u32,
                });
            }
        }
        // (define-library (name ...) ...)
        else if let Some(rest) = trimmed.strip_prefix("(define-library ") {
            if let Some(name) = extract_library_name(rest) {
                symbols.push(SchemeDocumentSymbol {
                    name,
                    kind: SchemeSymbolKind::Variable,
                    line: line_no as u32,
                });
            }
        }
    }

    symbols
}

/// Get signature help for a function.
///
/// Looks up the function in the VM and returns its arity and parameter info.
pub fn signature_help(vm: &Vm, symbol: &str) -> Option<SchemeSignatureHelp> {
    if let Some(value) = vm.globals.get(symbol) {
        match value {
            Value::Foreign(f) => {
                let params = make_param_labels(&f.arity);
                let label = format!("({} {})", f.name, params.join(" "));
                Some(SchemeSignatureHelp {
                    label,
                    documentation: if f.doc.is_empty() {
                        None
                    } else {
                        Some(f.doc.clone())
                    },
                    parameters: params,
                })
            }
            Value::Closure(c) => {
                let name = c.name.as_deref().unwrap_or(symbol);
                let params = make_param_labels(&c.arity);
                let label = format!("({} {})", name, params.join(" "));
                Some(SchemeSignatureHelp {
                    label,
                    documentation: c.doc.clone(),
                    parameters: params,
                })
            }
            _ => None,
        }
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_param_labels(arity: &Arity) -> Vec<String> {
    match arity {
        Arity::Fixed(n) => (0..*n).map(|i| format!("arg{}", i + 1)).collect(),
        Arity::Variadic(n) => {
            let mut params: Vec<String> = (0..*n).map(|i| format!("arg{}", i + 1)).collect();
            params.push("...".into());
            params
        }
        Arity::Multi(ns) => {
            let max = ns.iter().max().copied().unwrap_or(0);
            (0..max).map(|i| format!("arg{}", i + 1)).collect()
        }
    }
}

fn is_import_form(v: &Value) -> bool {
    if let Value::Pair(p) = v {
        if let Value::Symbol(s) = &p.0 {
            return s.name() == "import";
        }
    }
    false
}

fn is_define_library_form(v: &Value) -> bool {
    if let Value::Pair(p) = v {
        if let Value::Symbol(s) = &p.0 {
            return s.name() == "define-library";
        }
    }
    false
}

/// Extract the name from a define form rest string.
/// "(name args...)" → "name"
/// "name value" → "name"
fn extract_define_name(rest: &str) -> Option<String> {
    let rest = rest.trim();
    if let Some(inner) = rest.strip_prefix('(') {
        // (define (name args...) body)
        extract_first_symbol(inner)
    } else {
        extract_first_symbol(rest)
    }
}

/// Extract the first symbol-like token from text.
fn extract_first_symbol(text: &str) -> Option<String> {
    let text = text.trim();
    let end = text
        .find(|c: char| c.is_whitespace() || c == ')' || c == '(')
        .unwrap_or(text.len());
    let sym = &text[..end];
    if sym.is_empty() {
        None
    } else {
        Some(sym.to_string())
    }
}

/// Extract library name from "(name parts...)" form.
fn extract_library_name(text: &str) -> Option<String> {
    let text = text.trim();
    if !text.starts_with('(') {
        return extract_first_symbol(text);
    }
    // Find matching close paren
    let end = text.find(')')?;
    Some(text[1..end].to_string())
}

/// Extract the word (symbol) at or before a given column in a line of text.
/// Public alias for use by the LSP bridge.
pub fn extract_word_at(line_text: &str, col: u32) -> (String, u32) {
    word_at_position(line_text, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_at_position() {
        let (word, start) = word_at_position("(define foo 42)", 9);
        assert_eq!(word, "foo");
        assert_eq!(start, 8);
    }

    #[test]
    fn test_word_at_position_start_of_line() {
        let (word, _) = word_at_position("buffer-insert", 5);
        assert_eq!(word, "buffer-insert");
    }

    #[test]
    fn test_word_at_position_after_paren() {
        let (word, _) = word_at_position("(map f xs)", 3);
        assert_eq!(word, "map");
    }

    #[test]
    fn test_completions_keywords() {
        let vm = Vm::new();
        let results = completions(&vm, "def");
        assert!(results.iter().any(|c| c.label == "define"));
        assert!(results.iter().any(|c| c.label == "define-syntax"));
    }

    #[test]
    fn test_completions_globals() {
        let mut vm = Vm::new();
        vm.register_fn("buffer-insert", "Insert text", Arity::Fixed(1), |_| {
            Ok(Value::Void)
        });
        let results = completions(&vm, "buffer");
        assert!(results.iter().any(|c| c.label == "buffer-insert"));
    }

    #[test]
    fn test_hover_foreign() {
        let mut vm = Vm::new();
        vm.register_fn(
            "buffer-insert",
            "Insert text at point",
            Arity::Fixed(1),
            |_| Ok(Value::Void),
        );
        let h = hover(&vm, "buffer-insert").unwrap();
        assert!(h.contents.contains("Insert text at point"));
        assert!(h.contents.contains("1 args"));
    }

    #[test]
    fn test_hover_keyword() {
        let vm = Vm::new();
        let h = hover(&vm, "lambda").unwrap();
        assert!(h.contents.contains("R7RS special form"));
    }

    #[test]
    fn test_hover_missing() {
        let vm = Vm::new();
        assert!(hover(&vm, "nonexistent-xyz").is_none());
    }

    #[test]
    fn test_diagnostics_parse_error() {
        let vm = Vm::new();
        let diags = diagnostics(&vm, "(define x", "test.scm");
        assert!(!diags.is_empty());
        assert!(diags[0].message.contains("unterminated"));
    }

    #[test]
    fn test_diagnostics_clean() {
        let vm = Vm::new();
        let diags = diagnostics(&vm, "(define x 42)", "test.scm");
        assert!(diags.is_empty());
    }

    #[test]
    fn test_document_symbols() {
        let source = "(define (foo x) (+ x 1))\n(define bar 42)\n(define-syntax my-mac\n  (syntax-rules () ((my-mac) 1)))";
        let syms = document_symbols(source, "test.scm");
        assert_eq!(syms.len(), 3);
        assert_eq!(syms[0].name, "foo");
        assert_eq!(syms[0].kind, SchemeSymbolKind::Function);
        assert_eq!(syms[1].name, "bar");
        assert_eq!(syms[1].kind, SchemeSymbolKind::Variable);
        assert_eq!(syms[2].name, "my-mac");
        assert_eq!(syms[2].kind, SchemeSymbolKind::Macro);
    }

    #[test]
    fn test_signature_help() {
        let mut vm = Vm::new();
        vm.register_fn("map", "Apply proc to list", Arity::Variadic(2), |_| {
            Ok(Value::Void)
        });
        let sig = signature_help(&vm, "map").unwrap();
        assert!(sig.label.contains("map"));
        assert_eq!(sig.parameters.len(), 3); // arg1, arg2, ...
    }

    #[test]
    fn test_completions_macros() {
        let mut vm = Vm::new();
        // Define a macro via eval — the proper way to populate vm.macros
        vm.eval("(define-syntax my-when (syntax-rules () ((my-when test body) (if test body))))")
            .unwrap();
        let results = completions(&vm, "my-");
        assert!(results.iter().any(|c| c.label == "my-when"));
        assert_eq!(
            results.iter().find(|c| c.label == "my-when").unwrap().kind,
            SchemeSymbolKind::Macro
        );
    }
}
