//! The facade's operations: a JSON request in, a JSON response out. Safe
//! Rust only; `lib.rs` moves the bytes across the ABI.
//!
//! Every response is `{"ok": true, "result": R}` or `{"ok": false,
//! "error": "message"}`. A request is an object; every key is optional
//! unless an operation needs it, and an unknown key is an error (a typo
//! must not silently fall back to a default):
//!
//! ```json
//! {"text": "* org text",
//!  "parse": {"todo": [{"todo": ["TODO"], "done": ["DONE"]}],
//!            "max_depth": 128, "link_types": ["id", "https", "…"]},
//!  "semantics": {"exclude_tags": ["noexport"], "select_tags": [],
//!                "archive_tag": "ARCHIVE",
//!                "archived_trees": "headline" | "all" | "none",
//!                "link_types": {"web": [], "mail": [], "other": []},
//!                "link_exclude_keywords": [], "link_exclude_properties": []},
//!  "render": {"headline_ids": "explicit" | "slug", "images": false,
//!             "property_drawers": false},
//!  "resolver": {"targets": {"KB id": {"href": "/x.html", "title": "X"}},
//!               "placeholder": "[?]"},
//!  "node": "KB id",
//!  "path": "id:x?rel=r",
//!  "section": {…}, "file": {…},           // syntaxSection: see `syntax.rs`
//!  "ranges": [[0, 10]], "uuids": ["…"],     // CUSTOM_ID ops: see `custom_id.rs`
//!  "position": 12, "uuid": "…"}
//! ```
//!
//! Spans in results are UTF-8 BYTE offsets into `text`.

use crate::json::{parse, Json};
use mae_org::{
    json_string, ArchivedTrees, HeadlineIds, LinkTypes, ParseOptions, RenderOptions, Resolved,
    Resolver, SemanticsOptions, TodoSequence, TreeFormat,
};

/// An operation, by its ABI number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Semantics = 1,
    RenderHast = 2,
    RenderHtml = 3,
    Text = 4,
    ClassifyLink = 5,
    Version = 6,
    Syntax = 7,
    SyntaxSection = 8,
    HeadingsMissingCustomId = 9,
    CustomIdEdits = 10,
    DuplicateCustomIds = 11,
    ReplaceCustomId = 12,
}

impl Op {
    pub fn from_u32(n: u32) -> Option<Op> {
        [
            Op::Semantics,
            Op::RenderHast,
            Op::RenderHtml,
            Op::Text,
            Op::ClassifyLink,
            Op::Version,
            Op::Syntax,
            Op::SyntaxSection,
            Op::HeadingsMissingCustomId,
            Op::CustomIdEdits,
            Op::DuplicateCustomIds,
            Op::ReplaceCustomId,
        ]
        .into_iter()
        .find(|o| *o as u32 == n)
    }
}

/// Runs operation `op` on `request` (JSON). Total: any input gives a
/// response.
pub fn call(op: u32, request: &str) -> String {
    let result = Op::from_u32(op)
        .ok_or_else(|| format!("unknown operation {op}"))
        .and_then(|op| run(op, request));
    match result {
        Ok(r) => format!("{{\"ok\":true,\"result\":{r}}}"),
        Err(e) => format!("{{\"ok\":false,\"error\":{}}}", json_string(&e)),
    }
}

fn run(op: Op, request: &str) -> Result<String, String> {
    if op == Op::Version {
        return Ok(version());
    }
    if !mae_org::seeded::is_seeded() {
        return Err("the module is not seeded: call mae_org_seed first (the loader does)".into());
    }
    let req = Request::read(&parse(request)?)?;
    match op {
        Op::Semantics => Ok(semantics(&req)),
        Op::RenderHast | Op::RenderHtml => {
            let format = if op == Op::RenderHast {
                TreeFormat::Hast
            } else {
                TreeFormat::Html
            };
            let (doc, sem) = req.document();
            Ok(mae_org::render(&doc, &sem, &req.render, &req.resolver).to_json(format))
        }
        Op::Text => text(&req),
        Op::ClassifyLink => {
            let path = req.path.as_deref().ok_or("classify_link needs \"path\"")?;
            Ok(mae_org::classify_link_with(path, &req.semantics.link_types).to_json())
        }
        Op::Version => Ok(version()),
        Op::Syntax => Ok(crate::syntax::syntax(&req.text, &req.parse, req.post_blank)),
        Op::SyntaxSection => crate::syntax::syntax_section(
            &req.text,
            &req.parse,
            req.section.as_ref(),
            req.file.as_ref(),
            req.post_blank,
        ),
        Op::HeadingsMissingCustomId => Ok(crate::custom_id::missing(
            &req.text,
            &req.parse,
            &req.custom_id,
        )),
        Op::CustomIdEdits => crate::custom_id::edits(&req.text, &req.parse, &req.custom_id),
        Op::DuplicateCustomIds => Ok(crate::custom_id::duplicates(&req.text, &req.parse)),
        Op::ReplaceCustomId => crate::custom_id::replace(&req.text, &req.parse, &req.custom_id),
    }
}

