//! Org's macros, read and never expanded (ADR-112 D1, D4): whether a macro
//! must render nothing because, expanded, it would put text the export
//! hides in its place. MAE draws a macro as its source (registered:
//! macros-citations), so its ARGUMENTS appear in the output, and they must
//! not when Org's expansion would hide them.
//!
//! A macro renders nothing (fails closed, hiding more than Org, never
//! less) when:
//! - its arguments, read as objects, hold an inline src block, babel call,
//!   export snippet or results macro, or a macro that renders nothing;
//! - its template, EXPANDED with its arguments as Org expands it, read as
//!   objects, holds one (arguments split across `$1$2` can form one only
//!   once substituted), or calls a macro that hides (through every macro it
//!   calls, at most [`MAX_DEPTH`] deep and [`MAX_EXPANSIONS`] calls; a cycle
//!   hides);
//! - its arguments or template hold the START of such a construct that
//!   does not parse as one (`src_`, `call_`, `@@`, `{{{`): once Org
//!   expands the macro, the text after it can complete it (a template
//!   `src_sh[:exports none]{echo $1` before a `}` hides the argument in
//!   Org);
//! - its template cannot be known from the text: an `(eval …)` template,
//!   or a name neither defined in the file nor one of Org's built-ins
//!   (Org's export fails on an undefined macro; `#+SETUPFILE` and
//!   `org-export-global-macros` are not read).
//!
//! Templates as `org-macro-initialize-templates` (Org 9.7) builds them:
//! names compared case-insensitively; of two `#+MACRO:` lines with one
//! name the FIRST in the file wins (`org-macro--set-templates` lets a later
//! list entry override, and the collected list is newest first); `author`,
//! `email`, `title` and `date` are the values of their keywords when the
//! file has them, overriding a `#+MACRO:` of that name; `keyword`, `n`,
//! `property`, `time`, `input-file` and `modification-time` are functions
//! (a `#+MACRO:` of the name comes first); `results` is expanded later, as
//! its argument. Checked against Emacs 30.2 (tests/render_hidden_inline.rs).

use super::objects::holds_hidden;
use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::Document;
use std::collections::HashMap;

/// How deep a template's macros are followed before the macro hides.
pub(crate) const MAX_DEPTH: usize = 16;

/// How many macro calls one check expands at most, and how long one
/// expansion may grow, before the macro hides: a template calling itself
/// twice must not make the check exponential.
const MAX_EXPANSIONS: usize = 256;
const MAX_EXPANSION_BYTES: usize = 64 * 1024;

/// Org's built-in macros whose template is a function of their arguments,
/// not a text: they put no argument in a hidden construct.
const FUNCTIONS: &[&str] = &[
    "keyword",
    "n",
    "property",
    "time",
    "input-file",
    "modification-time",
    "results",
];

/// The built-ins whose template is a keyword's value.
const FROM_KEYWORDS: &[&str] = &["author", "email", "title", "date"];

#[derive(Debug, Default)]
pub(crate) struct Macros {
    /// Folded name → template text.
    templates: HashMap<String, String>,
}

fn fold(name: &str) -> String {
    crate::chars::fold(name)
}

fn token(n: &SyntaxNode, kind: crate::SyntaxKind) -> String {
    n.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == kind)
        .map(|t| t.text().to_string())
        .collect()
}

impl Macros {
    pub fn new(doc: &Document) -> Macros {
        let mut templates: HashMap<String, String> = HashMap::new();
        let keywords: Vec<_> = doc.keywords().collect();
        for k in keywords
            .iter()
            .filter(|k| k.key().eq_ignore_ascii_case("MACRO"))
        {
            let value = k.value();
            // `^\(\S-+\)[ \t]*`: the name, then the definition.
            let v = value.trim_start_matches(crate::chars::is_space);
            let end = v.find(crate::chars::is_space).unwrap_or(v.len());
            if end == 0 {
                continue;
            }
            let definition = v[end..].trim_start_matches([' ', '\t']);
            templates
                .entry(fold(&v[..end]))
                .or_insert_with(|| definition.to_string());
        }
        for name in FROM_KEYWORDS {
            let values: Vec<String> = keywords
                .iter()
                .filter(|k| k.key().eq_ignore_ascii_case(name))
                .map(|k| k.value())
                .collect();
            if !values.is_empty() {
                templates.insert(name.to_string(), values.join(" "));
            } else {
                templates.entry(name.to_string()).or_default();
            }
        }
        Macros { templates }
    }

    /// Whether macro object `n` renders nothing (see the module doc).
    pub fn hides(&self, n: &SyntaxNode) -> bool {
        let mut budget = MAX_EXPANSIONS;
        let name = fold(&token(n, NAME));
        self.call_hides(&name, &token(n, VALUE), 0, &mut Vec::new(), &mut budget)
    }

