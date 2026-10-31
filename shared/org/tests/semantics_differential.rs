//! The semantics differential: `mae_org::semantics` against mae-kb's org
//! parser (`parse_org_multi_result`), the projection it replaces (ADR-112 D2,
//! D5), over the bundled corpus and `tests/semantics/*.org`.
//!
//! Every difference is first given a CLASS by a mechanical check that it is
//! one of the intended changes (`classify_*` below), or `unclassified`.
//! `conformance/semantics-divergences.toml` then lists, per class and file
//! pattern, why the difference is intended and which test pins mae-org's
//! side. An unlisted difference fails; so does an entry that matches
//! nothing, and an entry whose pinning test does not exist.
//!
//! The adapter maps `Semantics` to mae-kb's representation only where the
//! REPRESENTATION differs, never the meaning, and each mapping is what the O4
//! consumer will do: `ID` is a node field, not a property; a file node's
//! `KIND`/`ALIASES` are fields; a missing title is the id (file, item) or
//! empty (headline); a kind is `NodeKind::from_str_lossy` of the text; an
//! edge is a KB link with a source and a target or fragment.

mod support;

use mae_kb::org::{parse_org_multi_result, ParsedLink};
use mae_kb::{Node, NodeKind};
use mae_org::{
    classify_link, parse, semantics, Document, LinkKind, NodeOrigin, ParseOptions,
    SemanticDiagnostic, SemanticNode, Semantics, SyntaxKind::*, TodoSequence,
};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use support::{bundled_corpus, read, rel};

/// mae-kb hard-codes these TODO keywords; the differential declares the same
/// set (ADR-112 D4: they are data), so only in-file `#+TODO` lines differ.
fn options() -> ParseOptions {
    let words = |ws: &[&str]| ws.iter().map(|w| w.to_string()).collect();
    ParseOptions {
        todo_sequences: vec![TodoSequence {
            todo: words(&["TODO", "NEXT", "WAIT"]),
            done: words(&["DONE", "CANCELLED", "DEFERRED"]),
        }],
        ..ParseOptions::default()
    }
}

#[derive(Debug, Clone)]
struct Diff {
    what: &'static str,
    class: &'static str,
    file: String,
    text: String,
}

/// One node in mae-kb's representation.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    title: String,
    tags: Vec<String>,
    kind: NodeKind,
    aliases: Vec<String>,
    properties: BTreeMap<String, String>,
    todo: Option<String>,
    priority: Option<char>,
}

fn kb_row(n: &Node) -> Row {
    Row {
        title: n.title.clone(),
        tags: n.tags.clone(),
        kind: n.kind,
        aliases: n.aliases.clone(),
        properties: n.properties.clone().into_iter().collect(),
        todo: n.todo_state.clone(),
        priority: n.priority,
    }
}

