//! The render divergence register (`conformance/render-divergences.toml`):
//! every deliberate difference between the renderers and Org's own HTML
//! export (`org-html-export-as-html` in `emacs -Q`), each with its reason
//! and the test that pins MAE's side. This file checks the register (every
//! entry complete, ids unique, every pinning test a `#[test]` function of
//! this crate) and holds the pins no other test file does.

mod support;

use mae_org::{
    parse, render_html, semantics, semantics_with, ArchivedTrees, ParseOptions, RenderOptions,
    Resolver, SemanticsOptions,
};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

struct Entry {
    line: usize,
    fields: BTreeMap<String, String>,
}

/// `[[divergence]]` tables of `key = "string"` lines: a tiny subset of
/// TOML, so no dependency; anything else is an error.
fn register() -> Vec<Entry> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/render-divergences.toml");
    let text = support::read(&path);
    let mut out: Vec<Entry> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[divergence]]" {
            out.push(Entry {
                line: n + 1,
                fields: BTreeMap::new(),
            });
            continue;
        }
        let (k, v) = line
            .split_once(" = ")
            .unwrap_or_else(|| panic!("render-divergences.toml:{}: not `key = \"value\"`", n + 1));
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or_else(|| panic!("render-divergences.toml:{}: value not quoted", n + 1));
        let e = out
            .last_mut()
            .unwrap_or_else(|| panic!("render-divergences.toml:{}: key outside an entry", n + 1));
        assert!(
            e.fields.insert(k.to_string(), v.to_string()).is_none(),
            "render-divergences.toml:{}: {k} twice",
            n + 1
        );
    }
    out
}

/// The `#[test]` functions defined in `source` (proptest's included).
fn tests_in(source: &str, out: &mut HashSet<String>) {
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    for (i, l) in lines.iter().enumerate() {
        let Some(rest) = l.strip_prefix("fn ") else {
            continue;
        };
        let name = rest.split(['(', '<']).next().unwrap_or("");
        let mut attrs = lines[..i]
            .iter()
            .rev()
            .take_while(|p| p.starts_with("#[") || p.starts_with("//"));
        if attrs.any(|p| *p == "#[test]") {
            out.insert(name.to_string());
        }
    }
}

/// Every `.rs` file under `dir`.
fn rust_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out
}

/// Every `#[test]` function in `src/` and `tests/`.
fn test_functions() -> HashSet<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut names = HashSet::new();
    for dir in ["src", "tests"] {
        for f in rust_files(&root.join(dir)) {
            tests_in(&support::read(&f), &mut names);
        }
    }
    names
}

#[test]
fn the_render_register_is_complete_and_pinned() {
    let entries = register();
    assert!(entries.len() >= 15, "only {} entries", entries.len());
    let tests = test_functions();
    let mut ids = HashSet::new();
    for e in &entries {
        for k in ["id", "what", "org", "mae", "reason", "test"] {
            let v = e.fields.get(k).map(String::as_str).unwrap_or("");
            assert!(
                !v.is_empty(),
                "render-divergences.toml:{}: no `{k}`",
                e.line
            );
        }
        let known = ["id", "what", "org", "mae", "reason", "test"];
        for k in e.fields.keys() {
            assert!(
                known.contains(&k.as_str()),
                "render-divergences.toml:{}: unknown key {k}",
                e.line
            );
        }
        assert!(
            ids.insert(e.fields["id"].clone()),
            "render-divergences.toml:{}: id repeated",
            e.line
        );
        let test = &e.fields["test"];
        assert!(
            tests.contains(test),
            "render-divergences.toml:{}: pinning test `{test}` is not a #[test] function",
            e.line
        );
    }
}

fn html(doc: &str) -> String {
    let d = parse(doc, &ParseOptions::default());
    render_html(
        &d,
        &semantics(&d),
        &RenderOptions::default(),
        &Resolver::default(),
    )
}

