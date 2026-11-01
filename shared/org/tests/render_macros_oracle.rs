//! The macro oracle, run locally (review round 4):
//!
//! ```sh
//! cargo test -p mae-org --release --test render_macros_oracle -- --ignored --nocapture
//! ```
//!
//! Generates a corpus of documents (template shapes × argument shapes ×
//! the places a macro stands, plus the reviewers' inputs), each holding
//! markers (`QZ…QZ`); exports every one through Emacs's ox-html
//! (`conformance/macro-export.el`: body only, no file visited, babel never
//! evaluated) in one bounded `emacs -Q --batch`; and compares the markers
//! each page SHOWS (tags stripped) with MAE's body and text. It fails if
//! MAE shows a marker Emacs hides (a leak), or hides one Emacs shows
//! outside the registered divergences (`macros` in
//! `conformance/render-divergences.toml`: a document MAE cannot expand
//! renders nothing; `input-file`, `modification-time` and `time` are
//! fixed). It prints the counts. Nothing is written into the repository:
//! the corpus goes to a scratch directory under the system temp dir.

mod support;

use mae_org::{parse, render_html, semantics, text, ParseOptions, RenderOptions, Resolver};
use std::collections::BTreeSet;
use std::path::Path;
use support::*;

/// Template shapes; `$1`, `$2` and `$0` are Org's.
const TEMPLATES: &[&str] = &[
    "$1",
    "<$1>",
    "$1$2",
    "$0|$2",
    "src_sh[:exports none]{echo $1}",
    "src_sh[:exports code]{echo $1}",
    "@@html:<b>@@$1@@html:</b>@@",
    "@@latex:$1@@",
    "call_fn(x=\"$1\")",
    "=$1=",
    "*$1*",
    "[[https://example.com][$1]]",
    "{{{n}}}-$1",
    "{{{wrap($1)}}}",
    "$1}",
    "$1)",
    "$1@",
    "# $1",
    "{{{results($1)}}}",
    "{{{keyword(K)}}} $1",
];

/// Argument shapes; `A` and `B` become markers.
const ARGUMENTS: &[&str] = &[
    "A",
    "A,B",
    "src,_sh[:exports none]{echo A}",
    "A\\,B",
    "  A \n B ",
    "*A*",
    "@@latex:A@@",
    ":noexport:",
    "COMMENT",
];

/// Where the macro stands; `{X}` is the call, `C` a marker of the context.
const PLACES: &[&str] = &[
    "Para {X} end C.\n",
    "{X} tail C\n",
    "* H {X}\nbody C\n",
    "* {X} H\nbody C\n",
    "#+TITLE: T {X}\n\nBody {{{title}}} C.\n",
    "| {X} | C |\n",
    "- {X} item C\n",
    "- {X} :: desc C\n",
    "Ref[fn:1] C.\n\n[fn:1] Note {X}.\n",
    "#+CAPTION: {X}\n| C |\n",
    "Ref[fn:: n {X}] C.\n",
    "[[https://e.com][d {X}]] C\n",
    "=v {X}= C\n",
    "#+begin_src sh\n{X}\n#+end_src\n\nC\n",
    "* H\n:PROPERTIES:\n:P: {X}\n:END:\n{{{property(P)}}} C\n",
    "src_sh[:exports none]{echo {X}} C\n",
];

/// The called block, the macro a template calls, and a keyword.
const POSTAMBLE: &str = "\n* Post\n#+NAME: fn\n#+begin_src sh :var x=\"\" :exports both\necho visible\n#+end_src\n\n#+MACRO: wrap [$1]\n#+K: kval QZkQZ\n";