    /// Whether the call of macro `name` with raw arguments `args` hides:
    /// its arguments do, or its template, EXPANDED with those arguments as
    /// Org expands it (`org-macro-expand`), does: arguments split across
    /// `$1$2` may only form a hidden construct once substituted.
    fn call_hides(
        &self,
        name: &str,
        args: &str,
        depth: usize,
        stack: &mut Vec<String>,
        budget: &mut usize,
    ) -> bool {
        if depth > MAX_DEPTH || *budget == 0 {
            return true;
        }
        *budget -= 1;
        if !args.is_empty() {
            let tree = crate::green::objects(args, PARAGRAPH);
            if self.tree_hides(&tree, depth, stack, budget) {
                return true;
            }
        }
        let Some(template) = self.templates.get(name) else {
            return !FUNCTIONS.contains(&name);
        };
        if is_eval(template) || stack.iter().any(|s| s == name) {
            return true;
        }
        let expanded = expand(template, &extract_arguments(args));
        if expanded.len() > MAX_EXPANSION_BYTES {
            return true;
        }
        stack.push(name.to_string());
        let tree = crate::green::objects(&expanded, PARAGRAPH);
        let hides = self.tree_hides(&tree, depth, stack, budget);
        stack.pop();
        hides
    }

    /// Whether `tree` (arguments, or an expansion, read as objects) holds a
    /// hidden construct, the start of one, or a macro call that hides.
    fn tree_hides(
        &self,
        tree: &SyntaxNode,
        depth: usize,
        stack: &mut Vec<String>,
        budget: &mut usize,
    ) -> bool {
        holds_starts(&tree.to_string(), tree)
            || holds_hidden(tree)
            || tree.descendants().filter(|d| d.kind() == MACRO).any(|m| {
                let name = fold(&token(&m, NAME));
                self.call_hides(&name, &token(&m, VALUE), depth + 1, stack, budget)
            })
    }
}

/// `org-element-macro-parser`'s arguments: trimmed, every blank run one
/// space, then `org-macro-extract-arguments`: split at each comma not
/// escaped, a run of N backslashes before a comma halved (an odd run
/// escapes it).
fn extract_arguments(raw: &str) -> Vec<String> {
    let collapsed = raw
        .split([' ', '\t', '\r', '\n'])
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut args = vec![String::new()];
    let mut slashes = 0usize;
    for c in collapsed.chars() {
        match c {
            '\\' => slashes += 1,
            ',' => {
                let cur = args.last_mut().expect("one argument at least");
                cur.extend(std::iter::repeat_n('\\', slashes / 2));
                if slashes % 2 == 1 {
                    cur.push(',');
                } else {
                    args.push(String::new());
                }
                slashes = 0;
            }
            c => {
                let cur = args.last_mut().expect("one argument at least");
                cur.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                cur.push(c);
            }
        }
    }
    let cur = args.last_mut().expect("one argument at least");
    cur.extend(std::iter::repeat_n('\\', slashes));
    args
}

/// `org-macro-expand` on a text template: each `$N` the N-th argument, or
/// nothing.
fn expand(template: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek().is_some_and(char::is_ascii_digit) {
            let mut n = String::new();
            while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                n.push(d);
                chars.next();
            }
            let i = n.parse::<usize>().unwrap_or(0);
            if let Some(a) = i.checked_sub(1).and_then(|i| args.get(i)) {
                out.push_str(a);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether `text` (parsed as `tree`) holds the start of an inline src
/// block, babel call or export snippet, parsed or not (a parsed one hides
/// anyway; an unparsed one may be completed by the text after the macro
/// once Org expands it; a plain word holding `src_` hides too: more than
/// Org, never less), or a `{{{` that is no macro of `tree` (a parsed macro
/// is judged as a macro).
fn holds_starts(text: &str, tree: &SyntaxNode) -> bool {
    let macros = tree.descendants().filter(|d| d.kind() == MACRO).count();
    ["src_", "call_", "@@"].iter().any(|s| text.contains(s)) || text.matches("{{{").count() > macros
}

/// `(string-match-p "\\`(eval\\>" value)`: a template Org evaluates.
fn is_eval(t: &str) -> bool {
    t.strip_prefix("(eval")
        .is_some_and(|rest| !rest.starts_with(|c: char| crate::chars::is_word(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `org-macro-extract-arguments` after the parser's trim and collapse,
    /// and `org-macro-expand`, on Emacs 30.2's answers.
    #[test]
    fn arguments_split_and_expand_as_org_does() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(extract_arguments("a,b"), v(&["a", "b"]));
        assert_eq!(extract_arguments("a\\,b,c"), v(&["a,b", "c"]));
        assert_eq!(extract_arguments("a\\\\,b"), v(&["a\\", "b"]));
        assert_eq!(extract_arguments("  x \n  y ,z "), v(&["x y ", "z"]));
        assert_eq!(extract_arguments(""), v(&[""]));
        assert_eq!(extract_arguments(",,"), v(&["", "", ""]));
        let args = v(&["src", "_sh{x}"]);
        assert_eq!(expand("$1$2 $3.", &args), "src_sh{x} .");
        assert_eq!(expand("$10", &args), "");
        assert_eq!(expand("$ and $x", &args), "$ and $x");
    }
}