fn version() -> String {
    format!(
        "{{\"api\":{},\"crate\":{}}}",
        json_string(mae_org::API_VERSION),
        json_string(env!("CARGO_PKG_VERSION"))
    )
}

fn semantics(req: &Request) -> String {
    let (doc, sem) = req.document();
    let diags: Vec<String> = doc
        .diagnostics()
        .iter()
        .map(|d| {
            format!(
                "{{\"kind\":\"{}\",\"start\":{},\"end\":{}}}",
                d.kind.as_str(),
                d.span.start,
                d.span.end
            )
        })
        .collect();
    let eol = match doc.line_ending() {
        mae_org::LineEnding::Lf => "lf",
        mae_org::LineEnding::CrLf => "crlf",
        mae_org::LineEnding::Cr => "cr",
    };
    format!(
        "{{\"semantics\":{},\"diagnostics\":[{}],\"line_ending\":\"{eol}\"}}",
        sem.to_json(),
        diags.join(",")
    )
}

fn text(req: &Request) -> Result<String, String> {
    let (doc, sem) = req.document();
    let t = match &req.node {
        Some(id) => mae_org::node_text(&doc, &sem, id).ok_or_else(|| format!("no node {id:?}"))?,
        None => mae_org::text(&doc, &sem),
    };
    Ok(json_string(&t))
}

/// A parsed request.
struct Request {
    text: String,
    parse: ParseOptions,
    semantics: SemanticsOptions,
    render: RenderOptions,
    resolver: Resolver,
    node: Option<String>,
    path: Option<String>,
    /// For `syntaxSection`: the region and file facts `syntax` returned.
    section: Option<Json>,
    file: Option<Json>,
    /// For `syntax`/`syntaxSection`: whether a span's end includes its
    /// post-blank (default true, as `org-element` reports it).
    post_blank: bool,
    /// For the `CUSTOM_ID` operations: `ranges`, `uuids`, `position`, `uuid`.
    custom_id: crate::custom_id::CustomIdRequest,
}

const KEYS: &[&str] = &[
    "text",
    "parse",
    "semantics",
    "render",
    "resolver",
    "node",
    "path",
    "section",
    "file",
    "post_blank",
    "ranges",
    "uuids",
    "position",
    "uuid",
];

impl Request {
    fn read(j: &Json) -> Result<Request, String> {
        known_keys(j, KEYS, "request")?;
        Ok(Request {
            text: opt_str(j.get("text"), "text")?.unwrap_or_default(),
            parse: parse_options(j.get("parse"))?,
            semantics: semantics_options(j.get("semantics"))?,
            render: render_options(j.get("render"))?,
            resolver: resolver(j.get("resolver"))?,
            node: opt_str(j.get("node"), "node")?,
            path: opt_str(j.get("path"), "path")?,
            section: j.get("section").filter(|v| **v != Json::Null).cloned(),
            file: j.get("file").filter(|v| **v != Json::Null).cloned(),
            post_blank: opt_bool(j.get("post_blank"), "post_blank")?.unwrap_or(true),
            custom_id: crate::custom_id::CustomIdRequest {
                ranges: crate::custom_id::ranges(j.get("ranges"))?,
                uuids: strs(j.get("uuids"), "uuids")?,
                position: crate::custom_id::offset(j.get("position"), "position")?,
                uuid: opt_str(j.get("uuid"), "uuid")?,
            },
        })
    }

    fn document(&self) -> (mae_org::Document, mae_org::Semantics) {
        let doc = mae_org::parse(&self.text, &self.parse);
        let sem = mae_org::semantics_with(&doc, &self.semantics);
        (doc, sem)
    }
}

fn known_keys(j: &Json, keys: &[&str], what: &str) -> Result<(), String> {
    let Json::Obj(kv) = j else {
        return Err(format!("{what} must be an object"));
    };
    match kv.iter().find(|(k, _)| !keys.contains(&k.as_str())) {
        Some((k, _)) => Err(format!("unknown key {k:?} in {what}")),
        None => Ok(()),
    }
}

fn opt_str(j: Option<&Json>, what: &str) -> Result<Option<String>, String> {
    match j {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{what} must be a string")),
    }
}

fn strs(j: Option<&Json>, what: &str) -> Result<Option<Vec<String>>, String> {
    match j {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Arr(v)) => v
            .iter()
            .map(|x| match x {
                Json::Str(s) => Ok(s.clone()),
                _ => Err(format!("{what} must hold strings")),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(format!("{what} must be an array")),
    }
}

fn opt_bool(j: Option<&Json>, what: &str) -> Result<Option<bool>, String> {
    match j {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("{what} must be a boolean")),
    }
}

