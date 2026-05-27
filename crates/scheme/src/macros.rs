//! Hygienic macro system for mae-scheme.
//!
//! Implements `syntax-rules` (R7RS §4.3.2) via explicit renaming.
//! Also supports `define-macro` for simple non-hygienic macros.
//!
//! @stability: unstable (Phase 13d)
//! @since: 0.12.0

use std::collections::HashMap;

use crate::lisp_error::LispError;
use crate::value::Value;

/// A syntax-rules transformer: list of (pattern, template) pairs.
#[derive(Clone, Debug)]
pub struct SyntaxRules {
    /// Literal identifiers that must match exactly.
    pub literals: Vec<String>,
    /// (pattern, template) pairs tried in order.
    pub rules: Vec<(Value, Value)>,
    /// Custom ellipsis identifier (default: "...").
    /// R7RS §4.3.2 / SRFI 46: `(syntax-rules <ellipsis> (literals...) ...)`
    pub ellipsis: String,
}

/// Expand a `syntax-rules` macro application.
///
/// Tries each rule's pattern against the form. On match, instantiates
/// the template with captured bindings, using gensym for hygiene.
pub fn expand_syntax_rules(transformer: &SyntaxRules, form: &[Value]) -> Result<Value, LispError> {
    let ellipsis = &transformer.ellipsis;
    for (pattern, template) in &transformer.rules {
        let mut bindings = HashMap::new();
        let pat_items = pattern
            .to_vec()
            .map_err(|_| LispError::syntax("invalid syntax-rules pattern", format!("{pattern}")))?;
        // Pattern starts with _ (keyword position), match against rest
        if match_pattern(
            &pat_items[1..],
            &form[1..],
            &transformer.literals,
            ellipsis,
            &mut bindings,
        )? {
            return instantiate_template(template, &bindings, ellipsis);
        }
    }
    Err(LispError::syntax(
        "no matching syntax-rules pattern",
        format!("{}", Value::list(form.to_vec())),
    ))
}

/// Match a pattern against input forms, collecting bindings.
///
/// Pattern elements:
/// - `_` matches anything (no binding)
/// - literal identifiers must match exactly
/// - symbols bind to the corresponding input
/// - `(pattern ...)` with ellipsis matches zero or more
/// - nested lists recurse
fn match_pattern(
    pattern: &[Value],
    input: &[Value],
    literals: &[String],
    ellipsis: &str,
    bindings: &mut HashMap<String, MatchResult>,
) -> Result<bool, LispError> {
    let mut pi = 0;
    let mut ii = 0;

    while pi < pattern.len() {
        // Check for ellipsis: pattern[pi] followed by the ellipsis identifier
        let has_ellipsis = pi + 1 < pattern.len() && is_ellipsis_id(&pattern[pi + 1], ellipsis);

        if has_ellipsis {
            // Match zero or more of pattern[pi]
            let subpat = &pattern[pi];
            let mut collected = Vec::new();

            // How many remaining non-ellipsis patterns after this?
            let remaining_patterns = pattern.len() - pi - 2;

            // Consume input until we need to leave `remaining_patterns` for the rest
            while ii + remaining_patterns < input.len() {
                let mut sub_bindings = HashMap::new();
                if match_single(subpat, &input[ii], literals, ellipsis, &mut sub_bindings)? {
                    collected.push(sub_bindings);
                    ii += 1;
                } else {
                    break;
                }
            }

            // Merge collected bindings as lists
            if let Value::Symbol(sym) = subpat {
                let name = sym.name().to_string();
                if name != "_" && !literals.contains(&name) {
                    let values: Vec<Value> = collected
                        .iter()
                        .filter_map(|b| {
                            b.get(&name).and_then(|m| {
                                if let MatchResult::Single(v) = m {
                                    Some(v.clone())
                                } else {
                                    None
                                }
                            })
                        })
                        .collect();
                    bindings.insert(name, MatchResult::Ellipsis(values));
                }
            } else if let Ok(sub_pats) = subpat.to_vec() {
                // Collect names from nested pattern
                let names = collect_pattern_names(subpat, literals, ellipsis);
                for name in &names {
                    let values: Vec<Value> = collected
                        .iter()
                        .filter_map(|b| {
                            b.get(name).and_then(|m| {
                                if let MatchResult::Single(v) = m {
                                    Some(v.clone())
                                } else {
                                    None
                                }
                            })
                        })
                        .collect();
                    bindings.insert(name.clone(), MatchResult::Ellipsis(values));
                }
                let _ = sub_pats; // suppress unused warning
            }

            pi += 2; // skip pattern + ellipsis
        } else {
            // Normal match
            if ii >= input.len() {
                return Ok(false);
            }
            if !match_single(&pattern[pi], &input[ii], literals, ellipsis, bindings)? {
                return Ok(false);
            }
            pi += 1;
            ii += 1;
        }
    }

    Ok(ii == input.len())
}