fn sem_row(n: &SemanticNode) -> Row {
    let title = n.title.clone().unwrap_or_else(|| match n.origin {
        NodeOrigin::Headline => String::new(),
        _ => n.id.clone(),
    });
    let file = n.origin == NodeOrigin::File;
    let is_field = |k: &str| k == "ID" || (file && (k == "KIND" || k == "ALIASES"));
    Row {
        title,
        tags: n.tags.clone(),
        kind: NodeKind::from_str_lossy(n.kind.as_deref().unwrap_or("")),
        aliases: n.aliases.clone(),
        properties: n
            .properties
            .iter()
            .filter(|(k, _)| !is_field(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        todo: n.todo.clone(),
        priority: n.priority,
    }
}

/// An edge's payload: everything but its source.
#[derive(Debug, Clone, PartialEq)]
struct Payload {
    target: String,
    fragment: Option<String>,
    rel: String,
    weight: String,
    confidence: String,
    attrs: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
struct Edge {
    from: String,
    p: Payload,
}

fn kb_edge(from: &str, l: &ParsedLink) -> Edge {
    Edge {
        from: from.to_string(),
        p: Payload {
            target: l.target.clone(),
            fragment: l.fragment.clone(),
            rel: l.rel_type.clone(),
            weight: format!("{}", l.weight),
            confidence: format!("{}", l.confidence),
            attrs: l.attrs.clone(),
        },
    }
}

fn sem_edges(s: &Semantics) -> Vec<Edge> {
    s.links
        .iter()
        .filter(|l| l.kind == LinkKind::Kb && !(l.target.is_empty() && l.fragment.is_none()))
        .filter_map(|l| {
            Some(Edge {
                from: l.from.clone()?,
                p: Payload {
                    target: l.target.clone(),
                    fragment: l.fragment.clone(),
                    rel: l.rel.clone(),
                    weight: format!("{}", l.weight),
                    confidence: format!("{}", l.confidence),
                    attrs: l.attrs.clone(),
                },
            })
        })
        .collect()
}

/// The common part of two multisets removed: (only in a, only in b).
fn multiset_diff<T: PartialEq + Clone>(a: &[T], b: &[T]) -> (Vec<T>, Vec<T>) {
    let mut rest: Vec<Option<T>> = b.iter().cloned().map(Some).collect();
    let mut only_a = Vec::new();
    for x in a {
        match rest.iter_mut().find(|y| y.as_ref() == Some(x)) {
            Some(slot) => *slot = None,
            None => only_a.push(x.clone()),
        }
    }
    (only_a, rest.into_iter().flatten().collect())
}

/// Everything a classifier may consult about one file.
struct Ctx<'a> {
    file: String,
    text: &'a str,
    doc: &'a Document,
    sem: &'a Semantics,
    out: Vec<Diff>,
}

impl Ctx<'_> {
    fn push(&mut self, what: &'static str, class: &'static str, text: String) {
        self.out.push(Diff {
            what,
            class,
            file: self.file.clone(),
            text,
        });
    }

    fn node(&self, id: &str) -> Option<&SemanticNode> {
        self.sem.nodes.iter().find(|n| n.id == id)
    }

    fn file_id(&self) -> Option<&str> {
        self.sem
            .nodes
            .iter()
            .find(|n| n.origin == NodeOrigin::File)
            .map(|n| n.id.as_str())
    }

    fn duplicated(&self, id: &str) -> bool {
        self.sem
            .diagnostics
            .iter()
            .any(|d| matches!(d, SemanticDiagnostic::DuplicateId { id: i, .. } if i == id))
    }

    fn unrendered_id(&self, id: &str) -> bool {
        self.sem
            .diagnostics
            .iter()
            .any(|d| matches!(d, SemanticDiagnostic::IdInUnrenderedRegion { id: i, .. } if i == id))
    }

    /// Whether `offset` lies in a region the semantics reported unrendered.
    fn in_unrendered(&self, offset: usize) -> bool {
        self.sem.diagnostics.iter().any(|d| match d {
            SemanticDiagnostic::IdInUnrenderedRegion { .. }
            | SemanticDiagnostic::LinkInUnrenderedRegion { .. } => {
                d.span().range().contains(&offset)
            }
            _ => false,
        })
    }

    /// Where the text at `offset` sits, as a class: what kind of element
    /// keeps it from being what mae-kb read it as.
    fn place(&self, offset: usize) -> &'static str {
        if self.in_unrendered(offset) {
            return "unrendered_region";
        }
        let root = self.doc.syntax();
        let Some(tok) = root.token_at_offset((offset as u32).into()).right_biased() else {
            return "unclassified";
        };
        for a in tok.parent_ancestors() {
            let class = match a.kind() {
                LINK => "is_link",
                TABLE => "pending_table_cell",
                SRC_BLOCK | EXAMPLE_BLOCK => "verbatim_block",
                VERBATIM | CODE => "verbatim_object",
                FIXED_WIDTH => "verbatim_block",
                KEYWORD => "keyword_value",
                NODE_PROPERTY => "property_value",
                _ => continue,
            };
            return class;
        }
        "not_a_link_in_org"
    }
}

/// Offsets of `[[` links whose path (after `id:`) starts with `target`
/// followed by the end of the target.
fn link_occurrences(text: &str, target: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("[[") {
        let at = from + i;
        from = at + 1;
        let inner = &text[at + 2..];
        let inner = inner.strip_prefix("id:").unwrap_or(inner);
        let after = inner.strip_prefix(target).and_then(|r| r.chars().next());
        if matches!(after, Some(']' | '|' | '#' | '?')) {
            out.push(at + 2);
        }
    }
    out
}