#[test]
fn drawers_other_than_properties_are_never_rendered() {
    // Org's default (`org-export-with-drawers` = (not "LOGBOOK")) exports
    // NOTES; ADR-112 D2 renders no drawer but the property drawer.
    let out = html("* H\n:NOTES:\nNOTEBODY\n:END:\n:LOGBOOK:\nLOGBODY\n:END:\nText.\n");
    assert!(
        !out.contains("NOTEBODY") && !out.contains("LOGBODY"),
        "{out}"
    );
    assert!(out.contains("Text."), "{out}");
}

#[test]
fn options_keywords_are_not_read() {
    // `#+OPTIONS:` is an exporter's setting: Org would hide the TODO
    // keyword and the tags, and read `a_b` literally.
    let out = html("#+OPTIONS: todo:nil tags:nil ^:nil toc:t num:t\n* TODO Head :tag:\nx a_b\n");
    assert!(out.contains("org-todo") && out.contains("org-tag"), "{out}");
    assert!(out.contains("<sub>b</sub>"), "{out}");
    assert!(
        !out.contains("table-of-contents") && !out.contains("1. "),
        "{out}"
    );
}

#[test]
fn latex_timestamps_macros_and_citations_render_as_their_source() {
    let out = html(
        "\\(x^2\\) <2026-01-01 Thu> {{{title}}} [cite:@k]\n\\begin{equation}\ny\n\\end{equation}\n",
    );
    for want in [
        "<code class=\"org-latex\">\\(x^2\\)</code>",
        "<time class=\"org-timestamp\">&lt;2026-01-01 Thu&gt;</time>",
        "<span class=\"org-macro\">{{{title}}}</span>",
        "<span class=\"org-citation\">[cite:@k]</span>",
        "<pre class=\"org-latex-environment\"><code>\\begin{equation}",
    ] {
        assert!(out.contains(want), "{want} missing: {out}");
    }
}

#[test]
fn a_paragraph_caption_is_a_figure() {
    // Org's HTML ignores a caption on a paragraph that is not an image.
    let out = html("#+CAPTION: Cap\nA paragraph.\n");
    assert!(
        out.contains(
            "<figure class=\"org-figure\"><p>A paragraph.</p><figcaption>Cap</figcaption></figure>"
        ),
        "{out}"
    );
}

#[test]
fn file_links_and_local_images_are_not_links() {
    // Org links `file:x.org` to x.html and inlines `./img.png`.
    let d = parse(
        "[[file:x.org][a file]] [[./img.png]] [[https://h/i.png]]\n",
        &ParseOptions::default(),
    );
    let sem = semantics(&d);
    let images = RenderOptions::default().with_images(true);
    let out = render_html(&d, &sem, &images, &Resolver::default());
    assert!(!out.contains("x.org") && !out.contains("x.html"), "{out}");
    assert!(!out.contains("./img.png\""), "{out}");
    assert!(out.contains("<img src=\"https://h/i.png\""), "{out}");
    let off = render_html(&d, &sem, &RenderOptions::default(), &Resolver::default());
    assert!(!off.contains("<img"), "{off}");
}

#[test]
fn export_blocks_snippets_and_html_keywords_never_render() {
    let out = html("#+HTML: <b>KW</b>\n#+begin_export html\n<b>BLOCK</b>\n#+end_export\nx @@html:<b>SNIP</b>@@\n");
    for raw in ["KW", "BLOCK", "SNIP", "<b>"] {
        assert!(!out.contains(raw), "{raw} rendered: {out}");
    }
}

#[test]
fn archived_blocks_follow_their_exports() {
    // Org's babel pass skips archived subtrees, so with archived trees
    // exported whole their blocks are exported as written.
    let doc = "* A :ARCHIVE:\n#+begin_src sh :exports none\nARCHSECRET\n#+end_src\n#+begin_src sh\nARCHSHOWN\n#+end_src\n";
    let d = parse(doc, &ParseOptions::default());
    let opts = SemanticsOptions::default().with_archived_trees(ArchivedTrees::All);
    let out = render_html(
        &d,
        &semantics_with(&d, &opts),
        &RenderOptions::default(),
        &Resolver::default(),
    );
    assert!(!out.contains("ARCHSECRET"), "{out}");
    assert!(out.contains("ARCHSHOWN"), "{out}");
}
