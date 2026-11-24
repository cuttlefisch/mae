//! The live site's highlighter (ADR-119 D12; P-105 to P-110) with the real
//! grammars: every language the daemon carries colours code, and keeps
//! every block's text exact whatever the text is.

use std::collections::BTreeSet;

use mae_daemon::site_highlight::{plain_block_lines, shared};
use mae_site::{derive_with, Addressing, Plain, PlainBlock, SiteOptions, SourcePage};
use mae_syntax::Grammar;

/// Code each grammar reads as code: what a reader of that language writes.
fn sample(g: Grammar) -> &'static str {
    match g.name() {
        "rust" => "fn main() { let x = \"hi\"; } // c",
        "toml" => "[package]\nname = \"x\" # c\nversion = 1",
        "markdown" => "# Title\n\nSome `code` here.",
        "python" => "def f(a):\n    return \"x\" + str(a)  # c",
        "javascript" | "typescript" | "tsx" => "const f = (a) => `x${a}`; // c",
        "go" => "package main\nfunc f() string { return \"x\" } // c",
        "json" => "{\"a\": [1, true, null]}",
        "bash" => "echo \"$HOME\" | grep -c x # c",
        "scheme" => "(define (f a) \"x\") ; c",
        "yaml" => "key: value # c\nn: 42",
        "c" | "cpp" => "int f(void) { return 42; } // c",
        "ruby" => "def f(a)\n  \"x\" # c\nend",
        "terraform" => "resource \"a\" \"b\" {\n  count = 2 # c\n}",
        "dockerfile" => "FROM rust:1.80\nRUN [\"echo\", \"hi\"]",
        "powershell" => "$x = Get-Item -Path \"C:\\\\t\" # c",
        "ini" => "[section]\nkey = value",
        "sql" => "SELECT a, COUNT(*) FROM t WHERE b = 'x'; -- c",
        "nginx" => "server {\n  listen 80;\n}",
        other => panic!("no sample for {other}: add one"),
    }
}

fn page(id: &str, body: &str) -> SourcePage {
    SourcePage {
        origin: id.into(),
        text: format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n\n{body}\n"),
    }
}

fn derive(pages: &[SourcePage], highlight: bool) -> mae_site::Site {
    let opts = SiteOptions {
        highlighter: highlight.then(shared),
        ..Default::default()
    };
    derive_with(pages, &Addressing::live("kb").unwrap(), &opts)
}

/// Every source block's text in `html`: tags dropped, escapes decoded.
fn block_texts(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in html.split("<pre class=\"org-src\">").skip(1) {
        let inner = part.split("</pre>").next().unwrap_or_default();
        let mut text = String::new();
        let mut in_tag = false;
        for c in inner.chars() {
            match c {
                '<' => in_tag = true,
                '>' if in_tag => in_tag = false,
                c if !in_tag => text.push(c),
                _ => {}
            }
        }
        out.push(
            text.replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&nbsp;", "\u{a0}")
                .replace("&amp;", "&"),
        );
    }
    out
}

/// The daemon carries the editor's grammars and the site's four, and each
/// one's queries compile (nginx's is vendored: a stale copy fails here) and
/// colour its own language.
#[test]
fn every_daemon_grammar_builds_and_colours_its_language() {
    let names: Vec<&str> = Grammar::ALL.iter().map(|g| g.name()).collect();
    for site_only in ["powershell", "ini", "sql", "nginx"] {
        assert!(names.contains(&site_only), "{site_only}: {names:?}");
    }
    let h = shared();
    for &g in Grammar::ALL {
        assert!(mae_syntax::build_configuration(g).is_some(), "{g:?}");
        let spans = h.highlight(g.name(), sample(g)).expect("a known language");
        assert!(!spans.is_empty(), "{g:?} coloured nothing");
    }
}

/// A deterministic generator: varied texts, reproducible failures.
fn rng(mut s: u64) -> impl FnMut(usize) -> usize {
    move |n| {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s % n as u64) as usize
    }
}

