//! Inline objects: the properties the goldens cannot show — no recursion on
//! nesting, the nesting cap, and the cases where Org itself fails — plus the
//! typed link view on inputs chosen to break it. Cases marked "oracle" were
//! confirmed against Emacs org-element (Org 9.7.11).

mod support;

use mae_org::{parse, Link, LinkFormat, ParseOptions, SyntaxKind, SyntaxKind::*, SyntaxNode};
use support::*;

fn count(doc: &mae_org::Document, kind: SyntaxKind) -> usize {
    doc.syntax()
        .descendants()
        .filter(|n| n.kind() == kind)
        .count()
}

fn max_depth(root: &SyntaxNode) -> usize {
    root.descendants()
        .map(|n| n.ancestors().count())
        .max()
        .unwrap_or(0)
}

/// `*a *a … z***…`: each closer is the last byte of its region, so every
/// level nests (oracle). `[fn:: a [fn:: …]]` nests through bracket pairing.
fn deep_inputs(n: usize) -> Vec<String> {
    vec![
        format!("{}z{}\n", "*a ".repeat(n), "*".repeat(n)),
        format!("{}x{}\n", "[fn:: a ".repeat(n), "]".repeat(n)),
    ]
}

/// Oracle (Emacs 30.2, `* h http:x^2\r\nb\n* t2 http:a\r :tag:\n`, decoded
/// unix): a headline title's objects run to the last char that is not a
/// space or tab, so a content CR ends a plain link there; `:raw-value` is
/// `org-trim`med. Pinned here rather than in a corpus fixture because
/// semantics() still drops that CR from the link's span (its corpus walk
/// asserts the span holds the path).
#[test]
fn a_content_cr_ends_a_plain_link_in_a_headline_title() {
    let text = "* h http:x^2\r\nb\n* t2 http:a\r :tag:\n";
    let doc = parse(text, &ParseOptions::default());
    let spans: Vec<(usize, usize)> = mae_elements(&doc)
        .into_iter()
        .filter(|e| e.kind == "link")
        .map(|e| (e.begin, e.end))
        .collect();
    assert_eq!(spans, [(4, 13), (21, 28)]);
    let titles: Vec<String> = doc
        .syntax()
        .descendants()
        .filter_map(mae_org::Headline::cast)
        .map(|h| h.title())
        .collect();
    assert_eq!(titles, ["h http:x^2", "t2 http:a"]);
    let paths: Vec<String> = links(text)
        .iter()
        .map(|l| l.info(&ParseOptions::default().link_types).path)
        .collect();
    assert_eq!(paths, ["x^2\r", "a\r"]);
}

#[test]
fn objects_nested_5000_deep_parse_on_a_small_stack_within_the_cap() {
    for text in deep_inputs(5_000) {
        let ok = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || {
                let opts = ParseOptions {
                    max_depth: usize::MAX,
                    ..ParseOptions::default()
                };
                let doc = parse(&text, &opts);
                assert_eq!(doc.syntax().to_string(), text);
                assert!(doc == parse(&text, &opts));
                check_tree(&doc, &text, mae_org::MAX_DEPTH_CEILING).unwrap();
                let depth = max_depth(&doc.syntax());
                assert!(depth <= mae_org::MAX_DEPTH_CEILING + 2, "depth {depth}");
                // The cap bounds the objects, it does not drop them all:
                // about a thousand levels are objects, the rest is text.
                let objects = count(&doc, BOLD) + count(&doc, FOOTNOTE_REFERENCE);
                assert!((1000..5000).contains(&objects), "{objects} objects");
                drop(doc);
            })
            .expect("thread")
            .join();
        assert!(ok.is_ok(), "deep objects failed on a 1 MiB stack");
    }
}

#[test]
fn objects_past_the_default_cap_are_plain_text() {
    for text in deep_inputs(300) {
        let doc = parse_default(&text);
        check_tree(&doc, &text, mae_org::DEFAULT_MAX_DEPTH).unwrap();
        let objects = count(&doc, BOLD) + count(&doc, FOOTNOTE_REFERENCE);
        assert!(
            (100..mae_org::DEFAULT_MAX_DEPTH).contains(&objects),
            "{objects} objects under the default cap"
        );
        // Below the cap the nesting is exact: each object is inside the last.
        let deepest = doc
            .syntax()
            .descendants()
            .filter(|n| matches!(n.kind(), BOLD | FOOTNOTE_REFERENCE))
            .map(|n| n.ancestors().count())
            .max()
            .unwrap_or(0);
        assert!(deepest >= objects, "objects are not nested");
    }
}

