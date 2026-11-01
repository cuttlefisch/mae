//! Every output channel of mae-org for one document, in every render mode:
//! the body as HTML and as hast, `text()`, `node_text` of every node, and
//! every `nodes[]` field (`title`, `title_unlinked`, `aliases`,
//! `properties`, `anchor`) as HTML and as hast, plus the whole result JSON.
//! Shared with `examples/probe.rs` (included by path).
#![allow(dead_code)]

use mae_org::{
    node_text, parse, render, semantics, text, to_hast, to_html, HeadlineIds, LinkKind, Node,
    NodeMeta, ParseOptions, RenderDiagnostic, RenderOptions, Rendered, Resolved, Resolver,
    Semantics, TreeFormat,
};

/// Whether a channel is the page itself or metadata about its nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Body,
    Meta,
}

/// One channel's content.
#[derive(Debug, Clone)]
pub struct Channel {
    pub name: String,
    pub kind: Kind,
    /// Rendered with `property_drawers` on.
    pub pd: bool,
    pub text: String,
}

/// Everything mae-org outputs for a document.
#[derive(Debug, Clone)]
pub struct MaeView {
    pub channels: Vec<Channel>,
    /// No mode rendered any body or node, and every text is empty.
    pub empty: bool,
    /// The render failed on a macro (`MacroFailed`).
    pub failed: bool,
    /// Diagnostic kinds of the default render.
    pub diags: Vec<&'static str>,
}

/// A render mode: `(property_drawers, headline_ids, full resolver)`.
pub type Mode = (bool, HeadlineIds, bool);

/// Every mode the oracle checks.
pub fn modes() -> Vec<Mode> {
    let mut out = Vec::new();
    for pd in [false, true] {
        for ids in [HeadlineIds::Explicit, HeadlineIds::Slug] {
            for res in [false, true] {
                out.push((pd, ids, res));
            }
        }
    }
    out
}

pub fn options(pd: bool, ids: HeadlineIds) -> RenderOptions {
    let mut o = RenderOptions::default();
    o.property_drawers = pd;
    o.headline_ids = ids;
    o
}

/// A resolver that resolves every KB link target and every node id, to
/// opaque hrefs and titles (no marker can come from the resolver).
pub fn full_resolver(sem: &Semantics) -> Resolver {
    let mut r = Resolver::default();
    let kb = sem.links.iter().filter(|l| l.kind == LinkKind::Kb);
    let ids = kb
        .map(|l| l.target.clone())
        .chain(sem.nodes.iter().map(|n| n.id.clone()));
    for (i, id) in ids.enumerate() {
        let to = Resolved::new(format!("/n/r{i}")).with_title(format!("resolved r{i}"));
        r.targets.entry(id).or_insert(to);
    }
    r
}

/// The render of `text` in `mode`.
pub fn render_mode(text: &str, mode: Mode) -> Rendered {
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics(&doc);
    let resolver = if mode.2 {
        full_resolver(&sem)
    } else {
        Resolver::default()
    };
    render(&doc, &sem, &options(mode.0, mode.1), &resolver)
}

/// Every channel of `text`.
pub fn mae_view(text_in: &str) -> MaeView {
    let doc = parse(text_in, &ParseOptions::default());
    let sem = semantics(&doc);
    let mut channels = Vec::new();
    let mut empty = true;
    let mut failed = false;
    let mut diags = Vec::new();
    for mode in modes() {
        let resolver = if mode.2 {
            full_resolver(&sem)
        } else {
            Resolver::default()
        };
        let r = render(&doc, &sem, &options(mode.0, mode.1), &resolver);
        empty &= r.body.is_empty() && r.nodes.is_empty();
        if diags.is_empty() {
            diags = r.diagnostics.iter().map(|d| d.kind_name()).collect();
        }
        failed |= r
            .diagnostics
            .iter()
            .any(|d| matches!(d, RenderDiagnostic::MacroFailed(..)));
        push_render(&mut channels, &r, mode);
    }
    let t = text(&doc, &sem);
    empty &= t.is_empty();
    channels.push(channel("text".into(), Kind::Body, false, t));
    for n in &sem.nodes {
        let nt = node_text(&doc, &sem, &n.id).unwrap_or_default();
        empty &= nt.is_empty();
        channels.push(channel(
            format!("node_text[{}]", n.id),
            Kind::Meta,
            false,
            nt,
        ));
    }
    MaeView {
        channels,
        empty,
        failed,
        diags,
    }
}

fn channel(name: String, kind: Kind, pd: bool, text: String) -> Channel {
    Channel {
        name,
        kind,
        pd,
        text,
    }
}

fn mode_name((pd, ids, res): Mode) -> String {
    let ids = if ids == HeadlineIds::Slug {
        "slug"
    } else {
        "explicit"
    };
    format!("pd={pd},ids={ids},res={res}")
}

fn push_render(out: &mut Vec<Channel>, r: &Rendered, mode: Mode) {
    let m = mode_name(mode);
    let pd = mode.0;
    out.push(channel(
        format!("{m}:body.html"),
        Kind::Body,
        pd,
        to_html(&r.body),
    ));
    out.push(channel(
        format!("{m}:body.hast"),
        Kind::Body,
        pd,
        to_hast(&r.body),
    ));
    for f in [TreeFormat::Html, TreeFormat::Hast] {
        let json = r.to_json(f);
        out.push(channel(format!("{m}:json.{f:?}"), Kind::Meta, pd, json));
    }
    for n in &r.nodes {
        push_node(out, n, &format!("{m}:node[{}]", n.id), pd);
    }
}

fn push_node(out: &mut Vec<Channel>, n: &NodeMeta, at: &str, pd: bool) {
    let mut trees: Vec<(String, &Vec<Node>)> = vec![
        ("title".into(), &n.title),
        ("title_unlinked".into(), &n.title_unlinked),
    ];
    for (i, a) in n.aliases.iter().enumerate() {
        trees.push((format!("aliases[{i}]"), a));
    }
    for (k, v) in &n.properties {
        trees.push((format!("properties[{k}]"), v));
    }
    for (field, t) in trees {
        out.push(channel(
            format!("{at}.{field}.html"),
            Kind::Meta,
            pd,
            to_html(t),
        ));
        out.push(channel(
            format!("{at}.{field}.hast"),
            Kind::Meta,
            pd,
            to_hast(t),
        ));
    }
    let keys: Vec<&str> = n.properties.iter().map(|(k, _)| k.as_str()).collect();
    out.push(channel(
        format!("{at}.property-keys"),
        Kind::Meta,
        pd,
        keys.join(" "),
    ));
    let anchor = n.anchor.clone().unwrap_or_default();
    out.push(channel(format!("{at}.anchor"), Kind::Meta, pd, anchor));
}
