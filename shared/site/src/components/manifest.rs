//! `components.toml`, read and checked (ADR-119 H1).
//!
//! The manifest is read field by field from a TOML table rather than through
//! a serde struct, so that one load reports every problem at once, each
//! naming its component and field, instead of stopping at the first.

use std::collections::{BTreeMap, BTreeSet};

use super::clean::attribute_problem;
use super::shape::{parse_field, Shape};
use super::{LoadError, Slot};
use crate::address::{export_collision_key, segment, valid_id};

/// The most components one manifest may declare (D21).
pub const MAX_COMPONENTS: usize = 64;

/// The most bytes a template, behaviour or stylesheet may hold (D21).
pub const MAX_FILE_BYTES: usize = 256 * 1024;

/// One component as the manifest declares it.
#[derive(Debug, Clone)]
pub(super) struct Spec {
    pub name: String,
    pub slot: Option<Slot>,
    pub template: Option<String>,
    pub script: Option<String>,
    pub stylesheet: Option<String>,
    pub attributes: BTreeSet<String>,
    /// The data file, its shape, and its data once validated.
    pub data: Option<(String, Shape, serde_json::Value)>,
    pub page: Option<PageSpec>,
}

/// A page a component emits.
#[derive(Debug, Clone)]
pub(super) struct PageSpec {
    pub name: String,
    pub title: String,
    pub template: String,
}

/// The fields of one TOML table, read once each; what is left is unknown.
struct Fields<'a> {
    table: &'a toml::Table,
    component: String,
    prefix: &'static str,
    read: BTreeSet<&'a str>,
}

impl<'a> Fields<'a> {
    fn new(table: &'a toml::Table, component: &str, prefix: &'static str) -> Self {
        Fields {
            table,
            component: component.to_string(),
            prefix,
            read: BTreeSet::new(),
        }
    }

    fn err(&self, errors: &mut Vec<LoadError>, field: &str, reason: impl Into<String>) {
        errors.push(LoadError {
            component: Some(self.component.clone()),
            field: format!("{}{field}", self.prefix),
            reason: reason.into(),
        });
    }

