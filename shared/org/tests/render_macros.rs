//! Macros are expanded as Org's export expands them, and what renders is
//! the parse of the expanded text (ADR-112 D3, review round 4). Each
//! document's expectation is Emacs 30.2's (`emacs -Q`, ox-html body only,
//! babel never evaluated): `tests/render_macros_oracle.rs` checks a larger
//! generated corpus against Emacs itself.

use mae_org::{
    node_text, parse, render, semantics, text, HeadlineIds, MacroFailure, ParseOptions,
    RenderDiagnostic, RenderOptions, Resolved, Resolver, TreeFormat,
};

/// Every output of `doc` (each option set's hast and HTML with the node
/// metadata, the text, every node's text), one string per surface.
fn outputs(doc: &str) -> Vec<(String, String)> {
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    let r = Resolver::default();
    let mut out = Vec::new();
    let option_sets = [
        RenderOptions::default(),
        RenderOptions::default()
            .with_property_drawers(true)
            .with_images(true),
        RenderOptions::default().with_headline_ids(HeadlineIds::Slug),
    ];
    for (i, opts) in option_sets.iter().enumerate() {
        let rendered = render(&d, &sem, opts, &r);
        out.push((format!("hast/{i}"), rendered.to_json(TreeFormat::Hast)));
        out.push((format!("html/{i}"), rendered.to_json(TreeFormat::Html)));
    }
    out.push(("text".into(), text(&d, &sem)));
    for n in &sem.nodes {
        let t = node_text(&d, &sem, &n.id).unwrap_or_default();
        out.push((format!("node_text {}", n.id), t));
    }
    out
}

fn html(doc: &str) -> String {
    let d = parse(doc, &ParseOptions::default());
    mae_org::render_html(
        &d,
        &semantics(&d),
        &RenderOptions::default(),
        &Resolver::default(),
    )
}