fn parse_options(j: Option<&Json>) -> Result<ParseOptions, String> {
    let mut o = ParseOptions::default();
    let Some(j) = j.filter(|j| **j != Json::Null) else {
        return Ok(o);
    };
    known_keys(j, &["todo", "max_depth", "link_types"], "parse")?;
    match j.get("todo") {
        None | Some(Json::Null) | Some(Json::Arr(_)) => {}
        Some(_) => return Err("parse.todo must be an array".into()),
    }
    if let Some(Json::Arr(seqs)) = j.get("todo") {
        o.todo_sequences = seqs
            .iter()
            .map(|s| {
                known_keys(s, &["todo", "done"], "parse.todo")?;
                Ok(TodoSequence::new(
                    strs(s.get("todo"), "todo")?.unwrap_or_default(),
                    strs(s.get("done"), "done")?.unwrap_or_default(),
                ))
            })
            .collect::<Result<_, String>>()?;
    }
    match j.get("max_depth") {
        None | Some(Json::Null) => {}
        Some(Json::Num(n)) if *n >= 1.0 && n.fract() == 0.0 => {
            o.max_depth = (*n as usize).min(mae_org::MAX_DEPTH_CEILING)
        }
        Some(_) => return Err("parse.max_depth must be a positive integer".into()),
    }
    if let Some(t) = strs(j.get("link_types"), "parse.link_types")? {
        o.link_types = t;
    }
    Ok(o)
}

fn semantics_options(j: Option<&Json>) -> Result<SemanticsOptions, String> {
    let mut o = SemanticsOptions::default();
    let Some(j) = j.filter(|j| **j != Json::Null) else {
        return Ok(o);
    };
    let keys = [
        "exclude_tags",
        "select_tags",
        "archive_tag",
        "archived_trees",
        "link_types",
        "link_exclude_keywords",
        "link_exclude_properties",
    ];
    known_keys(j, &keys, "semantics")?;
    let set = |dst: &mut Vec<String>, key: &str| -> Result<(), String> {
        if let Some(v) = strs(j.get(key), key)? {
            *dst = v;
        }
        Ok(())
    };
    set(&mut o.exclude_tags, "exclude_tags")?;
    set(&mut o.select_tags, "select_tags")?;
    set(&mut o.link_exclude_keywords, "link_exclude_keywords")?;
    set(&mut o.link_exclude_properties, "link_exclude_properties")?;
    if let Some(t) = opt_str(j.get("archive_tag"), "archive_tag")? {
        o.archive_tag = t;
    }
    o.archived_trees = match opt_str(j.get("archived_trees"), "archived_trees")?.as_deref() {
        None | Some("headline") => ArchivedTrees::Headline,
        Some("all") => ArchivedTrees::All,
        Some("none") => ArchivedTrees::None,
        Some(other) => return Err(format!("archived_trees {other:?}: headline, all or none")),
    };
    if let Some(lt) = j.get("link_types").filter(|v| **v != Json::Null) {
        known_keys(lt, &["web", "mail", "other"], "semantics.link_types")?;
        let mut t = LinkTypes::default();
        if let Some(v) = strs(lt.get("web"), "web")? {
            t.web = v;
        }
        if let Some(v) = strs(lt.get("mail"), "mail")? {
            t.mail = v;
        }
        if let Some(v) = strs(lt.get("other"), "other")? {
            t.other = v;
        }
        o.link_types = t;
    }
    Ok(o)
}

fn render_options(j: Option<&Json>) -> Result<RenderOptions, String> {
    let Some(j) = j.filter(|j| **j != Json::Null) else {
        return Ok(RenderOptions::default());
    };
    known_keys(j, &["headline_ids", "images", "property_drawers"], "render")?;
    let ids = match opt_str(j.get("headline_ids"), "headline_ids")?.as_deref() {
        None | Some("explicit") => HeadlineIds::Explicit,
        Some("slug") => HeadlineIds::Slug,
        Some(other) => return Err(format!("headline_ids {other:?}: explicit or slug")),
    };
    let images = opt_bool(j.get("images"), "images")?.unwrap_or(false);
    let drawers = opt_bool(j.get("property_drawers"), "property_drawers")?.unwrap_or(false);
    Ok(RenderOptions::default()
        .with_headline_ids(ids)
        .with_images(images)
        .with_property_drawers(drawers))
}

fn resolver(j: Option<&Json>) -> Result<Resolver, String> {
    let mut r = Resolver::default();
    let Some(j) = j.filter(|j| **j != Json::Null) else {
        return Ok(r);
    };
    known_keys(j, &["targets", "placeholder"], "resolver")?;
    if let Some(p) = opt_str(j.get("placeholder"), "placeholder")? {
        r.placeholder = p;
    }
    match j.get("targets") {
        None | Some(Json::Null) => {}
        Some(Json::Obj(kv)) => {
            for (id, v) in kv {
                known_keys(v, &["href", "title"], "a resolver target")?;
                let href = opt_str(v.get("href"), "href")?
                    .ok_or_else(|| format!("resolver target {id:?} has no href"))?;
                let title = opt_str(v.get("title"), "title")?;
                let mut target = Resolved::new(href);
                target.title = title;
                r.targets.insert(id.clone(), target);
            }
        }
        Some(_) => return Err("resolver.targets must be an object".into()),
    }
    Ok(r)
}