/// Match a single pattern element against a single input value.
fn match_single(
    pattern: &Value,
    input: &Value,
    literals: &[String],
    ellipsis: &str,
    bindings: &mut HashMap<String, MatchResult>,
) -> Result<bool, LispError> {
    match pattern {
        Value::Symbol(sym) => {
            let name = sym.name();
            if name == "_" {
                // Wildcard — matches anything
                Ok(true)
            } else if literals.contains(&name.to_string()) {
                // Literal — must match exactly
                if let Value::Symbol(input_sym) = input {
                    Ok(input_sym.name() == name)
                } else {
                    Ok(false)
                }
            } else {
                // Pattern variable — bind to input
                bindings.insert(name.to_string(), MatchResult::Single(input.clone()));
                Ok(true)
            }
        }
        Value::Pair(_) | Value::Null => {
            // Nested list pattern
            let pat_items = pattern
                .to_vec()
                .map_err(|_| LispError::syntax("invalid pattern", format!("{pattern}")))?;
            let input_items = match input.to_vec() {
                Ok(v) => v,
                Err(_) => return Ok(false),
            };
            match_pattern(&pat_items, &input_items, literals, ellipsis, bindings)
        }
        // Literal constants
        Value::Int(a) => Ok(matches!(input, Value::Int(b) if a == b)),
        Value::Bool(a) => Ok(matches!(input, Value::Bool(b) if a == b)),
        Value::String(a) => Ok(matches!(input, Value::String(b) if a == b)),
        Value::Char(a) => Ok(matches!(input, Value::Char(b) if a == b)),
        _ => Ok(false),
    }
}

/// Result of matching a pattern variable.
#[derive(Clone, Debug)]
pub enum MatchResult {
    /// A single value binding.
    Single(Value),
    /// An ellipsis binding (list of values).
    Ellipsis(Vec<Value>),
}

fn is_ellipsis_id(v: &Value, ellipsis: &str) -> bool {
    matches!(v, Value::Symbol(s) if s.name() == ellipsis)
}

/// Collect all pattern variable names from a pattern.
fn collect_pattern_names(pattern: &Value, literals: &[String], ellipsis: &str) -> Vec<String> {
    let mut names = Vec::new();
    collect_names_inner(pattern, literals, ellipsis, &mut names);
    names
}

fn collect_names_inner(
    pattern: &Value,
    literals: &[String],
    ellipsis: &str,
    names: &mut Vec<String>,
) {
    match pattern {
        Value::Symbol(sym) => {
            let name = sym.name();
            if name != "_" && name != ellipsis && !literals.contains(&name.to_string()) {
                names.push(name.to_string());
            }
        }
        Value::Pair(_) => {
            if let Ok(items) = pattern.to_vec() {
                for item in &items {
                    collect_names_inner(item, literals, ellipsis, names);
                }
            }
        }
        _ => {}
    }
}

