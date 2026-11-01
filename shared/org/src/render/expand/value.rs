//! Templates and the values of macros: `org-macro-initialize-templates`'
//! templates, `org-macro-expand` on them, and the built-ins.

use super::scan::Engine;
use super::syntax::{extract_arguments, fold, is_eval, substitute};
use super::{MacroFailure, Stop, SPECIAL_PROPERTIES};
use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::views::Headline;
use crate::{Document, LineEnding};
use rowan::WalkEvent;
use std::collections::HashMap;

/// A template, as `org-macro-initialize-templates` installs it.
#[derive(Debug, Clone)]
pub(super) enum Template {
    Text(String),
    /// `(eval …)`: Org evaluates it.
    Eval,
    /// `date` when `#+DATE:` is one timestamp: `{{{date(FORMAT)}}}`
    /// formats it (unsupported), `{{{date}}}` is the value.
    Date(String),
    Keyword,
    N,
    Property,
    Time,
    InputFile,
    ModificationTime,
}

impl Engine {
    /// The value of the macro `name` with raw arguments `args` (its source
    /// `raw`, standing at `at` of the buffer; `place` is where its entry is
    /// in the current parse, for `property`).
    pub(super) fn call(
        &mut self,
        (name, args, raw): (&str, Option<&str>, &str),
        place: usize,
        at: usize,
    ) -> Result<String, Stop> {
        let key = fold(name);
        let Some(template) = self.templates.get(&key).cloned() else {
            return Err((MacroFailure::Undefined, 0, 0));
        };
        // `org-macro-replace-all`: the same macro, same arguments, at the
        // same place again is a cycle.
        if !self.record.insert((at, raw.to_string())) {
            return Err((MacroFailure::Circular, 0, 0));
        }
        self.charge(raw.len())?;
        let value = self.value(&template, extract_arguments(args), place)?;
        self.charge(value.len())?;
        Ok(value)
    }

    /// A template's value for `args` (`None`: no parentheses).
    fn value(
        &mut self,
        template: &Template,
        args: Option<Vec<String>>,
        place: usize,
    ) -> Result<String, Stop> {
        let unsupported = Err((MacroFailure::Unsupported, 0, 0));
        let all = args.unwrap_or_default();
        let arg = |i: usize| all.get(i).map(String::as_str);
        let nonblank =
            |s: Option<&str>| s.is_some_and(|s| !s.trim_matches(crate::text::is_trim).is_empty());
        Ok(match template {
            Template::Text(t) => substitute(t, &all),
            Template::Eval => return Err((MacroFailure::Eval, 0, 0)),
            Template::Date(_) if nonblank(arg(0)) => return unsupported,
            Template::Date(v) => v.clone(),
            Template::Keyword => match arg(0) {
                Some(name) if !name.contains(crate::chars::is_space) => {
                    self.keyword(name)?.unwrap_or_default()
                }
                _ => return unsupported,
            },
            Template::N => self.counter(arg(0), arg(1))?,
            Template::Property => match (arg(0), nonblank(arg(1))) {
                (Some(p), false) => self.property(p, place)?,
                _ => return unsupported,
            },
            Template::Time | Template::ModificationTime => {
                let Some(f) = arg(0).and_then(epoch_format) else {
                    return unsupported;
                };
                self.environment = true;
                f
            }
            Template::InputFile => {
                self.environment = true;
                String::new()
            }
        })
    }

    /// `org-macro--find-keyword-value NAME t` on the buffer as last read
    /// whole (`#+…` lines outside COMMENT subtrees).
    fn keyword(&mut self, name: &str) -> Result<Option<String>, Stop> {
        let key = fold(name);
        if let Some(v) = self.keywords.get(&key) {
            return Ok(v.clone());
        }
        if self.lines.is_none() && self.outer_lines.is_none() {
            let lines = keyword_lines(&self.d.syntax(), &self.dtext, self.eol);
            self.lines = Some(lines.into_iter().map(str::to_string).collect());
        }
        let lines = match &self.outer_lines {
            Some(outer) => outer.as_slice(),
            None => self.lines.as_deref().unwrap_or_default(),
        };
        let n = lines.len();
        let v =
            crate::semantics::keywords::keyword_value(lines.iter().map(String::as_str), name, true);
        self.charge(n)?;
        self.keywords.insert(key, v.clone());
        Ok(v)
    }