/// A text from the pieces that break copying (tabs, trailing spaces, CRLF,
/// blank lines, emoji, markup-like text, a line over 2,000 characters,
/// unbalanced syntax) around the language's own code. No line starts with
/// `*` or `#+` (Org would read them).
fn text(g: Grammar, next: &mut impl FnMut(usize) -> usize) -> String {
    const PIECES: &[&str] = &[
        "\t",
        "x   ",
        "\n",
        "\n\n\n",
        "\r\n",
        "<b>&amp;</b>",
        "]] &",
        "\u{1F600}",
        "é",
        "\"unterminated",
        "((( {",
        "end end",
    ];
    let mut out = String::new();
    for _ in 0..next(8) {
        match next(12) {
            0 => out.push_str(&"y".repeat(2_100)),
            1 | 2 => out.push_str(sample(g)),
            _ => out.push_str(PIECES[next(PIECES.len())]),
        }
    }
    out
}

/// P-107, for every language: each block reads exactly as it does plain,
/// whatever its text, empty included; none needs the byte-exact fallback.
#[test]
fn every_language_keeps_every_block_exact() {
    let mut next = rng(107);
    for &g in Grammar::ALL {
        let mut body = format!("#+begin_src {}\n#+end_src\n", g.name());
        for _ in 0..12 {
            body.push_str(&format!(
                "#+begin_src {}\n{}\n#+end_src\n",
                g.name(),
                text(g, &mut next)
            ));
        }
        let pages = [page("p", &body)];
        let (plain, lit) = (derive(&pages, false), derive(&pages, true));
        let (plain, lit_html) = (&plain.pages["p"].html, &lit.pages["p"].html);
        let want = block_texts(plain);
        assert_eq!(want.len(), 13, "{g:?}");
        assert_eq!(block_texts(lit_html), want, "{g:?}");
        assert!(lit_html.contains("org-code-"), "{g:?}: nothing coloured");
        assert!(!lit_html.contains("style="), "{g:?}");
        let not_exact: Vec<_> = lit
            .plain_blocks
            .iter()
            .filter(|b| b.reason == Plain::NotExact)
            .collect();
        assert!(not_exact.is_empty(), "{g:?}: {not_exact:?}");
    }
}

/// P-108: an unknown, typo or undeclared language is plain, the page
/// publishes, and the log line names the language and the page.
#[test]
fn unknown_and_undeclared_languages_are_plain_and_logged_by_name() {
    let body = "#+begin_src pyhton\nx = 1\n#+end_src\n#+begin_src\nbare\n#+end_src\n#+begin_src sh\necho ok\n#+end_src\n";
    let site = derive(&[page("the-page", body)], true);
    assert!(site.pages.contains_key("the-page"));
    let lines = plain_block_lines("kb-1", &site.plain_blocks);
    assert_eq!(lines.len(), 2, "{lines:?}");
    let unknown = lines.iter().find(|l| l.contains("unknown language"));
    assert!(
        unknown.is_some_and(|l| l.contains("\"pyhton\"") && l.contains("\"the-page\"")),
        "{lines:?}"
    );
    let undeclared = lines.iter().find(|l| l.contains("undeclared language"));
    assert!(
        undeclared.is_some_and(|l| l.contains("(none)") && l.contains("\"the-page\"")),
        "{lines:?}"
    );
    assert!(lines.iter().all(|l| l.contains("\"kb-1\"")), "{lines:?}");
}

/// The log line is one per reason and language however many pages share
/// it, names at most ten of them, and cannot be broken by what a page ID or
/// language holds.
#[test]
fn a_plain_block_log_line_is_bounded_and_escaped() {
    let mut blocks = BTreeSet::new();
    for i in 0..25 {
        blocks.insert(PlainBlock {
            page: format!("p{i:02}"),
            language: Some("elisp".into()),
            reason: Plain::Unknown,
        });
    }
    blocks.insert(PlainBlock {
        page: "evil\nINFO forged line".into(),
        language: Some("x\r\ny".into()),
        reason: Plain::Unknown,
    });
    let lines = plain_block_lines("kb", &blocks);
    assert_eq!(lines.len(), 2, "{lines:?}");
    let elisp = lines.iter().find(|l| l.contains("\"elisp\"")).unwrap();
    assert!(
        elisp.contains("\"p09\"") && !elisp.contains("\"p10\""),
        "{elisp}"
    );
    assert!(elisp.ends_with("and 15 more"), "{elisp}");
    for l in &lines {
        assert!(!l.contains('\n') && !l.contains('\r'), "{l:?}");
    }
}