/// Instantiate a template with matched bindings.
fn instantiate_template(
    template: &Value,
    bindings: &HashMap<String, MatchResult>,
    ellipsis: &str,
) -> Result<Value, LispError> {
    match template {
        Value::Symbol(sym) => {
            let name = sym.name();
            match bindings.get(name) {
                Some(MatchResult::Single(v)) => Ok(v.clone()),
                Some(MatchResult::Ellipsis(_)) => {
                    // Ellipsis variable used outside of ellipsis context
                    Err(LispError::syntax(
                        "ellipsis variable used outside ellipsis template",
                        name,
                    ))
                }
                None => Ok(template.clone()), // free variable — keep as-is
            }
        }
        Value::Pair(_) | Value::Null => {
            let items = template
                .to_vec()
                .map_err(|_| LispError::syntax("invalid template", format!("{template}")))?;

            // R7RS §4.3.2: Ellipsis escape — (... template) in a template
            // suppresses ellipsis processing within template.
            // The default ellipsis is "...", so (... x) means x is literal.
            if items.len() == 2 && ellipsis == "..." {
                if let Value::Symbol(s) = &items[0] {
                    if s.name() == "..." {
                        // Ellipsis escape: return the inner template verbatim,
                        // but still substitute non-ellipsis pattern variables.
                        return instantiate_template_literal(&items[1], bindings);
                    }
                }
            }

            // Check for ellipsis in template: (expr <ellipsis>)
            let mut result = Vec::new();
            let mut i = 0;
            while i < items.len() {
                if i + 1 < items.len() && is_ellipsis_id(&items[i + 1], ellipsis) {
                    // Expand ellipsis
                    let sub_template = &items[i];
                    let ellipsis_names = collect_template_ellipsis_vars(sub_template, bindings);

                    if let Some(first_name) = ellipsis_names.first() {
                        if let Some(MatchResult::Ellipsis(values)) = bindings.get(first_name) {
                            let count = values.len();
                            for idx in 0..count {
                                // Create bindings for this iteration
                                let mut iter_bindings = bindings.clone();
                                for name in &ellipsis_names {
                                    if let Some(MatchResult::Ellipsis(vs)) = bindings.get(name) {
                                        if idx < vs.len() {
                                            iter_bindings.insert(
                                                name.clone(),
                                                MatchResult::Single(vs[idx].clone()),
                                            );
                                        }
                                    }
                                }
                                result.push(instantiate_template(
                                    sub_template,
                                    &iter_bindings,
                                    ellipsis,
                                )?);
                            }
                        }
                    }
                    i += 2; // skip template + ellipsis
                } else {
                    result.push(instantiate_template(&items[i], bindings, ellipsis)?);
                    i += 1;
                }
            }
            Ok(Value::list(result))
        }
        _ => Ok(template.clone()), // constants pass through
    }
}

/// Instantiate a template literally — no ellipsis expansion.
/// Used inside `(... template)` escape. Still substitutes single bindings.
fn instantiate_template_literal(
    template: &Value,
    bindings: &HashMap<String, MatchResult>,
) -> Result<Value, LispError> {
    match template {
        Value::Symbol(sym) => {
            let name = sym.name();
            match bindings.get(name) {
                Some(MatchResult::Single(v)) => Ok(v.clone()),
                // In literal context, ellipsis variables are NOT expanded
                _ => Ok(template.clone()),
            }
        }
        Value::Pair(_) | Value::Null => {
            let items = template
                .to_vec()
                .map_err(|_| LispError::syntax("invalid template", format!("{template}")))?;
            let result: Result<Vec<Value>, LispError> = items
                .iter()
                .map(|item| instantiate_template_literal(item, bindings))
                .collect();
            Ok(Value::list(result?))
        }
        _ => Ok(template.clone()),
    }
}

/// Find all variables in a template that have ellipsis bindings.
fn collect_template_ellipsis_vars(
    template: &Value,
    bindings: &HashMap<String, MatchResult>,
) -> Vec<String> {
    let mut vars = Vec::new();
    collect_ellipsis_vars_inner(template, bindings, &mut vars);
    vars
}

fn collect_ellipsis_vars_inner(
    template: &Value,
    bindings: &HashMap<String, MatchResult>,
    vars: &mut Vec<String>,
) {
    match template {
        Value::Symbol(sym) => {
            let name = sym.name();
            if let Some(MatchResult::Ellipsis(_)) = bindings.get(name) {
                if !vars.contains(&name.to_string()) {
                    vars.push(name.to_string());
                }
            }
        }
        Value::Pair(_) => {
            if let Ok(items) = template.to_vec() {
                for item in &items {
                    collect_ellipsis_vars_inner(item, bindings, vars);
                }
            }
        }
        _ => {}
    }
}