    fn get(&mut self, key: &'a str) -> Option<&'a toml::Value> {
        self.read.insert(key);
        self.table.get(key)
    }

    fn string(&mut self, key: &'a str, required: bool, e: &mut Vec<LoadError>) -> Option<String> {
        match self.get(key) {
            Some(toml::Value::String(s)) => Some(s.clone()),
            Some(_) => {
                self.err(e, key, "is not a string");
                None
            }
            None if required => {
                self.err(e, key, "is required");
                None
            }
            None => None,
        }
    }

    fn table(&mut self, key: &'a str, e: &mut Vec<LoadError>) -> Option<&'a toml::Table> {
        match self.get(key) {
            Some(toml::Value::Table(t)) => Some(t),
            Some(_) => {
                self.err(e, key, "is not a table");
                None
            }
            None => None,
        }
    }

    /// A file the component names: one plain name in its directory, and
    /// supplied.
    fn file(
        &mut self,
        key: &'a str,
        required: bool,
        ext: &[&str],
        files: &BTreeMap<String, String>,
        e: &mut Vec<LoadError>,
    ) -> Option<String> {
        let name = self.string(key, required, e)?;
        if let Some(why) = file_problem(&name, ext, files) {
            self.err(e, key, format!("`{name}` {why}"));
            return None;
        }
        Some(name)
    }

    fn finish(self, e: &mut Vec<LoadError>) {
        for key in self.table.keys() {
            if !self.read.contains(key.as_str()) {
                self.err(e, key, "is not a field this manifest knows");
            }
        }
    }
}

/// Why `name` cannot be a component's file: not one plain name (so the
/// directory loader can never be sent elsewhere), a wrong extension, not
/// supplied, or too large.
fn file_problem(name: &str, ext: &[&str], files: &BTreeMap<String, String>) -> Option<String> {
    if !plain_file_name(name) {
        return Some("is not one plain file name ([A-Za-z0-9._-], not starting with `.`)".into());
    }
    if !ext.iter().any(|x| name.ends_with(x)) {
        return Some(format!("does not end in {}", ext.join(" or ")));
    }
    match files.get(name) {
        None => Some("is not in the component directory".into()),
        Some(t) if t.len() > MAX_FILE_BYTES => Some(format!(
            "is {} bytes; a component file may hold at most {MAX_FILE_BYTES}",
            t.len()
        )),
        Some(_) => None,
    }
}

/// One plain file name in the component directory: `[A-Za-z0-9._-]`, not
/// starting with `.`. No separator and no dot segment, so a name can never
/// reach outside the directory.
fn plain_file_name(name: &str) -> bool {
    !name.starts_with('.')
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// The plain file names `text` names (each component's `template`,
/// `script`, `stylesheet`, `data.file` and `page.template`), for a loader to
/// read and hand to [`super::Components::load`]. A name that is not plain is
/// left out: the load then reports it. Nothing is checked here; an
/// unreadable manifest names nothing.
pub(super) fn named_files(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(doc) = toml::from_str::<toml::Table>(text) else {
        return out;
    };
    let Some(list) = doc.get("component").and_then(toml::Value::as_array) else {
        return out;
    };
    let mut take = |t: &toml::Table, key: &str| {
        if let Some(name) = t.get(key).and_then(toml::Value::as_str) {
            if plain_file_name(name) {
                out.insert(name.to_string());
            }
        }
    };
    for t in list.iter().filter_map(toml::Value::as_table) {
        for key in ["template", "script", "stylesheet"] {
            take(t, key);
        }
        if let Some(d) = t.get("data").and_then(toml::Value::as_table) {
            take(d, "file");
        }
        if let Some(p) = t.get("page").and_then(toml::Value::as_table) {
            take(p, "template");
        }
    }
    out
}

/// A component name: `[a-z0-9][a-z0-9-]*`, at most 64. It appears in logs
/// and in served asset names.
fn valid_name(s: &str) -> bool {
    s.len() <= 64
        && s.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Every component in `text` that is valid on its own, and every problem
/// found. The caller compiles the valid ones' templates before reporting, so
/// a compile error is reported with the rest.
pub(super) fn parse(text: &str, files: &BTreeMap<String, String>) -> (Vec<Spec>, Vec<LoadError>) {
    let top_err = |field: &str, reason: String| LoadError {
        component: None,
        field: field.to_string(),
        reason,
    };
    let doc: toml::Table = match toml::from_str(text) {
        Ok(doc) => doc,
        Err(e) => return (Vec::new(), vec![top_err("", e.to_string())]),
    };
    let mut errors = Vec::new();
    for key in doc.keys().filter(|k| *k != "component") {
        errors.push(top_err(key, "is not a field this manifest knows".into()));
    }
    let list: &[toml::Value] = match doc.get("component") {
        None => &[],
        Some(toml::Value::Array(a)) => a,
        Some(_) => {
            errors.push(top_err("component", "is not a list of tables".into()));
            &[]
        }
    };
    if list.len() > MAX_COMPONENTS {
        errors.push(top_err(
            "component",
            format!(
                "declares {} components; a manifest may declare at most {MAX_COMPONENTS}",
                list.len()
            ),
        ));
    }
    let mut specs = Vec::new();
    for (i, v) in list.iter().enumerate() {
        let Some(t) = v.as_table() else {
            errors.push(top_err(&format!("component[{i}]"), "is not a table".into()));
            continue;
        };
        if let Some(s) = component(t, i, files, &mut errors) {
            specs.push(s);
        }
    }
    across(&specs, &mut errors);
    (specs, errors)
}

/// One component's table.
fn component(
    t: &toml::Table,
    i: usize,
    files: &BTreeMap<String, String>,
    e: &mut Vec<LoadError>,
) -> Option<Spec> {
    let label = match t.get("name") {
        Some(toml::Value::String(s)) => s.clone(),
        _ => format!("#{}", i + 1),
    };
    let before = e.len();
    let mut f = Fields::new(t, &label, "");
    let name = f.string("name", true, e);
    if name.as_deref().is_some_and(|n| !valid_name(n)) {
        f.err(
            e,
            "name",
            "is not [a-z0-9][a-z0-9-]*, at most 64 characters",
        );
    }
    let slot = f.string("slot", false, e).and_then(|s| {
        let slot = Slot::parse(&s);
        if slot.is_none() {
            f.err(e, "slot", format!("`{s}` is not one of {}", Slot::names()));
        }
        slot
    });
    let template = f.file("template", slot.is_some(), &[".html"], files, e);
    let script = f.file("script", false, &[".js", ".mjs"], files, e);
    let stylesheet = f.file("stylesheet", false, &[".css"], files, e);
    let attributes = attributes(&mut f, slot, e);
    let data = f.table("data", e).and_then(|d| data(d, &label, files, e));
    let page = f.table("page", e).and_then(|p| page(p, &label, files, e));
    let has = |k: &str| f.table.contains_key(k);
    let (has_slot, has_page, has_data, has_template) =
        (has("slot"), has("page"), has("data"), has("template"));
    if !has_slot && !has_page {
        f.err(
            e,
            "slot",
            "is required: a component either fills a slot or emits a page",
        );
    }
    if has_page && !has_data {
        f.err(
            e,
            "page",
            "is rendered from the component's data, and it declares none",
        );
    }
    if !has_slot && has_template {
        f.err(
            e,
            "template",
            "is a slot's template, and the component fills no slot",
        );
    }
    f.finish(e);
    (e.len() == before).then(|| Spec {
        name: name.unwrap_or_default(),
        slot,
        template,
        script,
        stylesheet,
        attributes,
        data,
        page,
    })
}

fn attributes(f: &mut Fields<'_>, slot: Option<Slot>, e: &mut Vec<LoadError>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Some(v) = f.get("attributes") else {
        return out;
    };
    let Some(list) = v.as_array() else {
        f.err(e, "attributes", "is not a list of strings");
        return out;
    };
    if slot == Some(Slot::DocumentHead) && !list.is_empty() {
        f.err(
            e,
            "attributes",
            "cannot be declared in the document head, which admits only meta[name, content]",
        );
        return out;
    }
    for a in list {
        match a.as_str() {
            Some(name) => match attribute_problem(name) {
                Some(why) => f.err(e, "attributes", format!("`{name}` {why}")),
                None => {
                    out.insert(name.to_string());
                }
            },
            None => f.err(e, "attributes", "is not a list of strings"),
        }
    }
    out
}

fn data(
    t: &toml::Table,
    label: &str,
    files: &BTreeMap<String, String>,
    e: &mut Vec<LoadError>,
) -> Option<(String, Shape, serde_json::Value)> {
    let mut f = Fields::new(t, label, "data.");
    let before = e.len();
    let file = f.file("file", true, &[".json"], files, e);
    let list = match f.get("list") {
        None => false,
        Some(toml::Value::Boolean(b)) => *b,
        Some(_) => {
            f.err(e, "list", "is not true or false");
            false
        }
    };
    let mut fields = Vec::new();
    match f.table("fields", e) {
        None if !f.table.contains_key("fields") => f.err(e, "fields", "is required"),
        None => {}
        Some(ft) if ft.is_empty() => f.err(e, "fields", "declares no field"),
        Some(ft) => {
            for (k, v) in ft {
                match v.as_str().map(|ty| parse_field(k, ty)) {
                    Some(Ok(field)) => fields.push(field),
                    Some(Err(why)) => f.err(e, "fields", why),
                    None => f.err(e, "fields", format!("field `{k}`'s type is not a string")),
                }
            }
        }
    }
    f.finish(e);
    if e.len() != before {
        return None;
    }
    let file = file?;
    let shape = Shape { list, fields };
    // The data itself is checked once the shape is known (H2: a malformed
    // entry stops the daemon).
    match shape.validate(&files[&file]) {
        Ok(v) => Some((file, shape, v)),
        Err(problems) => {
            for p in problems {
                let sep = if p.starts_with(['[', '.']) { "" } else { " " };
                e.push(LoadError {
                    component: Some(label.to_string()),
                    field: "data.file".into(),
                    reason: format!("`{file}`{sep}{p}"),
                });
            }
            None
        }
    }
}

fn page(
    t: &toml::Table,
    label: &str,
    files: &BTreeMap<String, String>,
    e: &mut Vec<LoadError>,
) -> Option<PageSpec> {
    let mut f = Fields::new(t, label, "page.");
    let before = e.len();
    let name = f.string("name", true, e);
    if let Some(n) = &name {
        // One URL segment that reads as written: `/kb/<kb-id>/site/<name>`
        // live, `<base>/site/<name>/` in the export (H2).
        if !valid_id(n) || segment(n) != *n {
            f.err(
                e,
                "name",
                format!("`{n}` is not one URL segment of letters, digits, `-`, `_`, `.` or `~`, and not all dots"),
            );
        }
    }
    let title = f.string("title", true, e);
    if title.as_deref().is_some_and(|t| t.trim().is_empty()) {
        f.err(e, "title", "is empty");
    }
    let template = f.file("template", true, &[".html"], files, e);
    f.finish(e);
    (e.len() == before).then(|| PageSpec {
        name: name.unwrap_or_default(),
        title: title.unwrap_or_default(),
        template: template.unwrap_or_default(),
    })
}

/// Rules across components: unique names, and unique page names as the
/// export compares directories.
fn across(specs: &[Spec], e: &mut Vec<LoadError>) {
    let mut names = BTreeSet::new();
    let mut pages = BTreeMap::new();
    for s in specs {
        if !names.insert(s.name.as_str()) {
            e.push(LoadError {
                component: Some(s.name.clone()),
                field: "name".into(),
                reason: "is declared by another component too".into(),
            });
        }
        if let Some(p) = &s.page {
            if let Some(other) = pages.insert(export_collision_key(&p.name), s.name.as_str()) {
                e.push(LoadError {
                    component: Some(s.name.clone()),
                    field: "page.name".into(),
                    reason: format!(
                        "`{}` is the same address as component `{other}`'s page",
                        p.name
                    ),
                });
            }
        }
    }
}