/// The single class of the places, ignoring real links; `None` if mixed or
/// every place is a real link.
fn single_class(places: impl Iterator<Item = &'static str>) -> Option<&'static str> {
    let mut classes: Vec<&'static str> = places.filter(|c| *c != "is_link").collect();
    classes.dedup();
    match classes.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

fn compare(file: &str, text: &str) -> Vec<Diff> {
    let kb = parse_org_multi_result(text);
    let doc = parse(text, &options());
    let sem = semantics(&doc);
    let mut cx = Ctx {
        file: file.to_string(),
        text,
        doc: &doc,
        sem: &sem,
        out: Vec::new(),
    };
    compare_nodes(&mut cx, &kb.nodes);
    compare_links(&mut cx, &kb.typed_links);
    compare_transclusions(&mut cx, &kb.transclusions);
    cx.out
}

fn compare_nodes(cx: &mut Ctx, kb: &[Node]) {
    let mut first: Vec<&Node> = Vec::new();
    for n in kb {
        if !first.iter().any(|f| f.id == n.id) {
            first.push(n);
        }
    }
    for n in &first {
        match cx.node(&n.id).cloned() {
            Some(s) => compare_row(cx, &n.id, &kb_row(n), &sem_row(&s)),
            None => {
                let class = classify_kb_only_node(cx, &n.id);
                cx.push("node", class, format!("only mae-kb: {}", n.id));
            }
        }
    }
    for s in cx.sem.nodes.clone() {
        if !first.iter().any(|f| f.id == s.id) {
            cx.push("node", "unclassified", format!("only mae-org: {}", s.id));
        }
    }
}

fn classify_kb_only_node(cx: &Ctx, id: &str) -> &'static str {
    if cx.unrendered_id(id) {
        return "unrendered_region";
    }
    let line = format!(":ID: {id}");
    let places = cx.text.match_indices(&line).map(|(i, _)| cx.place(i));
    single_class(places).unwrap_or("unclassified")
}

fn compare_row(cx: &mut Ctx, id: &str, k: &Row, s: &Row) {
    let in_file_todo = cx.doc.todo_sequences() != options().todo_sequences.as_slice();
    let todo_class = if in_file_todo {
        "in_file_todo"
    } else {
        "unclassified"
    };
    if k.title != s.title {
        let class = if in_file_todo {
            "in_file_todo"
        } else if plain_title(&k.title) == s.title {
            "title_plain_text"
        } else {
            "unclassified"
        };
        cx.push(
            "title",
            class,
            format!("{id}: {:?} vs {:?}", k.title, s.title),
        );
    }
    if k.tags != s.tags {
        let mut dedup: Vec<String> = Vec::new();
        for t in &k.tags {
            if !dedup.contains(t) {
                dedup.push(t.clone());
            }
        }
        let class = if dedup == s.tags {
            "tags_once"
        } else {
            "unclassified"
        };
        cx.push("tags", class, format!("{id}: {:?} vs {:?}", k.tags, s.tags));
    }
    if k.kind != s.kind {
        cx.push(
            "kind",
            "unclassified",
            format!("{id}: {:?} vs {:?}", k.kind, s.kind),
        );
    }
    if k.aliases != s.aliases {
        let text = format!("{id}: {:?} vs {:?}", k.aliases, s.aliases);
        cx.push("aliases", "unclassified", text);
    }
    if k.properties != s.properties {
        let lower: BTreeMap<String, String> = s
            .properties
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect();
        let class = if lower == k.properties {
            "property_key_case"
        } else {
            "unclassified"
        };
        let text = format!("{id}: {:?} vs {:?}", k.properties, s.properties);
        cx.push("properties", class, text);
    }
    if k.todo != s.todo {
        cx.push(
            "todo",
            todo_class,
            format!("{id}: {:?} vs {:?}", k.todo, s.todo),
        );
    }
    if k.priority != s.priority {
        let text = format!("{id}: {:?} vs {:?}", k.priority, s.priority);
        cx.push("priority", todo_class, text);
    }
}

/// mae-kb's (raw) title read by mae-org's plain-text rule.
fn plain_title(raw: &str) -> String {
    let text = format!("* {raw}\n:PROPERTIES:\n:ID: x\n:END:\n");
    let doc = parse(&text, &ParseOptions::default());
    let sem = semantics(&doc);
    sem.nodes
        .first()
        .and_then(|n| n.title.clone())
        .unwrap_or_default()
}

fn compare_links(cx: &mut Ctx, kb: &[(String, ParsedLink)]) {
    let kb_edges: Vec<Edge> = kb.iter().map(|(f, l)| kb_edge(f, l)).collect();
    let sem_edges = sem_edges(cx.sem);
    let (mut only_kb, mut only_sem) = multiset_diff(&kb_edges, &sem_edges);
    pair_moved_edges(cx, &mut only_kb, &mut only_sem);
    classify_kb_only_edges(cx, only_kb);
    for e in only_sem {
        let class = classify_sem_only_edge(cx, &e, &kb_edges);
        cx.push("link", class, format!("only mae-org: {e:?}"));
    }
}

/// An edge both sides have, from different sources, where mae-kb's source
/// is not a node in mae-org: the content belongs to the node around it.
fn pair_moved_edges(cx: &mut Ctx, only_kb: &mut Vec<Edge>, only_sem: &mut Vec<Edge>) {
    let mut i = 0;
    while i < only_kb.len() {
        let e = &only_kb[i];
        let gone = cx.node(&e.from).is_none() || cx.duplicated(&e.from);
        let partner = only_sem.iter().position(|f| f.p == e.p);
        match partner {
            Some(j) if gone => {
                let f = only_sem.remove(j);
                let e = only_kb.remove(i);
                let class = if cx.duplicated(&e.from) {
                    "duplicate_id"
                } else {
                    "source_not_a_node"
                };
                cx.push("link", class, format!("moved: {e:?} -> {}", f.from));
            }
            _ => i += 1,
        }
    }
}

/// How many of mae-org's edges with `e`'s payload come from nodes strictly
/// inside `e`'s source: mae-kb's headline body runs to the next headline of
/// the same level, so it also counts each of these for every ancestor.
fn descendant_count(cx: &Ctx, e: &Edge) -> usize {
    let Some(parent) = cx.node(&e.from).map(|n| n.span) else {
        return 0;
    };
    sem_edges(cx.sem)
        .iter()
        .filter(|f| f.p == e.p && f.from != e.from)
        .filter_map(|f| cx.node(&f.from).map(|n| n.span))
        .filter(|c| parent.start <= c.start && c.end <= parent.end)
        .count()
}

/// mae-kb-only edges: an edge counted again for an ancestor is explained
/// only up to the number of descendant edges that account for it.
fn classify_kb_only_edges(cx: &mut Ctx, only_kb: Vec<Edge>) {
    let mut spent: Vec<(Edge, usize)> = Vec::new();
    for e in only_kb {
        let n = match spent.iter_mut().find(|u| u.0 == e) {
            Some(u) => {
                u.1 += 1;
                u.1
            }
            None => {
                spent.push((e.clone(), 1));
                1
            }
        };
        let class = if n <= descendant_count(cx, &e) {
            "ancestor_double_count"
        } else {
            classify_kb_only_edge(cx, &e)
        };
        cx.push("link", class, format!("only mae-kb: {e:?}"));
    }
}

fn classify_kb_only_edge(cx: &Ctx, e: &Edge) -> &'static str {
    if classify_link(&e.p.target).kind != LinkKind::Kb {
        return "external_classifier";
    }
    let places = link_occurrences(cx.text, &e.p.target)
        .into_iter()
        .map(|i| cx.place(i));
    single_class(places).unwrap_or("unclassified")
}