/// The reviewers' inputs and the built-in cases (`tests/render_macros.rs`).
const EXTRA: &[&str] = &[
    "#+MACRO: close $1}\n\nsrc_sh[:exports none]{echo {{{close(QZaQZ)}}}\n",
    "#+MACRO: at $1@\n\n@@latex:{{{at(QZaQZ)}}}@\n",
    "#+MACRO: j $0$2\n\n{{{j(src,_sh[:exports none]{echo QZaQZ})}}}\n",
    "#+MACRO: j $2\n\n{{{j(src,_sh[:exports none]{echo QZaQZ})}}}\n",
    "#+MACRO: id $1\n\nPara one QZpQZ.\n{{{id(#)}}} QZaQZ\n",
    "#+MACRO: id $1\n\n#+NAME: fn\n#+begin_src sh :var x=\"\" :exports none\necho\n#+end_src\n\nPara QZpQZ.\n\n{{{id(#+CALL: fn(QZaQZ))}}}\n",
    "#+X: src_sh[:exports none]{echo\n\n{{{keyword(X)}}} QZaQZ}\n",
    "#+MACRO: id $1\n\n{{{id(#+begin_comment)}}}\nQZaQZ\n#+end_comment\n",
    "* H\n:PROPERTIES:\n:P: src_sh[:exports none]{echo\n:END:\n{{{property(P)}}} QZaQZ}\n",
    "#+MACRO: id $1\n\n* {{{id(COMMENT)}}} QZvQZ\nQZaQZ body\n",
    "#+MACRO: id $1\n\n* QZhQZ {{{id(:noexport:)}}}\nQZaQZ body\n",
    "#+MACRO: id $1\n\n* QZvQZ\n{{{id(:PROPERTIES:)}}}\n:QZaQZ: x\n:END:\n",
    "Text {{{m(QZaQZ)}}} here.\n\n* COMMENT old\n#+MACRO: m safe $1\n\n* Real\n#+MACRO: m src_sh[:exports none]{echo $1}\n",
    "Text {{{m(QZaQZ)}}} here.\n\n#+MACRO: M src_sh[:exports none]{echo $1}\n#+MACRO: m safe $1\n",
    "#+MACRO: title src_sh[:exports none]{echo $1}\n#+TITLE: QZtQZ\n\nText {{{title(QZaQZ)}}} here.\n",
    "#+MACRO: color @@html:<span style=\"color:$1\">@@$2@@html:</span>@@\n\nSee {{{color(QZcQZ,QZvQZ)}}} now.\n",
    "#+MACRO: sec S{{{n}}}QZsQZ\nA {{{n}}} {{{sec}}} {{{n}}} {{{n(x)}}} {{{n(x,10)}}} {{{n(x,-)}}} {{{n(x,reset)}}} QZaQZ\n",
    "#+MACRO: w <$1>\n#+MACRO: v {{{w($1)}}}\n#+K: {{{w(QZkQZ)}}}\nA {{{v(QZxQZ)}}} {{{keyword(K)}}} {{{w({{{w(QZyQZ)}}})}}} B\n",
    "#+MACRO: a <{{{b}}}>\n#+MACRO: b <{{{a}}}>\nA {{{a}}} QZaQZ\n",
    "#+MACRO: id $1\n{{{id(=)}}}{{{nosuch}}}= QZaQZ\n",
    "#+MACRO: id $1\n* {{{id(COMMENT)}}} T\n{{{nosuch}}}\n* QZuQZ\nok\n",
    "* TODO [#A] QZhQZ :t:\nA {{{property(ITEM)}}} {{{property(TODO)}}} B\n",
    "#+DATE: QZdQZ\n#+AUTHOR: QZaQZ\n#+EMAIL: QZeQZ\nA {{{date}}} {{{author}}} {{{email}}} B\n",
    "#+MACRO: Title x\nA {{{title}}} B {{{Title}}} QZaQZ\n",
    "A {{{n}}} {{{n()}}} {{{n( )}}} {{{n(,)}}} {{{n(a,007)}}} {{{n(a)}}} QZaQZ\n",
    "#+MACRO: a {{{b}}}\n#+MACRO: b {{{a}}}\nA {{{a}}} QZaQZ\n",
    "#+MACRO: m $1\n#+MACRO: cat $1$2\nSee {{{m({{{cat(<,QZaQZ>)}}})}}} end QZbQZ.\n",
    "#+MACRO: mr {{{results($1)}}}\nA {{{mr(QZaQZ)}}} {{{results(QZbQZ,QZcQZ)}}} B\n",
    "#+MACRO: mp <$1>\n#+MACRO: mq <{{{mp($1)}}}>\n:PROPERTIES:\n:ID: f\n:END:\n#+TITLE: T {{{mq(QZaQZ)}}}\nBody {{{title}}} QZbQZ\n",
    "* H\n:PROPERTIES:\n:EXPORT_TITLE: {{{n}}} QZaQZ\n:P: {{{n}}}\n:END:\nA {{{n}}} {{{property(P)}}} B\n",
];

/// The corpus: (name, document).
fn corpus() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut k = 0;
    for (ti, t) in TEMPLATES.iter().enumerate() {
        for (ai, a) in ARGUMENTS.iter().enumerate() {
            for (pi, p) in PLACES.iter().enumerate() {
                // A caption whose macro runs onto the next line captions
                // that line's paragraph, which ox-html drops and MAE draws
                // (registered: paragraph-caption); a property line that
                // does makes the drawer an ordinary one, which Org exports
                // and MAE does not (registered: drawers).
                if a.contains('\n') && (p.starts_with("#+CAPTION") || p.contains(":P:")) {
                    continue;
                }
                let m = |c: &str| format!("QZ{c}{k}QZ");
                let args = a.replace('A', &m("a")).replace('B', &m("b"));
                let call = format!("{{{{{{m({args})}}}}}}");
                let place = p
                    .replace("{X}", &call)
                    .replace(" C", &format!(" {}", m("c")));
                let doc = format!("#+MACRO: m {t}\n{place}{POSTAMBLE}");
                out.push((format!("g{ti:02}-{ai}-{pi:02}"), doc));
                k += 1;
            }
        }
    }
    for (i, d) in EXTRA.iter().enumerate() {
        out.push((format!("x{i:02}"), d.to_string()));
    }
    out
}