/// `<2026-3-4 x +1d>` matches `org-element--timestamp-regexp` (its repeater
/// alternative allows any digit counts), but `org-parse-time-string` then
/// signals "Not an Org time string" and Org's parse of the whole buffer
/// fails (oracle). mae-org keeps the extent the regexp gives.
#[test]
fn a_timestamp_org_cannot_read_keeps_the_extent_its_regexp_gives() {
    let text = "A <2026-3-4 x +1d> b <1-2-3+1d> c <2026-3-4 x+1d c\n";
    let doc = parse_default(text);
    let ts: Vec<String> = doc
        .syntax()
        .descendants()
        .filter(|n| n.kind() == TIMESTAMP)
        .map(|n| n.to_string())
        .collect();
    // The second needs a char between the third number and `+`; the third
    // has no `>`.
    assert_eq!(ts, ["<2026-3-4 x +1d> "]);
}

fn links(text: &str) -> Vec<Link> {
    parse_default(text)
        .syntax()
        .descendants()
        .filter_map(Link::cast)
        .collect()
}

fn info(text: &str) -> Vec<(String, String, String, &'static str)> {
    let types = ParseOptions::default().link_types;
    links(text)
        .iter()
        .map(|l| {
            let i = l.info(&types);
            (i.link_type, i.path, i.raw_link, i.format.as_str())
        })
        .collect()
}

#[test]
fn link_view_resolves_file_links_like_org() {
    let text = "[[file+sys:a.org::*h]] [[FILE:///c:/x]] [[//net]] [[file:////a]]\n";
    let types = ParseOptions::default().link_types;
    let got: Vec<_> = links(text)
        .iter()
        .map(|l| {
            let i = l.info(&types);
            (i.link_type, i.path, i.search_option, i.application)
        })
        .collect();
    let s = |x: &str| x.to_string();
    assert_eq!(
        got,
        [
            (s("file"), s("a.org"), Some(s("*h")), Some(s("sys"))),
            // `\`///*\(.:\)?/` folds three slashes and a drive (oracle).
            (s("file"), s("c:/x"), None, None),
            // Two slashes are not folded: `//net` stays (oracle: `//%f`).
            (s("file"), s("//net"), None, None),
            (s("file"), s("/a"), None, None),
        ]
    );
}

#[test]
fn link_view_unescapes_and_joins_lines_in_bracket_links_only() {
    let text = "[[a\\]b\\\\]] [[x  \n  y]] <https://a.org/p\n  q> [[(ref)]] [[#id]] [[()]]\n";
    let s = |x: &str| x.to_string();
    assert_eq!(
        info(text),
        [
            (s("fuzzy"), s("a]b\\"), s("a]b\\"), "bracket"),
            (s("fuzzy"), s("x y"), s("x y"), "bracket"),
            // Angle links drop the line break and its blanks (RFC 3986).
            (
                s("https"),
                s("//a.org/pq"),
                s("https://a.org/p\n  q"),
                "angle"
            ),
            (s("coderef"), s("ref"), s("(ref)"), "bracket"),
            (s("custom-id"), s("id"), s("#id"), "bracket"),
            (s("coderef"), s(""), s("()"), "bracket"),
        ]
    );
}

#[test]
fn link_view_reads_crlf_line_endings_as_one_newline() {
    let lf = "[[x\n y]] <https://a.org/p\n q> <<<r s>>> r\ns\n";
    let crlf = to_crlf(lf);
    assert_eq!(info(lf), info(&crlf));
    let radio: Vec<_> = links(&crlf).into_iter().filter(Link::is_radio).collect();
    assert_eq!(radio.len(), 1);
    assert_eq!(radio[0].format(), LinkFormat::Plain);
    let types = ParseOptions::default().link_types;
    assert_eq!(radio[0].info(&types).path, "r\ns");
}

#[test]
fn the_default_link_types_are_the_oracles_after_org_mode_starts() {
    // `id:`, `info:` and `doi:` are plain links once `org-mode` has run
    // (oracle); before it they would be text.
    let text = "id:4F2A info:org#Links doi:10.1/x nosuchtype:x\n";
    let got: Vec<String> = links(text).iter().map(|l| l.syntax().to_string()).collect();
    assert_eq!(got, ["id:4F2A ", "info:org#Links ", "doi:10.1/x "]);
    // A plain link cannot end in `-`: `[^[:punct:] \t\n]` (oracle).
    let got: Vec<String> = links("see https://a.org/x-. now\n")
        .iter()
        .map(|l| l.syntax().to_string())
        .collect();
    assert_eq!(got, ["https://a.org/x"]);
}
