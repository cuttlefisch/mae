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

/// Mixed radio targets (a tab next to a space) are matched by backtracking
/// within a work budget proportional to the input. A small document stays
/// well inside it and makes Org's links; nested targets over a long text
/// exhaust it, which is reported (`RadioLinkBudget`) rather than paid for.
#[test]
fn mixed_radio_targets_work_within_a_budget_and_report_when_it_runs_out() {
    let small = "<<<a\t b>>>\n\nx a\t  b y\n";
    let doc = parse(small, &ParseOptions::default());
    assert_eq!(count(&doc, LINK), 1);
    assert!(doc.diagnostics().is_empty(), "{:?}", doc.diagnostics());
    // Every target's run-collapsed shape matches at every word, and each
    // fails only at its last stretch (` \t` against the text's `\t `): the
    // backtracking work per word grows with the targets' total length.
    let targets: String = (1..=120)
        .map(|j| format!("<<<{}a \ta>>>\n", "a\t ".repeat(j)))
        .collect();
    let big = format!("{targets}\n{}\n", "a\t ".repeat(40_000));
    let doc = parse(&big, &ParseOptions::default());
    assert!(doc
        .diagnostics()
        .iter()
        .any(|d| d.kind == mae_org::DiagnosticKind::RadioLinkBudget));
    assert_eq!(doc.syntax().to_string(), big);
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
        // The cap is reported where Org would still find objects.
        let limits: Vec<_> = doc
            .diagnostics()
            .iter()
            .filter(|d| d.kind == mae_org::DiagnosticKind::DepthLimit)
            .collect();
        assert_eq!(limits.len(), 1, "{:?}", doc.diagnostics());
        let span = limits[0].span;
        assert!(text[span.range()].contains("a"), "{span:?}");
    }
    // Under the cap, and past it with only text left, nothing is reported.
    for text in deep_inputs(50) {
        assert!(parse_default(&text).diagnostics().is_empty());
    }
    let text = format!("{}z{}\n", "*a ".repeat(300), "*".repeat(300));
    let opts = ParseOptions {
        max_depth: 400,
        ..ParseOptions::default()
    };
    assert!(parse(&text, &opts).diagnostics().is_empty());
}

/// A radio target nested past the cap is not seen (Org finds targets at any
/// depth), so its links are missing; the DepthLimit diagnostic says where.
#[test]
fn a_radio_target_past_the_cap_is_reported() {
    let deep = format!(
        "{}<<<rt>>>{}\n\nrt\n",
        "[fn:: a ".repeat(200),
        "]".repeat(200)
    );
    let doc = parse_default(&deep);
    assert_eq!(count(&doc, RADIO_TARGET), 0);
    assert_eq!(count(&doc, LINK), 0);
    let at = deep.find("<<<").unwrap();
    assert!(doc
        .diagnostics()
        .iter()
        .any(|d| d.kind == mae_org::DiagnosticKind::DepthLimit && d.span.range().contains(&at)));
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

/// `(kind, begin, end)` of every object mae-org finds in a fixture.
fn objects_of(fixture: &str) -> Vec<(String, usize, usize)> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    let text = read(&path);
    let doc = parse_default(&text);
    check_tree(&doc, &text, mae_org::DEFAULT_MAX_DEPTH).unwrap();
    assert!(doc.diagnostics().is_empty(), "{:?}", doc.diagnostics());
    let spans: Vec<(usize, usize)> = doc
        .syntax()
        .descendants()
        .filter(|n| is_object(n.kind()))
        .map(|n| {
            let r = n.text_range();
            (usize::from(r.start()), usize::from(r.end()))
        })
        .collect();
    mae_elements(&doc)
        .into_iter()
        .filter(|e| spans.contains(&(e.begin, e.end)) && !e.kind.contains("paragraph"))
        .map(|e| (e.kind, e.begin, e.end))
        .collect()
}

fn o(kind: &str, begin: usize, end: usize) -> (String, usize, usize) {
    (kind.to_string(), begin, end)
}

/// Emacs never finishes these (it allocates until killed; regen.sh records
/// them as "did not terminate"). In each, a radio link `x` is followed by a
/// `[`: the object lexer, one char past the link, backs up to the line start
/// to match the regexp's `^` and gets back an EMPTY radio link at point,
/// which it parses again forever. org-element's intended rule is that the
/// radio branch finds a link STARTING at point (it backs up one char only
/// so the bordering char can match `[^[:alnum:]]`); a match beginning
/// before point is not a link at point. So mae-org makes the radio link
/// `x`, then parses the `[` as anything else would be parsed there: a
/// bracket link `[[x]]` in the first file, nothing in the others.
#[test]
fn emacs_does_not_terminate_on_a_radio_link_before_a_bracket() {
    assert_eq!(
        objects_of("objects-emacs-nonterminating.org"),
        [o("radio-target", 0, 7), o("link", 9, 10), o("link", 10, 15)]
    );
    assert_eq!(
        objects_of("objects-emacs-nonterminating-tag.org"),
        [o("radio-target", 0, 7), o("link", 18, 19)]
    );
    assert_eq!(
        objects_of("objects-emacs-nonterminating-verse.org"),
        [
            o("radio-target", 0, 7),
            o("link", 23, 24),
            o("subscript", 43, 45),
            o("statistics-cookie", 56, 59),
            o("entity", 62, 65),
            o("statistics-cookie", 86, 89)
        ]
    );
    // Linear on the shape repeated (the scaling gate times it in release).
    let text = format!("<<<x>>>\n\n{}\n", "x[[x]] x[[/^\n".repeat(2_000));
    let doc = parse_default(&text);
    check_tree(&doc, &text, mae_org::DEFAULT_MAX_DEPTH).unwrap();
    assert_eq!(count(&doc, LINK), 6_000);
}

/// Targets `a b` and `b-c` over `a\nb-c`: the first link is `a\nb`, ending
/// one char into the line. The next lex, from there, backs up to the line
/// start, matches `b-c` with `^`, and Emacs returns a link that BEGINS
/// before point — two overlapping objects (oracle: links 21..24 and
/// 23..26). A tree cannot hold objects that overlap, and the second starts
/// inside the first, so mae-org keeps the first only.
#[test]
fn a_radio_link_never_overlaps_the_object_before_it() {
    assert_eq!(
        objects_of("objects-radio-overlap.org"),
        [
            o("radio-target", 0, 10),
            o("radio-target", 10, 19),
            o("link", 21, 24)
        ]
    );
}

/// Org splits its radio regexp into several above
/// `org-target-link-regexp-limit` (8192 chars) and then takes, among the
/// regexps' matches, the one that ENDS first (`org--re-list-search-forward`)
/// — not the leftmost match of one alternation. In the fixture the split
/// falls between `foo y… bar zz` and `bar`, so Emacs links `bar` (8277..8281)
/// instead of the long target. The limit is a constant of Org's
/// implementation (a regexp-engine size limit), not a property of the
/// document or a declared setting (ADR-112 D4), so mae-org matches as one
/// regexp at any size: the long target, as Emacs does below the limit.
#[test]
fn radio_targets_match_as_one_regexp_however_many_there_are() {
    let objects = objects_of("objects-radio-split-regexp.org");
    assert_eq!(objects.last(), Some(&o("link", 8172, 8284)));
    assert_eq!(objects.iter().filter(|x| x.0 == "link").count(), 1);
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