/// The third reviewer's inputs (round 4), as written, and Emacs's export
/// of each: (name, document, what Emacs hides, what it shows).
const REVIEWER: &[(&str, &str, &[&str], &[&str])] = &[
    (
        "m1: a template closes an inline src block opened before the macro",
        "#+MACRO: close $1}\n\nsrc_sh[:exports none]{echo {{{close(SECRET)}}}\n",
        &["SECRET", "echo"],
        &[],
    ),
    (
        "m2: a template closes an export snippet",
        "#+MACRO: at $1@\n\n@@latex:{{{at(SECRET)}}}@\n",
        &["SECRET"],
        &[],
    ),
    (
        "m3: $0 is the first argument",
        "#+MACRO: j $0$2\n\n{{{j(src,_sh[:exports none]{echo SECRET})}}}\n",
        &["SECRET"],
        &[],
    ),
    (
        "ctl3: $2 alone",
        "#+MACRO: j $2\n\n{{{j(src,_sh[:exports none]{echo SECRET})}}}\n",
        &[],
        &["_sh[:exports none]{echo SECRET}"],
    ),
    (
        "m4: an expansion makes a comment line",
        "#+MACRO: id $1\n\nPara one.\n{{{id(#)}}} SECRET\n",
        &["SECRET"],
        &["Para one."],
    ),
    (
        "m5: an expansion makes a babel call",
        "#+MACRO: id $1\n\n#+NAME: fn\n#+begin_src sh :var x=\"\" :exports none\necho\n#+end_src\n\nPara.\n\n{{{id(#+CALL: fn(SECRET))}}}\n",
        &["SECRET"],
        &["Para."],
    ),
    (
        "m6: a keyword's value opens an inline src block",
        "#+X: src_sh[:exports none]{echo\n\n{{{keyword(X)}}} SECRET}\n",
        &["SECRET"],
        &[],
    ),
    (
        "m7: a template closes an inline babel call",
        "#+MACRO: paren $1)\n\n#+NAME: fn\n#+begin_src sh :var x=\"\" :exports none\necho\n#+end_src\n\nPara call_fn({{{paren(SECRET)}}}\n",
        &["SECRET"],
        &["Para"],
    ),
    (
        "m8: an expansion opens a comment block",
        "#+MACRO: id $1\n\n{{{id(#+begin_comment)}}}\nSECRET\n#+end_comment\n",
        &["SECRET"],
        &[],
    ),
    (
        "m9: a property's value opens an inline src block",
        "* H\n:PROPERTIES:\n:P: src_sh[:exports none]{echo\n:END:\n{{{property(P)}}} SECRET}\n",
        &["SECRET"],
        &[">H<"],
    ),
    (
        "m10: an expansion makes a COMMENT subtree",
        "#+MACRO: id $1\n\n* {{{id(COMMENT)}}} Visible\nSECRET body\n",
        &["SECRET", "Visible"],
        &[],
    ),
    (
        "m11: an expansion adds a :noexport: tag",
        "#+MACRO: id $1\n\n* Heading {{{id(:noexport:)}}}\nSECRET body\n",
        &["SECRET", "Heading"],
        &[],
    ),
    (
        "m12: a whole comment line",
        "#+MACRO: id $1\n\nPara.\n{{{id(# SECRET)}}}\n",
        &["SECRET"],
        &["Para."],
    ),
    (
        "d1: a #+MACRO in a COMMENT subtree is not collected",
        "Text {{{m(SECRET)}}} here.\n\n* COMMENT old\n#+MACRO: m safe $1\n\n* Real\n#+MACRO: m src_sh[:exports none]{echo $1}\n",
        &["SECRET", "safe"],
        &["Text", "here."],
    ),
    (
        "d2: the first definition wins",
        "Text {{{m(SECRET)}}} here.\n\n#+MACRO: m safe $1\n#+MACRO: m src_sh[:exports none]{echo $1}\n",
        &["echo"],
        &["Text safe SECRET here."],
    ),
    (
        "d3: names compared in any case",
        "Text {{{m(SECRET)}}} here.\n\n#+MACRO: M src_sh[:exports none]{echo $1}\n#+MACRO: m safe $1\n",
        &["SECRET", "safe"],
        &["Text"],
    ),
    (
        "d4: names compared in any case, the plain one first",
        "Text {{{m(SECRET)}}} here.\n\n#+MACRO: m safe $1\n#+MACRO: M src_sh[:exports none]{echo $1}\n",
        &["echo"],
        &["Text safe SECRET here."],
    ),
    (
        "d5: #+TITLE overrides a title macro",
        "#+TITLE: plain\n#+MACRO: title src_sh[:exports none]{echo $1}\n\nText {{{title(SECRET)}}} here.\n",
        &["SECRET"],
        &["Text plain here."],
    ),
    (
        "d6: #+TITLE overrides a title macro defined first",
        "#+MACRO: title src_sh[:exports none]{echo $1}\n#+TITLE: plain\n\nText {{{title(SECRET)}}} here.\n",
        &["SECRET"],
        &["Text plain here."],
    ),
    (
        "d7: a template's backslash-n is two characters",
        "#+MACRO: m safe $1 \\n src_sh[:exports none]{echo x}\n\nText {{{m(SECRET)}}} here.\n",
        &["echo x"],
        &["Text safe SECRET", "\\n"],
    ),
    (
        "r1 (m1 of the review): a color macro's snippets go, its text shows",
        "#+MACRO: color @@html:<span style=\"color:$1\">@@$2@@html:</span>@@\n\nSee {{{color(red,VISIBLE)}}} now.\n",
        &["color:", "red", "span style"],
        &["See VISIBLE now."],
    ),
    (
        "u2: an upper-case SRC_ is no construct",
        "#+MACRO: id $1\n\nPara {{{id(SRC_sh[:exports none]{echo SECRET})}}} end.\n",
        &[],
        &["SECRET"],
    ),
    (
        "u3: an export snippet's backend in any case",
        "Para @@LaTeX:SECRET@@ end.\n",
        &["SECRET"],
        &["Para"],
    ),
];

