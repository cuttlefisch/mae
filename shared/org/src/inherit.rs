//! Property inheritance as `org-entry-get-with-inheritance` reads it
//! (org.el), from in-file declarations only (ADR-112 D4): the nearest
//! headline, then the file's property drawer, holding a plain `:KEY:` ends
//! the search; the `:KEY+:` values met on the way append to it; with no
//! plain value anywhere, `#+PROPERTY: KEY …` (itself `+`-appended) starts
//! the list. A value that is exactly `nil` is none, and stops the search
//! as Org's does (it is a value found).
//!
//! One reading, shared: babel's `header-args` ([`crate::render`]) and the
//! attachment directory of an `attachment:` link (the semantics) both ask
//! it, so they cannot disagree about which headline a property comes from.

use crate::kind::SyntaxKind::HEADLINE;
use crate::kind::SyntaxNode;
use crate::seeded::HashMap;
use crate::span::Span;
use crate::views::{canonical_key, Headline, PropertyDrawer};
use crate::Document;
use std::cell::RefCell;

/// The inherited properties of one document, memoised per headline, so
/// every lookup of one property is linear in the document overall.
#[derive(Debug)]
pub(crate) struct Inherited {
    root: SyntaxNode,
    /// `#+PROPERTY:` values, canonical key → value, `+` keys merged.
    global: HashMap<String, String>,
    file_drawer: Option<PropertyDrawer>,
    /// (headline start, canonical property) → its inherited value.
    memo: RefCell<HashMap<(u32, String), Option<String>>>,
}

impl Inherited {
    pub fn new(doc: &Document) -> Inherited {
        Inherited {
            root: doc.syntax(),
            global: global_properties(doc),
            file_drawer: doc.property_drawer(),
            memo: RefCell::new(HashMap::default()),
        }
    }

    /// The headline whose entry holds byte `at` (`None`: before the first
    /// headline, or past the end).
    pub fn headline_at(&self, at: u32) -> Option<SyntaxNode> {
        headline_at(&self.root, at)
    }

    /// `prop` (canonical) as `org-entry-get` with inheritance reads it at a
    /// point under `headline` (`None`: the zeroth section). `nil` is none.
    pub fn get(&self, prop: &str, headline: Option<&SyntaxNode>) -> Option<String> {
        self.get_literal(prop, headline).filter(|v| v != "nil")
    }

    /// [`Inherited::get`], but a value of `nil` is returned as written
    /// (`org-entry-get-with-inheritance`'s LITERAL-NIL).
    pub fn get_literal(&self, prop: &str, headline: Option<&SyntaxNode>) -> Option<String> {
        // The headlines up to the first one whose value is known (memoised,
        // or a plain value of its own), innermost first.
        let mut path: Vec<(SyntaxNode, Vec<String>)> = Vec::new();
        let mut cur = headline.cloned();
        let mut known = loop {
            let Some(h) = cur else {
                break self.file_level(prop);
            };
            let at = (Span::of(&h).start, prop.to_string());
            if let Some(v) = self.memo.borrow().get(&at) {
                break v.clone();
            }
            let drawer = Headline::cast(h.clone()).and_then(|h| h.property_drawer());
            let (base, plus) = local_values(drawer, prop);
            if let Some(b) = base {
                let v = join(Some(b), &plus);
                self.memo.borrow_mut().insert(at, v.clone());
                break v;
            }
            cur = h.ancestors().skip(1).find(|a| a.kind() == HEADLINE);
            path.push((h, plus));
        };
        for (h, plus) in path.into_iter().rev() {
            known = join(known, &plus);
            let at = (Span::of(&h).start, prop.to_string());
            self.memo.borrow_mut().insert(at, known.clone());
        }
        known
    }

    /// The file level: its property drawer, else `#+PROPERTY:`.
    fn file_level(&self, prop: &str) -> Option<String> {
        let (base, plus) = local_values(self.file_drawer.clone(), prop);
        match base {
            Some(b) => join(Some(b), &plus),
            None => join(self.global.get(prop).cloned(), &plus),
        }
    }
}

/// The headline whose entry holds byte `at` of the document `root`
/// (`None`: before the first headline, or past the end).
pub(crate) fn headline_at(root: &SyntaxNode, at: u32) -> Option<SyntaxNode> {
    if at >= u32::from(root.text_range().end()) {
        return None;
    }
    // The token STARTING at `at` (right-biased): at a headline's first
    // byte, the one before it ends the previous headline's section.
    let token = root.token_at_offset(at.into()).right_biased()?;
    token.parent()?.ancestors().find(|a| a.kind() == HEADLINE)
}

/// `base` then `plus`, joined by spaces (`org-property-separators`'s
/// default); `None` if there is nothing.
fn join(base: Option<String>, plus: &[String]) -> Option<String> {
    let mut parts: Vec<String> = base.into_iter().collect();
    parts.extend(plus.iter().cloned());
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// A drawer's plain value of `prop` (the last one wins) and its `prop+`
/// values in order (`org-element--get-node-properties`).
fn local_values(drawer: Option<PropertyDrawer>, prop: &str) -> (Option<String>, Vec<String>) {
    let Some(d) = drawer else {
        return (None, Vec::new());
    };
    let plus_key = format!("{prop}+");
    let mut base = None;
    let mut plus = Vec::new();
    for p in d.properties() {
        if p.key == prop {
            base = Some(p.value);
        } else if p.key == plus_key {
            plus.push(p.value);
        }
    }
    (base, plus)
}

/// `#+PROPERTY: NAME VALUE` lines in order, a `NAME+` appending to `NAME`
/// (`org--update-property-plist`).
pub(crate) fn global_properties(doc: &Document) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::default();
    for k in doc.keywords() {
        if !k.key().eq_ignore_ascii_case("PROPERTY") {
            continue;
        }
        let value = k.value();
        let Some((name, rest)) = value.split_once([' ', '\t']) else {
            continue;
        };
        let rest = rest.trim_start_matches([' ', '\t']);
        let (name, append) = match name.strip_suffix('+') {
            Some(n) if !n.is_empty() => (n, true),
            _ => (name, false),
        };
        let name = canonical_key(name);
        match out.get_mut(&name) {
            Some(old) if append => {
                old.push(' ');
                old.push_str(rest);
            }
            _ => {
                out.insert(name, rest.to_string());
            }
        }
    }
    out
}