/// The markers `html`'s text shows (tags stripped).
fn shown(html: &str) -> BTreeSet<String> {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find("QZ") {
        let after = &rest[i + 2..];
        let len = after
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(after.len());
        match after[..len].find("QZ") {
            Some(j) if j > 0 => {
                out.insert(format!("QZ{}QZ", &after[..j]));
                rest = &after[j + 2..];
            }
            _ => rest = after,
        }
    }
    out
}

/// What a document is to MAE: `None` when it renders nothing for a macro
/// failure, else the markers its body and text show.
fn mae(doc: &str) -> Option<BTreeSet<String>> {
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    let r = mae_org::render(&d, &sem, &RenderOptions::default(), &Resolver::default());
    if r.diagnostics
        .iter()
        .any(|d| matches!(d, mae_org::RenderDiagnostic::MacroFailed(..)))
    {
        return None;
    }
    let html = render_html(&d, &sem, &RenderOptions::default(), &Resolver::default());
    let mut s = shown(&html);
    s.extend(shown(&text(&d, &sem)));
    Some(s)
}

#[test]
#[ignore = "needs emacs; run locally"]
fn macro_oracle() {
    let corpus = corpus();
    let dir = std::env::temp_dir().join(format!("mae-org-macro-oracle-{}", std::process::id()));
    let (inp, out) = (dir.join("in"), dir.join("out"));
    std::fs::create_dir_all(&inp).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    for (name, doc) in &corpus {
        std::fs::write(inp.join(format!("{name}.org")), doc).unwrap();
    }
    let el = repo_root().join("shared/org/conformance/macro-export.el");
    let args: Vec<std::ffi::OsString> = vec![
        "-Q".into(),
        "--batch".into(),
        "-l".into(),
        el.into(),
        inp.clone().into(),
        out.clone().into(),
    ];
    let exit = run_emacs_bounded(
        &args,
        3 * 1024 * 1024,
        std::time::Duration::from_secs(1200),
        std::process::Stdio::null(),
    );
    eprintln!("emacs: {exit:?}");
    let mut n = Tally::default();
    let mut problems = Vec::new();
    for (name, doc) in &corpus {
        let emacs = read_result(&out, name);
        n.docs += 1;
        match (emacs, mae(doc)) {
            (Emacs::Missing, _) => n.missing_oracle += 1,
            (Emacs::Error(_), None) => n.both_fail += 1,
            (Emacs::Error(e), Some(_)) => {
                n.mae_renders_emacs_errors += 1;
                problems.push(format!("{name}: Emacs fails ({e}), MAE renders:\n{doc}"));
            }
            (Emacs::Html(_), None) => n.mae_fails_closed += 1,
            (Emacs::Html(h), Some(m)) => {
                let e = shown(&h);
                let leaked: Vec<_> = m.difference(&e).collect();
                let lost: Vec<_> = e.difference(&m).collect();
                n.compared += 1;
                n.markers += e.len();
                if leaked.is_empty() && lost.is_empty() {
                    n.agree += 1;
                }
                if !leaked.is_empty() {
                    n.leaks += 1;
                    problems.push(format!("{name}: LEAK {leaked:?}\n{doc}\nEmacs: {h}"));
                }
                if !lost.is_empty() {
                    n.lost += 1;
                    problems.push(format!("{name}: lost {lost:?}\n{doc}\nEmacs: {h}"));
                }
            }
        }
    }
    for p in problems.iter().take(400) {
        eprintln!("== {p}\n");
    }
    eprintln!("{n:#?}");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(n.missing_oracle, 0, "Emacs left documents unexported");
    assert_eq!(n.leaks, 0, "MAE shows markers Emacs hides");
    assert_eq!(n.lost, 0, "MAE hides markers Emacs shows");
    assert_eq!(
        n.mae_renders_emacs_errors, 0,
        "MAE renders where Emacs fails"
    );
}

#[derive(Debug, Default)]
struct Tally {
    docs: usize,
    compared: usize,
    agree: usize,
    markers: usize,
    leaks: usize,
    lost: usize,
    both_fail: usize,
    mae_fails_closed: usize,
    mae_renders_emacs_errors: usize,
    missing_oracle: usize,
}

enum Emacs {
    Html(String),
    Error(String),
    Missing,
}

fn read_result(out: &Path, name: &str) -> Emacs {
    if let Ok(h) = std::fs::read_to_string(out.join(format!("{name}.html"))) {
        return Emacs::Html(h);
    }
    match std::fs::read_to_string(out.join(format!("{name}.error"))) {
        Ok(e) => Emacs::Error(e),
        Err(_) => Emacs::Missing,
    }
}