#[test]
fn the_reviewers_macro_inputs_render_what_emacs_exports() {
    let mut failures = Vec::new();
    for (name, doc, hidden, shown) in REVIEWER {
        for (surface, s) in outputs(doc) {
            for h in *hidden {
                if s.contains(h) {
                    failures.push(format!("{name}: {h:?} in {surface}:\n{s}"));
                }
            }
        }
        let h = html(doc);
        for want in *shown {
            if !h.contains(want) {
                failures.push(format!("{name}: {want:?} not shown:\n{h}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// m13: an expansion makes a property drawer, which Org does not export
/// (MAE renders drawers only when asked).
#[test]
fn an_expansion_can_make_a_property_drawer() {
    let doc = "#+MACRO: id $1\n\n* Visible\n{{{id(:PROPERTIES:)}}}\n:SECRET: x\n:END:\n";
    let h = html(doc);
    assert!(h.contains(">Visible<") && !h.contains("SECRET"), "{h}");
}

/// The file's aliases keyword is cut as the property is (review B6): a
/// split at a comma inside a construct the whole value hides shows none.
#[test]
fn an_aliases_keyword_is_cut_as_the_property_is() {
    for doc in [
        ":PROPERTIES:\n:ID: f\n:END:\n#+aliases: src_sh[:exports none]{echo a, SECRET}\n#+title: T\n\nbody\n",
        ":PROPERTIES:\n:ID: f\n:ALIASES: src_sh[:exports none]{echo a, SECRET}\n:END:\n#+title: T\n\nbody\n",
        "* H\n:PROPERTIES:\n:ID: h\n:ALIASES: x\n:ALIASES+: src_sh[:exports none]{echo a, SECRET}\n:END:\nbody\n",
        "#+MACRO: m $1\n:PROPERTIES:\n:ID: f\n:END:\n#+aliases: {{{m(a, SECRET)}}}\n#+title: T\n\nbody\n",
    ] {
        for (surface, s) in outputs(doc) {
            assert!(!s.contains("SECRET") && !s.contains("echo"), "{surface}:\n{doc}\n{s}");
        }
    }
}

fn rendered(doc: &str) -> mae_org::Rendered {
    let d = parse(doc, &ParseOptions::default());
    render(
        &d,
        &semantics(&d),
        &RenderOptions::default(),
        &Resolver::default(),
    )
}

/// Where Org's export stops with an error, nothing renders: no body, no
/// node metadata, one diagnostic at the macro (in the source).
#[test]
fn a_macro_org_cannot_expand_renders_nothing() {
    let cases: &[(&str, &str, MacroFailure)] = &[
        (
            "Before.\n\nText {{{nosuch(x)}}} here.\n",
            "{{{nosuch(x)}}}",
            MacroFailure::Undefined,
        ),
        (
            "#+MACRO: a {{{a}}}\nA {{{a}}} B\n",
            "{{{a}}}",
            MacroFailure::Circular,
        ),
        (
            "#+MACRO: a {{{b}}}\n#+MACRO: b {{{a}}}\nA {{{a}}} B\n",
            "{{{a}}}",
            MacroFailure::Circular,
        ),
        (
            "#+MACRO: e (eval (+ 1 2))\nA {{{e}}} B\n",
            "{{{e}}}",
            MacroFailure::Eval,
        ),
        (
            "* H\nA {{{property(P,other)}}} B\n",
            "{{{property(P,other)}}}",
            MacroFailure::Unsupported,
        ),
        (
            "* H\nA {{{property(ALLTAGS)}}} B\n",
            "{{{property(ALLTAGS)}}}",
            MacroFailure::Unsupported,
        ),
        (
            "A {{{keyword}}} B\n",
            "{{{keyword}}}",
            MacroFailure::Unsupported,
        ),
        (
            "A {{{time(%c)}}} B\n",
            "{{{time(%c)}}}",
            MacroFailure::Unsupported,
        ),
        (
            "#+DATE: <2024-01-02 Tue>\nA {{{date(%Y)}}} B\n",
            "{{{date(%Y)}}}",
            MacroFailure::Unsupported,
        ),
    ];
    for (doc, at, want) in cases {
        let d = parse(doc, &ParseOptions::default());
        let sem = semantics(&d);
        let r = render(&d, &sem, &RenderOptions::default(), &Resolver::default());
        assert!(r.body.is_empty() && r.nodes.is_empty(), "{doc}: {r:?}");
        let start = doc.rfind(at).unwrap();
        let span = mae_org::Span {
            start: start as u32,
            end: (start + at.len()) as u32,
        };
        assert_eq!(
            r.diagnostics,
            [RenderDiagnostic::MacroFailed(*want, span)],
            "{doc}"
        );
        assert_eq!(text(&d, &sem), "", "{doc}");
    }
    // Not a cycle to Org: each expansion stands one place further, until
    // the text around makes the next one a target (Emacs exports one).
    let h = html("#+MACRO: a <{{{b}}}>\n#+MACRO: b <{{{a}}}>\nA {{{a}}} B\n");
    assert!(h.contains("<p>A <span class=\"org-target\""), "{h}");
    // An unused (eval …) template or undefined name is no failure, and a
    // macro under a COMMENT headline is not expanded at all.
    let r = rendered("#+MACRO: e (eval (+ 1 2))\n* COMMENT x\n{{{nosuch}}}\n* Y\nok\n");
    assert!(r.diagnostics.is_empty(), "{r:?}");
    assert!(mae_org::to_html(&r.body).contains("ok"));
}

/// The built-ins, as Emacs 30.2 expands them.
#[test]
fn built_in_macros_expand_as_org_expands_them() {
    let cases = [
        // `n`: counters in document order, named, set, held, reset.
        (
            "#+MACRO: sec S{{{n}}}\nA {{{n}}} {{{sec}}} {{{n}}} {{{n(x)}}} {{{n(x)}}} {{{n(x,10)}}} {{{n(x,-)}}} {{{n(x)}}} {{{n(x,reset)}}} {{{n(y,-)}}}\n",
            "<p>A 1 S2 3 1 2 10 10 11 1 1</p>",
        ),
        // `keyword`: every line of the name, joined; in any case.
        (
            "#+X: one\n#+x: two\nA {{{keyword(X)}}} B {{{keyword(nosuch)}}} C\n",
            "<p>A one two B  C</p>",
        ),
        // `title`, `author`, `email`, `date`; an empty one with no keyword.
        (
            "#+TITLE: T1\n#+TITLE: T2\n#+AUTHOR: Me\n#+DATE: today\nA {{{title}}}|{{{author}}}|{{{email}}}|{{{date}}} B\n",
            "<p>A T1 T2|Me||today B</p>",
        ),
        // `property`: the entry's own, `+` appended, not inherited.
        (
            "* H\n:PROPERTIES:\n:P: a\n:P+: b\n:END:\nA {{{property(P)}}} B\n** I\nC {{{property(P)}}} D\n",
            "<p>A a b B</p>",
        ),
        // A macro in an expansion expands before the next one; a keyword's
        // value and an argument are scanned again.
        (
            "#+MACRO: w <$1>\n#+MACRO: v {{{w($1)}}}\n#+K: {{{w(k)}}}\nA {{{v(x)}}} {{{keyword(K)}}} {{{w({{{w(y)}}})}}} B\n",
            // The outer macro's arguments end at the FIRST `)}}}`, so
            // `{{{w({{{w(y)}}})}}}` is `{{{w({{{w(y)}}}` then `)}}}`, and
            // Emacs exports a target `<<y>>`.
            "<p>A &lt;x&gt; &lt;k&gt; <span class=\"org-target\" id=\"t-y\"></span> B</p>",
        ),
        // `ITEM` and `TODO`, read from the headline (Emacs: `A H B`).
        ("* TODO [#A] H :t:\nA {{{property(ITEM)}}} {{{property(todo)}}} B\n", "<p>A H TODO B</p>"),
        // The fixed time.
        ("A {{{time(%Y-%m-%d)}}} B\n", "<p>A 1970-01-01 B</p>"),
    ];
    for (doc, want) in cases {
        let h = html(doc);
        assert!(h.contains(want), "{doc}\n{h}");
    }
    let r = rendered("A {{{time(%Y)}}} {{{input-file}}} B\n");
    let at = |s: &str| {
        let d = "A {{{time(%Y)}}} {{{input-file}}} B\n";
        let k = d.find(s).unwrap();
        RenderDiagnostic::MacroEnvironment(mae_org::Span {
            start: k as u32,
            end: (k + s.len()) as u32,
        })
    };
    assert_eq!(
        r.diagnostics,
        [at("{{{time(%Y)}}}"), at("{{{input-file}}}")]
    );
}

/// Parsed keywords (ox-html's `TITLE`, `DATE`, `AUTHOR`, `SUBTITLE`) have
/// their macros expanded anywhere in the value, as Org forces it: the file
/// node's title is the expansion.
#[test]
fn a_parsed_keyword_has_its_macros_expanded() {
    let doc = ":PROPERTIES:\n:ID: f\n:END:\n#+MACRO: m <$1>\n#+TITLE: Hello {{{m(world)}}} =v {{{m(x)}}}=\nbody\n";
    let r = rendered(doc);
    let title = mae_org::to_html(&r.nodes[0].title);
    assert_eq!(
        title,
        "Hello &lt;world&gt; <code class=\"org-verbatim\">v &lt;x&gt;</code>"
    );
}

/// Anchors do not depend on macro definitions (review M5): a heading's
/// slug is its title as written, its macros contributing nothing, so the
/// title spelled in a link fragment lands on it from any document.
#[test]
fn a_heading_anchor_ignores_its_macros() {
    let one = "#+MACRO: m $1\n\n* Intro {{{m(x)}}}\n:PROPERTIES:\n:ID: n1\n:END:\nbody [[id:n1#Intro {{{m(x)}}}]]\n";
    let other = "* Other\n:PROPERTIES:\n:ID: n2\n:END:\nlink [[id:n1#Intro {{{m(x)}}}]]\n";
    let r = Resolver::default().with_target("n1", Resolved::new("/r/n1"));
    let render_html = |doc: &str| {
        let d = parse(doc, &ParseOptions::default());
        mae_org::render_html(&d, &semantics(&d), &RenderOptions::default(), &r)
    };
    let h1 = render_html(one);
    assert!(h1.contains("<h1 id=\"h-intro\">Intro x</h1>"), "{h1}");
    assert!(h1.contains("#h-intro\""), "{h1}");
    // Another definition of `m`: the same anchor.
    let h2 = render_html(&one.replace("#+MACRO: m $1", "#+MACRO: m other words $1"));
    assert!(h2.contains("<h1 id=\"h-intro\">"), "{h2}");
    let h3 = render_html(other);
    assert!(h3.contains("href=\"/r/n1#h-intro\""), "{h3}");
}

/// Nodes are matched to their source nodes by id: offsets moved by an
/// expansion change nothing, and a node the expansion removes renders no
/// metadata.
#[test]
fn node_metadata_follows_the_node_through_the_expansion() {
    let doc = "#+MACRO: long a much longer text than the macro\n#+MACRO: id $1\nA {{{long}}}.\n* T {{{long}}}\n:PROPERTIES:\n:ID: t\n:END:\nbody\n* {{{id(COMMENT)}}} Gone\n:PROPERTIES:\n:ID: g\n:END:\nSECRET\n";
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    let r = render(&d, &sem, &RenderOptions::default(), &Resolver::default());
    let ids: Vec<_> = r.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, ["t", "g"]);
    assert_eq!(
        mae_org::to_html(&r.nodes[0].title),
        "T a much longer text than the macro"
    );
    assert_eq!(r.nodes[0].anchor.as_deref(), Some("id-t"));
    assert!(r.nodes[1].title.is_empty() && r.nodes[1].anchor.is_none());
    assert_eq!(node_text(&d, &sem, "g").as_deref(), Some(""));
    assert!(!r.to_json(TreeFormat::Html).contains("SECRET"));
}

/// Diagnostics of the expanded text are reported where the text is in the
/// source; and the expanded text is read with the document's own options
/// (here a depth cap that lets the emphasis nest past the renderer's).
#[test]
fn diagnostics_are_at_their_place_in_the_source() {
    let deep = format!("{}x{}", "*/_+".repeat(300), "+_/*".repeat(300));
    let doc = format!(
        "#+MACRO: long a much longer text than the macro\nA {{{{{{long}}}}}}.\n\nB {deep}\n"
    );
    let opts = ParseOptions::default().with_max_depth(mae_org::MAX_DEPTH_CEILING);
    let d = parse(&doc, &opts);
    let r = render(
        &d,
        &semantics(&d),
        &RenderOptions::default(),
        &Resolver::default(),
    );
    let source_start = doc.find("B ").unwrap() as u32;
    assert!(
        r.diagnostics
            .iter()
            .any(|d| matches!(d, RenderDiagnostic::TooDeep(s) if s.start >= source_start && (s.end as usize) <= doc.len())),
        "{:?}",
        r.diagnostics
    );
}

/// The reviewer's shape (B7): a template of thousands of words called
/// hundreds of times by another, called N times. Bounded linearly in the
/// source: it fails within the budget rather than costing seconds.
#[test]
fn an_exponential_macro_fails_within_the_budget() {
    let mut doc = format!("#+MACRO: b {}\n", "x ".repeat(3000));
    doc.push_str(&format!("#+MACRO: a {}\n", "{{{b}}}".repeat(255)));
    doc.push_str(&"{{{a}}} ".repeat(100));
    doc.push('\n');
    let t = std::time::Instant::now();
    let r = rendered(&doc);
    assert!(r.body.is_empty(), "{:?}", r.diagnostics);
    assert!(
        matches!(
            r.diagnostics[..],
            [RenderDiagnostic::MacroFailed(MacroFailure::Budget, _)]
        ),
        "{:?}",
        r.diagnostics
    );
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
}

/// What an expansion makes changes how Org reads the rest, as it scans:
/// a headline made COMMENT has its macros skipped (an undefined one there
/// is no error), a verbatim opened before a macro makes it text, and a
/// results macro babel keeps becomes its argument before the page is read
/// (a `:noexport:` tag that drops its subtree). Emacs 30.2 on each.
#[test]
fn what_an_expansion_makes_is_read_as_org_reads_it() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "#+MACRO: id $1\n* {{{id(COMMENT)}}} T\n{{{nosuch}}}\n* U\nok\n",
            "<p>ok</p>",
            &["T"],
        ),
        (
            "#+MACRO: id $1\n{{{id(=)}}}{{{nosuch}}}= after\n",
            "<p><code class=\"org-verbatim\">{{{nosuch}}}</code> after</p>",
            &[],
        ),
        (
            "#+MACRO: m {{{results($1)}}}\n* H {{{m(:noexport:)}}}\nbody SECRET\n* V\nseen\n",
            "<p>seen</p>",
            &["SECRET"],
        ),
        (
            "* {{{results(COMMENT)}}} H\nbody SECRET\n* V\nseen\n",
            "<p>seen</p>",
            &["SECRET"],
        ),
        (
            "Lone {{{results(=r=)}}} here.\n",
            "<p>Lone <code class=\"org-verbatim\">r</code> here.</p>",
            &[],
        ),
    ];
    for (doc, want, hidden) in cases {
        let h = html(doc);
        assert!(h.contains(want), "{doc}\n{h}");
        for s in *hidden {
            assert!(!h.contains(s), "{doc}\n{h}");
        }
    }
}