/// mae-kb drops a file-level edge when any headline has the same target and
/// relation, even where the file's own text links it too. Only an edge whose
/// link lies in the zeroth section can be such a drop.
fn classify_sem_only_edge(cx: &Ctx, e: &Edge, kb: &[Edge]) -> &'static str {
    let first_headline = cx
        .doc
        .headlines()
        .next()
        .map_or(usize::MAX, |h| usize::from(h.syntax().text_range().start()));
    let in_zeroth = cx.sem.links.iter().any(|l| {
        l.from.as_deref() == Some(e.from.as_str())
            && l.target == e.p.target
            && (l.span.start as usize) < first_headline
    });
    let on_a_headline = kb
        .iter()
        .any(|k| k.from != e.from && k.p.target == e.p.target && k.p.rel == e.p.rel);
    if Some(e.from.as_str()) == cx.file_id() && in_zeroth && on_a_headline {
        "file_link_dedup"
    } else {
        "unclassified"
    }
}

fn compare_transclusions(cx: &mut Ctx, kb: &[(String, String, String)]) {
    let sem: Vec<(String, String, String)> = cx
        .sem
        .transclusions
        .iter()
        .filter_map(|t| Some((t.from.clone()?, t.member.clone(), t.role.clone())))
        .collect();
    let (mut only_kb, mut only_sem) = multiset_diff(kb, &sem);
    // Same directive, attributed to the node around it rather than the file.
    only_kb.retain(|k| {
        let Some(j) = only_sem.iter().position(|s| s.1 == k.1 && s.2 == k.2) else {
            return true;
        };
        let s = only_sem.remove(j);
        cx.push(
            "transclusion",
            "innermost_node",
            format!("{k:?} -> {}", s.0),
        );
        false
    });
    for t in only_kb {
        let places = transclusion_lines(cx.text, &t.1).map(|i| cx.place(i));
        let class = single_class(places).unwrap_or("unclassified");
        cx.push("transclusion", class, format!("only mae-kb: {t:?}"));
    }
    for t in only_sem {
        let exact = ["#+TRANSCLUDE:", "#+transclude:"];
        let class = if exact
            .iter()
            .any(|p| cx.text.contains(&format!("{p} {}", t.1)))
        {
            "unclassified"
        } else {
            "keyword_case"
        };
        cx.push("transclusion", class, format!("only mae-org: {t:?}"));
    }
}

