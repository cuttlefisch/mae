//! The macro oracle, run locally (review rounds 4 and 5):
//!
//! ```sh
//! cargo test -p mae-org --release --test render_macros_oracle -- --ignored --nocapture
//! ```
//!
//! Generates a corpus of documents (template shapes × argument shapes ×
//! the places a macro stands; macro PAIRS that open a construct on one line
//! and close it on another, in every kind of element; the reviewers'
//! inputs from `tests/macro-review/`), each holding markers (`QZ…QZ`, or
//! the reviewers' `SECRET…`); exports every one through Emacs's ox-html
//! (`conformance/macro-export.el`: body only, no file visited, babel never
//! evaluated) in one bounded `emacs -Q --batch`; and compares markers,
//! case folded (a slug lowercases them):
//! - a LEAK is a marker anywhere in MAE's output (HTML with its attributes,
//!   hast, every node's metadata and anchor, `text`, every `node_text`)
//!   that is nowhere in Emacs's HTML, attributes included;
//! - a LOSS is a marker Emacs's page shows (tags stripped) that MAE's page
//!   and text do not.
//!
//! It fails on any leak or loss, on a document MAE renders where Emacs
//! fails, and unless the documents MAE renders nothing for while Emacs
//! renders them (registered: `macros`) are exactly [`FAIL_CLOSED`]. Emacs
//! failing on an `id:` link (no id tracking in the oracle) leaves a
//! document uncompared, counted. The corpus is written under Cargo's
//! target directory, never the system temp dir.

mod support;

use mae_org::{parse, render_html, semantics, text, ParseOptions, RenderOptions, Resolver};
use std::collections::BTreeSet;
use std::path::Path;
use support::*;

/// The documents MAE renders nothing for (a macro it cannot expand from
/// the text, or past the budget) while Emacs renders them: the `EXTRA`
/// entries with an `(eval …)` template, a date format and a special
/// property.
const FAIL_CLOSED: &[&str] = &["x26", "x27", "x28"];

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
    // MAE cannot expand these from the text; Emacs evaluates or looks up.
    "#+MACRO: e (eval (+ 1 2))\nA {{{e}}} QZaQZ\n",
    "#+DATE: <2024-01-02 Tue>\nA {{{date(%Y)}}} QZaQZ\n",
    "* H :t:\nA {{{property(ALLTAGS)}}} QZaQZ\n",
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

/// Macro pairs: one opens a construct, the other closes it; between and
/// after them, markers. Each is laid out across lines and elements.
const PAIRS: &[(&str, &str)] = &[
    ("#+begin_comment", "#+end_comment"),
    ("#+begin_export latex", "#+end_export"),
    ("#+begin_src sh :exports none", "#+end_src"),
    ("#+begin_example", "#+end_example"),
    ("#+begin_quote", "#+end_quote"),
    ("* COMMENT x", "* Visible"),
    ("* Hidden :noexport:", "* Visible"),
    (":DRAWER:", ":END:"),
    ("@@latex:", "@@"),
    ("src_sh[:exports none]{echo", "}"),
    ("call_fn(x=\"", "\")"),
    ("=", "="),
    ("[[https://e.com][", "]]"),
];

/// Layouts of a pair; `<O>`/`<C>` are the opening and closing macros,
/// `<CL>` the closer written out, `<M1>`/`<M2>` markers.
const LAYOUTS: &[&str] = &[
    "<O>\n<C>\n<M1>\n<CL>\nafter <M2>\n",
    "X <O>\n<C> <M1> <CL> Y <M2>\n",
    "<O>\n<M1>\n<C>\n<M2>\n",
    "* H\n<O>\n<C>\n<M1>\n",
    "X <O> <M1> <C> Y <M2>\n",
    "Para.\n\n<O>\n\n<C>\n\n<M1>\n\n<CL>\n\n<M2>\n",
    "- item <O>\n- item <C> <M1> <CL>\n\n<M2>\n",
];

fn pairs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (pi, (o, c)) in PAIRS.iter().enumerate() {
        for (li, l) in LAYOUTS.iter().enumerate() {
            let m = |k: u8| format!("QZp{pi}l{li}m{k}QZ");
            let body = l
                .replace("<M1>", &m(1))
                .replace("<M2>", &m(2))
                .replace("<CL>", c)
                .replace("<O>", "{{{o}}}")
                .replace("<C>", "{{{c}}}");
            let doc = format!("#+MACRO: o {o}\n#+MACRO: c {c}\n{body}{POSTAMBLE}");
            out.push((format!("p{pi:02}-{li}"), doc));
        }
    }
    out
}

/// The reviewers' inputs, as written.
fn review() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/macro-review");
    org_files(&dir)
        .into_iter()
        .map(|f| {
            let name = f.file_stem().unwrap().to_string_lossy().to_string();
            (format!("r-{name}"), read(&f))
        })
        .collect()
}