    /// `org-macro--counter-increment NAME ACTION`.
    fn counter(&mut self, name: Option<&str>, action: Option<&str>) -> Result<String, Stop> {
        let trim = |s: &str| s.trim_matches(crate::text::is_trim).to_string();
        let name = name.map(trim).unwrap_or_default();
        let action = action.map(trim).filter(|a| !a.is_empty());
        let cur = self.counters.get(&name).copied();
        let v = match action.as_deref() {
            None => cur.unwrap_or(0).checked_add(1),
            Some("-") => Some(cur.unwrap_or(1)),
            Some(a) if a.bytes().all(|b| b.is_ascii_digit()) => a.parse::<u64>().ok(),
            Some(_) => Some(1),
        };
        let Some(v) = v else {
            return Err((MacroFailure::Unsupported, 0, 0));
        };
        self.counters.insert(name, v);
        Ok(v.to_string())
    }

    /// `org-entry-get nil PROPERTY 'selective` at `place`: the property of
    /// the entry (headline, or the file before the first) holding it, not
    /// inherited (`org-use-property-inheritance` is nil); `KEY+` lines
    /// appended, `nil` none. A special property fails.
    fn property(&mut self, property: &str, place: usize) -> Result<String, Stop> {
        let key = crate::views::canonical_key(property);
        if SPECIAL_PROPERTIES.contains(&key.as_str()) {
            return Err((MacroFailure::Unsupported, 0, 0));
        }
        let root = self.d.syntax();
        let entry = entry_at(&root, place);
        if matches!(key.as_str(), "ITEM" | "TODO") {
            // `org-get-heading t t t t`, the TODO keyword; at the top of
            // the file Org reads neither.
            let h = entry.and_then(Headline::cast);
            let Some(h) = h else {
                return Err((MacroFailure::Unsupported, 0, 0));
            };
            return Ok(match key.as_str() {
                "ITEM" => h.title(),
                _ => h.todo().unwrap_or_default(),
            });
        }
        let start = entry
            .as_ref()
            .map_or(0, |e| usize::from(e.text_range().start()) + 1);
        if !self.properties.contains_key(&start) {
            let pairs = entry_properties(&root, entry.as_ref());
            self.charge(pairs.iter().map(|(k, v)| k.len() + v.len()).sum())?;
            let resolved = crate::semantics::resolve_properties(pairs);
            self.properties.insert(start, resolved);
        }
        let props = &self.properties[&start];
        Ok(props
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default())
    }
}

/// The headline whose entry holds `place` (`None`: before the first).
fn entry_at(root: &SyntaxNode, place: usize) -> Option<SyntaxNode> {
    let offset = rowan::TextSize::try_from(place).ok()?;
    if offset >= root.text_range().end() {
        return None;
    }
    let token = root.token_at_offset(offset).right_biased()?;
    token.parent_ancestors().find(|a| a.kind() == HEADLINE)
}

/// The property lines of an entry's drawer, values trimmed.
fn entry_properties(root: &SyntaxNode, entry: Option<&SyntaxNode>) -> Vec<(String, String)> {
    let drawer = match entry {
        Some(h) => Headline::cast(h.clone()).and_then(|h| h.property_drawer()),
        None => root
            .children()
            .next()
            .filter(|s| s.kind() == SECTION)
            .and_then(|s| s.children().find_map(crate::PropertyDrawer::cast)),
    };
    drawer
        .map(|d| d.properties())
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let v = crate::link_grammar::org_trim(&p.value).to_string();
            (p.key, v)
        })
        .collect()
}

/// The lines of `text` that start (after blanks) with `#+`, outside every
/// COMMENT subtree of its parse `root`.
pub(super) fn keyword_lines<'t>(root: &SyntaxNode, text: &'t str, eol: LineEnding) -> Vec<&'t str> {
    let mut skip = Vec::new();
    let mut walk = root.preorder();
    while let Some(ev) = walk.next() {
        let WalkEvent::Enter(n) = ev else { continue };
        if Headline::cast(n.clone()).is_some_and(|h| h.is_commented()) {
            let r = n.text_range();
            skip.push((usize::from(r.start()), usize::from(r.end())));
            walk.skip_subtree();
        }
    }
    let mut out = Vec::new();
    let mut at = 0;
    let mut k = 0;
    for line in crate::render::build::lines(text, eol) {
        while k < skip.len() && skip[k].1 <= at {
            k += 1;
        }
        let skipped = skip.get(k).is_some_and(|&(a, b)| at >= a && at < b);
        let l = crate::render::build::trim_eol(line, eol);
        if !skipped && l.trim_start_matches([' ', '\t']).starts_with("#+") {
            out.push(l);
        }
        at += line.len();
    }
    out
}