fn transclusion_lines<'a>(text: &'a str, member: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(member)
        .map(|(i, _)| i)
        .filter(move |&i| {
            let line_start = text[..i].rfind('\n').map_or(0, |n| n + 1);
            text[line_start..i]
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("#+transclude:")
        })
}

// ---------------------------------------------------------------- register

#[derive(Debug)]
struct Entry {
    what: String,
    class: String,
    file: String,
    detail: String,
    test: String,
    line: usize,
}

impl Entry {
    fn matches(&self, d: &Diff) -> bool {
        let file_ok = match self.file.strip_suffix('*') {
            Some(prefix) => d.file.starts_with(prefix),
            None => d.file == self.file,
        };
        self.what == d.what && self.class == d.class && file_ok && d.text.contains(&self.detail)
    }
}

fn register_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/semantics-divergences.toml")
}

/// The register: `[[divergence]]` tables of `key = "string"` lines.
fn load_register() -> Vec<Entry> {
    let text = read(&register_path());
    let mut rows: Vec<(usize, HashMap<String, String>)> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[divergence]]" {
            rows.push((n + 1, HashMap::new()));
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("register:{}: not `key = value`", n + 1));
        let v = v.trim();
        let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"'));
        let v = v.unwrap_or_else(|| panic!("register:{}: value must be a string", n + 1));
        let row = rows
            .last_mut()
            .unwrap_or_else(|| panic!("register:{}: outside a table", n + 1));
        row.1.insert(k.trim().to_string(), v.replace("\\\"", "\""));
    }
    rows.into_iter().map(entry).collect()
}

fn entry((line, mut m): (usize, HashMap<String, String>)) -> Entry {
    let mut take = |k: &str| m.remove(k).unwrap_or_default();
    let e = Entry {
        what: take("what"),
        class: take("class"),
        file: take("file"),
        detail: take("detail"),
        test: take("test"),
        line,
    };
    let reason = take("reason");
    assert!(
        !e.what.is_empty() && !e.class.is_empty() && !e.file.is_empty(),
        "register:{line}: what, class and file are required"
    );
    assert!(
        !reason.is_empty() && !e.test.is_empty(),
        "register:{line}: reason and test are required"
    );
    assert!(m.is_empty(), "register:{line}: unknown keys {:?}", m.keys());
    e
}

/// Every pinning test named by the register exists in this crate's sources.
fn assert_tests_exist(register: &[Entry]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = String::new();
    for dir in ["src", "tests"] {
        let mut stack = vec![root.join(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    sources.push_str(&read(&p));
                }
            }
        }
    }
    for e in register {
        assert!(
            sources.contains(&format!("fn {}(", e.test)),
            "register:{}: pinning test `{}` not found",
            e.line,
            e.test
        );
    }
}