/// The markers in `s`, case folded: `qz…qz`, and the reviewers' `secret…`.
fn markers(s: &str) -> BTreeSet<String> {
    let s = s.to_lowercase();
    let mut out = BTreeSet::new();
    let word = |t: &str| t.bytes().take_while(u8::is_ascii_alphanumeric).count();
    let mut rest = s.as_str();
    while let Some(i) = rest.find("qz") {
        let after = &rest[i + 2..];
        match after[..word(after)].find("qz") {
            Some(j) if j > 0 => {
                out.insert(format!("qz{}qz", &after[..j]));
                rest = &after[j + 2..];
            }
            _ => rest = after,
        }
    }
    let mut rest = s.as_str();
    while let Some(i) = rest.find("secret") {
        let w = &rest[i..];
        out.insert(w[..word(w)].to_string());
        rest = &w[6..];
    }
    out
}

/// `html` without its tags: what a reader sees.
fn visible(html: &str) -> String {
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
    text
}

/// What a document is to MAE: `None` when it renders nothing for a macro
/// failure; else the markers anywhere in its output, and those its page
/// and text show.
fn mae(doc: &str) -> Option<(BTreeSet<String>, BTreeSet<String>)> {
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    let mut raw = BTreeSet::new();
    let mut shown = BTreeSet::new();
    let option_sets = [
        RenderOptions::default(),
        RenderOptions::default().with_headline_ids(mae_org::HeadlineIds::Slug),
    ];
    for opts in option_sets {
        let r = mae_org::render(&d, &sem, &opts, &Resolver::default());
        if r.diagnostics
            .iter()
            .any(|d| matches!(d, mae_org::RenderDiagnostic::MacroFailed(..)))
        {
            return None;
        }
        raw.extend(markers(&r.to_json(mae_org::TreeFormat::Html)));
        raw.extend(markers(&r.to_json(mae_org::TreeFormat::Hast)));
        shown.extend(markers(&visible(&render_html(
            &d,
            &sem,
            &opts,
            &Resolver::default(),
        ))));
    }
    let t = text(&d, &sem);
    raw.extend(markers(&t));
    shown.extend(markers(&t));
    for n in &sem.nodes {
        raw.extend(markers(
            &mae_org::node_text(&d, &sem, &n.id).unwrap_or_default(),
        ));
    }
    Some((raw, shown))
}

#[test]
#[ignore = "needs emacs; run locally"]
fn macro_oracle() {
    let mut corpus = corpus();
    corpus.extend(pairs());
    corpus.extend(review());
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("mae-org-macro-oracle-{}", std::process::id()));
    let out = export(&corpus, &dir);
    let mut n = Tally::default();
    let mut problems = Vec::new();
    let mut fail_closed = Vec::new();
    for (name, doc) in &corpus {
        n.docs += 1;
        match (read_result(&out, name), mae(doc)) {
            (Emacs::Missing, _) => n.missing_oracle += 1,
            // Babel and id links Emacs cannot resolve here: not macros.
            (Emacs::Error(e), _)
                if e.contains("org-id-track-globally") || e.contains("Unknown Babel reference") =>
            {
                n.emacs_unrelated_error += 1
            }
            (Emacs::Error(_), None) => n.both_fail += 1,
            (Emacs::Error(e), Some(_)) => {
                n.mae_renders_emacs_errors += 1;
                problems.push(format!("{name}: Emacs fails ({e}), MAE renders:\n{doc}"));
            }
            (Emacs::Html(_), None) => fail_closed.push(name.clone()),
            (Emacs::Html(h), Some((raw, shown))) => {
                let e_raw = markers(&h);
                let e_shown = markers(&visible(&h));
                let leaked: Vec<_> = raw.difference(&e_raw).collect();
                // A drawer Org exports and MAE does not (registered:
                // drawers).
                let drawer = doc.contains(":DRAWER:");
                let lost: Vec<_> = e_shown.difference(&shown).filter(|_| !drawer).collect();
                n.compared += 1;
                n.markers += e_raw.len().max(raw.len());
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
    n.fail_closed = fail_closed.len();
    eprintln!("{n:#?}\nfail closed: {fail_closed:?}");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(n.missing_oracle, 0, "Emacs left documents unexported");
    assert_eq!(n.leaks, 0, "MAE shows markers Emacs hides");
    assert_eq!(n.lost, 0, "MAE hides markers Emacs shows");
    assert_eq!(
        n.mae_renders_emacs_errors, 0,
        "MAE renders where Emacs fails"
    );
    assert_eq!(fail_closed, FAIL_CLOSED, "fail-closed documents changed");
}

/// Writes `corpus` under `dir` and exports it through Emacs, bounded;
/// where the exports are.
fn export(corpus: &[(String, String)], dir: &Path) -> std::path::PathBuf {
    let (inp, out) = (dir.join("in"), dir.join("out"));
    std::fs::create_dir_all(&inp).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    for (name, doc) in corpus {
        std::fs::write(inp.join(format!("{name}.org")), doc).unwrap();
    }
    let el = repo_root().join("shared/org/conformance/macro-export.el");
    let args: Vec<std::ffi::OsString> = vec![
        "-Q".into(),
        "--batch".into(),
        "-l".into(),
        el.into(),
        inp.into(),
        out.clone().into(),
    ];
    let exit = run_emacs_bounded(
        &args,
        3 * 1024 * 1024,
        std::time::Duration::from_secs(1200),
        std::process::Stdio::null(),
    );
    eprintln!("emacs: {exit:?}");
    out
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
    fail_closed: usize,
    mae_renders_emacs_errors: usize,
    emacs_unrelated_error: usize,
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