/// The templates `org-macro-initialize-templates` installs for `doc`
/// (its text `source`), as the map a lookup reads: folded name → the
/// template the first entry of that name (any case) gives.
pub(super) fn templates(doc: &Document, source: &str) -> HashMap<String, Template> {
    let root = doc.syntax();
    let lines = keyword_lines(&root, source, doc.line_ending());
    let kw = |name: &str, collect| {
        crate::semantics::keywords::keyword_value(lines.iter().copied(), name, collect)
    };
    let text = |v: String| {
        if is_eval(&v) {
            Template::Eval
        } else {
            Template::Text(v)
        }
    };
    let date = kw("DATE", false).map(|v| {
        let tree = crate::green::objects(&v, PARAGRAPH);
        let mut parts = tree.children_with_tokens();
        let one = parts.next().and_then(|p| p.into_node());
        let stamp = one.is_some_and(|n| n.kind() == TIMESTAMP) && parts.next().is_none();
        if stamp {
            Template::Date(v)
        } else {
            text(v)
        }
    });
    // `org-macro--collect-macros`: the keyword built-ins, then each
    // `#+MACRO:` pushed in front, so the list runs from the LAST line.
    let mut list: Vec<(String, Option<Template>)> = Vec::new();
    for k in crate::semantics::keywords::outside_comment_trees(&root) {
        if !k.key().eq_ignore_ascii_case("MACRO") {
            continue;
        }
        let value = k.value();
        let v = value.trim_start_matches(crate::chars::is_space);
        let end = v.find(crate::chars::is_space).unwrap_or(v.len());
        if end == 0 {
            continue;
        }
        let def = v[end..].trim_start_matches([' ', '\t']);
        list.push((v[..end].to_string(), Some(text(def.to_string()))));
    }
    list.reverse();
    list.push(("author".into(), kw("AUTHOR", true).map(text)));
    list.push(("email".into(), kw("EMAIL", false).map(text)));
    list.push(("title".into(), kw("TITLE", true).map(text)));
    list.push(("date".into(), date));
    // `org-macro--set-templates`: a later entry of the SAME name (case
    // sensitive) overrides an earlier one when it has a value; a new name
    // is pushed in front.
    let mut set: Vec<(String, Template)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for (name, value) in list {
        match (at.get(&name), value) {
            (Some(&i), Some(v)) => set[i].1 = v,
            (Some(_), None) => {}
            (None, v) => {
                at.insert(name.clone(), set.len());
                set.push((name, v.unwrap_or(Template::Text(String::new()))));
            }
        }
    }
    let builtins = [
        ("input-file", Template::InputFile),
        ("modification-time", Template::ModificationTime),
        ("keyword", Template::Keyword),
        ("n", Template::N),
        ("property", Template::Property),
        ("time", Template::Time),
    ];
    let mut map = HashMap::new();
    // `set` was pushed in front: the lookup reads it from the end.
    for (name, t) in set.into_iter().rev() {
        map.entry(fold(&name)).or_insert(t);
    }
    for (name, t) in builtins {
        map.entry(name.to_string()).or_insert(t);
    }
    map
}

/// `format-time-string` of the fixed time, Thursday 1970-01-01 00:00:00
/// UTC, in the C locale. `None` for a directive outside the supported set
/// (a flag, a width, `%n`, a locale's date or time).
pub(crate) fn epoch_format(fmt: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        out.push_str(match chars.next()? {
            'Y' | 'G' => "1970",
            'y' | 'g' => "70",
            'C' => "19",
            'm' | 'd' => "01",
            'e' => " 1",
            'j' => "001",
            'H' | 'M' | 'S' | 'U' | 'W' => "00",
            'I' | 'l' => "12",
            'k' => " 0",
            'p' => "AM",
            'a' => "Thu",
            'A' => "Thursday",
            'b' | 'h' => "Jan",
            'B' => "January",
            'u' | 'w' => "4",
            'V' => "01",
            'Z' => "UTC",
            'z' => "+0000",
            's' => "0",
            'N' => "000000000",
            'T' => "00:00:00",
            'R' => "00:00",
            'D' => "01/01/70",
            'F' => "1970-01-01",
            't' => "\t",
            '%' => "%",
            _ => return None,
        });
    }
    Some(out)
}