/// Sizes of both projections over `files`, for the report.
fn print_totals(label: &str, files: &[PathBuf]) {
    let mut t = [0usize; 6];
    for path in files {
        let text = read(path);
        let kb = parse_org_multi_result(&text);
        let sem = semantics(&parse(&text, &options()));
        let tr = sem
            .transclusions
            .iter()
            .filter(|x| x.from.is_some())
            .count();
        let c = [
            kb.nodes.len(),
            sem.nodes.len(),
            kb.typed_links.len(),
            sem_edges(&sem).len(),
            kb.transclusions.len(),
            tr,
        ];
        t.iter_mut().zip(c).for_each(|(a, b)| *a += b);
    }
    println!(
        "{label}: {} files; nodes mae-kb {} / mae-org {}; edges {} / {}; transclusions {} / {}",
        files.len(),
        t[0],
        t[1],
        t[2],
        t[3],
        t[4],
        t[5]
    );
}

fn synthetic() -> Vec<PathBuf> {
    support::org_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/semantics"))
}

#[test]
fn semantics_agree_with_mae_kb_except_registered_divergences() {
    let register = load_register();
    assert_tests_exist(&register);
    let files: Vec<PathBuf> = bundled_corpus().into_iter().chain(synthetic()).collect();
    let mut diffs = Vec::new();
    for path in &files {
        diffs.extend(compare(&rel(path), &read(path)));
    }
    print_totals("bundled corpus", &bundled_corpus());
    print_totals("synthetic", &synthetic());
    let unlisted: Vec<String> = diffs
        .iter()
        .filter(|d| !register.iter().any(|e| e.matches(d)))
        .map(|d| format!("{} [{} / {}] {}", d.file, d.what, d.class, d.text))
        .collect();
    let stale: Vec<String> = register
        .iter()
        .filter(|e| !diffs.iter().any(|d| e.matches(d)))
        .map(|e| {
            format!(
                "line {}: {} / {} / {} {:?}",
                e.line, e.what, e.class, e.file, e.detail
            )
        })
        .collect();
    let mut counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for d in &diffs {
        *counts.entry((d.what, d.class)).or_default() += 1;
    }
    println!(
        "{} files, {} differences: {counts:#?}",
        files.len(),
        diffs.len()
    );
    assert!(
        unlisted.is_empty(),
        "differences not in the register:\n{}",
        unlisted.join("\n")
    );
    assert!(
        stale.is_empty(),
        "register entries that no longer occur:\n{}",
        stale.join("\n")
    );
}

/// The adapter and classifiers cannot hide a real disagreement: a node
/// title, a link source and a property changed in mae-org's output each
/// surface as an unclassified difference.
#[test]
fn the_differential_reports_an_injected_disagreement() {
    let text = ":PROPERTIES:\n:ID: f\n:END:\n#+title: T\n\n* H\n:PROPERTIES:\n:ID: h\n:X: 1\n:END:\nsee [[id:t]]\n";
    let base = compare("x.org", text);
    assert!(
        base.iter().all(|d| d.class == "property_key_case"),
        "{base:?}"
    );
    let kb = parse_org_multi_result(text);
    let doc = parse(text, &options());
    let mut sem = semantics(&doc);
    sem.nodes[1].title = Some("changed".into());
    sem.nodes[1].properties[1].1 = "2".into();
    sem.links[0].from = Some("f".into());
    let mut cx = Ctx {
        file: "x.org".into(),
        text,
        doc: &doc,
        sem: &sem,
        out: Vec::new(),
    };
    compare_nodes(&mut cx, &kb.nodes);
    compare_links(&mut cx, &kb.typed_links);
    let got: Vec<(&str, &str)> = cx.out.iter().map(|d| (d.what, d.class)).collect();
    assert!(got.contains(&("title", "unclassified")), "{got:?}");
    assert!(got.contains(&("properties", "unclassified")), "{got:?}");
    assert!(
        got.iter()
            .filter(|g| g.0 == "link")
            .all(|g| g.1 == "unclassified"),
        "{got:?}"
    );
    assert!(got.iter().any(|g| g.0 == "link"), "{got:?}");
}