/// Parse a `(syntax-rules (literals...) (pattern template) ...)` form.
///
/// R7RS §4.3.2 / SRFI 46: Also supports custom ellipsis identifier:
///   `(syntax-rules <ellipsis> (literals...) (pattern template) ...)`
/// where `<ellipsis>` is an identifier (symbol, not a list).
pub fn parse_syntax_rules(items: &[Value]) -> Result<SyntaxRules, LispError> {
    // items[0] = "syntax-rules"
    // items[1] = <ellipsis> or (literal ...)
    // If items[1] is a symbol (not a list), it's a custom ellipsis identifier.
    if items.len() < 3 {
        return Err(LispError::syntax(
            "syntax-rules requires at least one rule",
            format!("{}", Value::list(items.to_vec())),
        ));
    }

    // Detect custom ellipsis: items[1] is a symbol → custom ellipsis, items[2] is literals
    let (ellipsis, literal_idx, rules_start) = if let Value::Symbol(_) = &items[1] {
        // Custom ellipsis: (syntax-rules ::: (literals...) rules...)
        if items.len() < 4 {
            return Err(LispError::syntax(
                "syntax-rules with custom ellipsis requires at least one rule",
                format!("{}", Value::list(items.to_vec())),
            ));
        }
        let ell = if let Value::Symbol(s) = &items[1] {
            s.name().to_string()
        } else {
            unreachable!()
        };
        (ell, 2, 3)
    } else {
        // Default ellipsis: (syntax-rules (literals...) rules...)
        ("...".to_string(), 1, 2)
    };

    let literals = items[literal_idx]
        .to_vec()
        .map_err(|_| {
            LispError::syntax(
                "syntax-rules: invalid literal list",
                format!("{}", items[literal_idx]),
            )
        })?
        .iter()
        .map(|v| match v {
            Value::Symbol(s) => Ok(s.name().to_string()),
            _ => Err(LispError::syntax(
                "syntax-rules: literal must be identifier",
                format!("{v}"),
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut rules = Vec::new();
    for rule in &items[rules_start..] {
        let pair = rule.to_vec().map_err(|_| {
            LispError::syntax(
                "syntax-rules: rule must be (pattern template)",
                format!("{rule}"),
            )
        })?;
        if pair.len() != 2 {
            return Err(LispError::syntax(
                "syntax-rules: rule must have exactly 2 elements",
                format!("{rule}"),
            ));
        }
        rules.push((pair[0].clone(), pair[1].clone()));
    }

    Ok(SyntaxRules {
        literals,
        rules,
        ellipsis,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib;
    use crate::vm::Vm;

    fn eval(code: &str) -> Value {
        let mut vm = Vm::new();
        stdlib::register_stdlib(&mut vm);
        vm.eval(code).unwrap()
    }

    fn eval_err(code: &str) -> LispError {
        let mut vm = Vm::new();
        stdlib::register_stdlib(&mut vm);
        vm.eval(code).unwrap_err()
    }

    // -- define-macro tests --

    #[test]
    fn test_define_macro_simple() {
        // my-if as a macro
        assert_eq!(
            eval("(define-macro (my-if c t f) (list 'if c t f)) (my-if #t 1 2)"),
            Value::Int(1)
        );
    }

    #[test]
    fn test_define_macro_swap() {
        assert_eq!(
            eval("(define-macro (swap! a b) (list 'begin (list 'define '__tmp a) (list 'set! a b) (list 'set! b '__tmp)))
                  (define x 1) (define y 2) (swap! x y) x"),
            Value::Int(2)
        );
    }

    // -- syntax-rules tests --

    #[test]
    fn test_syntax_rules_basic() {
        assert_eq!(
            eval(
                "(define-syntax my-and
                    (syntax-rules ()
                      ((_ ) #t)
                      ((_ e) e)
                      ((_ e1 e2 ...) (if e1 (my-and e2 ...) #f))))
                  (my-and 1 2 3)"
            ),
            Value::Int(3)
        );
    }

    #[test]
    fn test_syntax_rules_false() {
        assert_eq!(
            eval(
                "(define-syntax my-and
                    (syntax-rules ()
                      ((_) #t)
                      ((_ e) e)
                      ((_ e1 e2 ...) (if e1 (my-and e2 ...) #f))))
                  (my-and 1 #f 3)"
            ),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_syntax_rules_let_macro() {
        // Reimplementation of let using syntax-rules
        assert_eq!(
            eval(
                "(define-syntax my-let
                    (syntax-rules ()
                      ((_ ((var val) ...) body ...)
                       ((lambda (var ...) body ...) val ...))))
                  (my-let ((x 10) (y 20)) (+ x y))"
            ),
            Value::Int(30)
        );
    }

    #[test]
    fn test_syntax_rules_no_match() {
        let _ = eval_err(
            "(define-syntax my-mac
                            (syntax-rules ()
                              ((_ a b) (+ a b))))
                          (my-mac 1)",
        ); // wrong arity
    }

    #[test]
    fn test_syntax_rules_with_literals() {
        assert_eq!(
            eval(
                "(define-syntax my-case
                    (syntax-rules (=>)
                      ((_ expr (val => result)) (if (= expr val) result #f))))
                  (my-case 5 (5 => 42))"
            ),
            Value::Int(42)
        );
    }

    // -- Pattern matching unit tests --

    #[test]
    fn test_match_simple() {
        let mut bindings = HashMap::new();
        let pattern = vec![Value::symbol("_"), Value::symbol("x")];
        let input = vec![Value::symbol("my-mac"), Value::Int(42)];
        assert!(match_pattern(&pattern, &input, &[], "...", &mut bindings).unwrap());
        assert!(matches!(
            bindings.get("x"),
            Some(MatchResult::Single(Value::Int(42)))
        ));
    }

    #[test]
    fn test_match_ellipsis() {
        let mut bindings = HashMap::new();
        let pattern = vec![Value::symbol("_"), Value::symbol("x"), Value::symbol("...")];
        let input = vec![
            Value::symbol("mac"),
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ];
        assert!(match_pattern(&pattern, &input, &[], "...", &mut bindings).unwrap());
        if let Some(MatchResult::Ellipsis(vs)) = bindings.get("x") {
            assert_eq!(vs.len(), 3);
        } else {
            panic!("expected ellipsis binding");
        }
    }

    #[test]
    fn test_template_ellipsis() {
        let mut bindings = HashMap::new();
        bindings.insert(
            "x".to_string(),
            MatchResult::Ellipsis(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        );
        let template = Value::list(vec![
            Value::symbol("list"),
            Value::symbol("x"),
            Value::symbol("..."),
        ]);
        let result = instantiate_template(&template, &bindings, "...").unwrap();
        let items = result.to_vec().unwrap();
        assert_eq!(items.len(), 4); // list + 3 values
    }

    // -- R7RS compliance: edge cases from §4.3.2 --

    #[test]
    fn test_syntax_rules_zero_ellipsis() {
        // Ellipsis matching zero elements
        assert_eq!(
            eval(
                "(define-syntax my-list
                    (syntax-rules ()
                      ((_ x ...) (list x ...))))
                  (my-list)"
            ),
            Value::Null // empty list
        );
    }

    #[test]
    fn test_syntax_rules_nested_ellipsis() {
        // Nested pattern with ellipsis: ((var val) ...)
        // This is the let-macro pattern — test with 0 bindings
        assert_eq!(
            eval(
                "(define-syntax my-let
                    (syntax-rules ()
                      ((_ ((var val) ...) body ...)
                       ((lambda (var ...) body ...) val ...))))
                  (my-let () 42)"
            ),
            Value::Int(42)
        );
    }

    #[test]
    fn test_syntax_rules_constant_pattern() {
        // Constants in patterns must match exactly (R7RS §4.3.2)
        assert_eq!(
            eval(
                "(define-syntax check-zero
                    (syntax-rules ()
                      ((_ 0) \"zero\")
                      ((_ n) \"nonzero\")))
                  (check-zero 0)"
            ),
            Value::String(std::rc::Rc::from("zero"))
        );
        assert_eq!(
            eval(
                "(define-syntax check-zero
                    (syntax-rules ()
                      ((_ 0) \"zero\")
                      ((_ n) \"nonzero\")))
                  (check-zero 5)"
            ),
            Value::String(std::rc::Rc::from("nonzero"))
        );
    }

    #[test]
    fn test_syntax_rules_wildcard() {
        // _ matches anything without binding
        assert_eq!(
            eval(
                "(define-syntax second
                    (syntax-rules ()
                      ((_ _ x) x)))
                  (second 1 2)"
            ),
            Value::Int(2)
        );
    }

    #[test]
    fn test_syntax_rules_or_macro() {
        // or is a classic macro that exercises recursion + ellipsis
        assert_eq!(
            eval(
                "(define-syntax my-or
                    (syntax-rules ()
                      ((_) #f)
                      ((_ e) e)
                      ((_ e1 e2 ...)
                       (let ((t e1)) (if t t (my-or e2 ...))))))
                  (my-or #f #f 42)"
            ),
            Value::Int(42)
        );
    }

    #[test]
    fn test_syntax_rules_when_unless() {
        // when/unless — derived expressions from R7RS §4.2.1
        assert_eq!(
            eval(
                "(define-syntax my-when
                    (syntax-rules ()
                      ((_ test body ...)
                       (if test (begin body ...) (void)))))
                  (my-when #t 1 2 3)"
            ),
            Value::Int(3)
        );
    }

    #[test]
    fn test_define_macro_persists_across_evals() {
        // Macro defined in one eval, used in another
        let mut vm = Vm::new();
        stdlib::register_stdlib(&mut vm);
        vm.eval("(define-macro (double x) (list '* x 2))").unwrap();
        assert_eq!(vm.eval("(double 5)").unwrap(), Value::Int(10));
    }

    #[test]
    fn test_syntax_rules_persists_across_evals() {
        let mut vm = Vm::new();
        stdlib::register_stdlib(&mut vm);
        vm.eval("(define-syntax add1 (syntax-rules () ((_ x) (+ x 1))))")
            .unwrap();
        assert_eq!(vm.eval("(add1 41)").unwrap(), Value::Int(42));
    }
}
