//! Source-block highlighting in the derivation (ADR-119 D12; P-105 to
//! P-110), with stub highlighters: the core holds no grammar, so what is
//! tested here is what it does with any answer, right or wrong. The daemon's
//! tests run the real grammars through the same path.

use std::sync::{Arc, Mutex};

use mae_site::highlight::MAX_HIGHLIGHT_BYTES;
use mae_site::sanitize::sanitize_fragment;
use mae_site::{
    derive_with, Addressing, CodeRole, CodeSpan, Highlighter, Plain, PlainBlock, Site, SiteOptions,
    SourcePage,
};

fn page(id: &str, body: &str) -> SourcePage {
    SourcePage {
        origin: format!("{id}.org"),
        text: format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n\n{body}\n"),
    }
}

fn block(lang: &str, text: &str) -> String {
    format!("#+begin_src {lang}\n{text}\n#+end_src\n")
}

fn derive(pages: &[SourcePage], h: Option<Arc<dyn Highlighter>>) -> Site {
    let opts = SiteOptions {
        highlighter: h,
        ..Default::default()
    };
    derive_with(pages, &Addressing::export("").unwrap(), &opts)
}

/// Every source block's text in `html`, as a reader's copy would read it:
/// tags dropped, the serialiser's escapes decoded.
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

/// Words as keywords, digit runs as numbers, the rest text: a correct
/// answer, for the languages it claims.
struct Words;

impl Highlighter for Words {
    fn highlight(&self, language: &str, text: &str) -> Option<Vec<CodeSpan>> {
        if !["sh", "python", "sql"].contains(&language) {
            return None;
        }
        let mut spans = Vec::new();
        let mut start = None;
        for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
            match (start, c.is_ascii_alphanumeric()) {
                (None, true) => start = Some(i),
                (Some(s), false) => {
                    let all_digits = text[s..i].bytes().all(|b| b.is_ascii_digit());
                    let role = if all_digits {
                        CodeRole::Number
                    } else {
                        CodeRole::Keyword
                    };
                    spans.push(CodeSpan {
                        start: s,
                        end: i,
                        role,
                    });
                    start = None;
                }
                _ => {}
            }
        }
        Some(spans)
    }
}

/// The whole text as one keyword, recording every text it was asked about.
#[derive(Default)]
struct Everything(Mutex<Vec<usize>>);

impl Highlighter for Everything {
    fn highlight(&self, _: &str, text: &str) -> Option<Vec<CodeSpan>> {
        self.0.lock().unwrap().push(text.len());
        Some(vec![CodeSpan {
            start: 0,
            end: text.len(),
            role: CodeRole::Keyword,
        }])
    }
}

/// One wrong answer to a text.
type Answer = fn(&str) -> Vec<CodeSpan>;

/// A highlighter that answers wrongly in one way each.
struct Wrong(Answer);

impl Highlighter for Wrong {
    fn highlight(&self, _: &str, text: &str) -> Option<Vec<CodeSpan>> {
        Some((self.0)(text))
    }
}

fn span(start: usize, end: usize) -> CodeSpan {
    CodeSpan {
        start,
        end,
        role: CodeRole::String,
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

/// A block's text from the pieces that break copying: tabs, trailing spaces,
/// CRLF, blank lines, emoji, markup-like text, a line over 2,000 characters,
/// unbalanced syntax. No line starts with `*` or `#` (Org would read them).
fn text(next: &mut impl FnMut(usize) -> usize) -> String {
    const PIECES: &[&str] = &[
        "\t",
        "x  ",
        "\n",
        "\n\n\n",
        "\r\n",
        "echo 42;",
        "<b>&amp;</b>",
        "]]",
        "&",
        "< >",
        "\u{1F600}",
        "é",
        "\"unterminated",
        "((( {",
        "end end",
        "\u{a0}",
    ];
    let mut out = String::from("a");
    for _ in 0..next(14) {
        if next(20) == 0 {
            out.push_str(&"y".repeat(2_100));
        } else {
            out.push_str(PIECES[next(PIECES.len())]);
        }
    }
    out
}

/// P-107: a highlighted block reads exactly as the plain one, whatever its
/// text: the highlighter changes how it looks, never what it says.
#[test]
fn every_block_keeps_its_exact_text_highlighted_or_not() {
    let mut next = rng(105);
    let mut highlighted = 0;
    for round in 0..60 {
        let texts: Vec<String> = (0..3).map(|_| text(&mut next)).collect();
        let body: String = texts.iter().map(|t| block("sh", t)).collect();
        let pages = [page("p", &body)];
        let plain = derive(&pages, None);
        let words = derive(&pages, Some(Arc::new(Words)));
        let (plain, words) = (&plain.pages["p"].html, &words.pages["p"].html);
        let want = block_texts(plain);
        assert_eq!(want.len(), 3, "{plain}");
        assert_eq!(block_texts(words), want, "round {round}: {words}");
        // The plain block is the oracle: the text as mae-org shows it. Where
        // that is plainly the author's (no CR, whose lines Org reads its own
        // way), check it against the source too: the blank lines before
        // `#+end_src` are not the block's.
        for (t, w) in texts.iter().zip(&want).filter(|(t, _)| !t.contains('\r')) {
            assert_eq!(w, t.trim_end_matches('\n'), "round {round}");
        }
        highlighted += words.matches("org-code-keyword").count();
        assert!(!words.contains("style="), "{words}");
    }
    assert!(highlighted > 100, "the stub highlighted almost nothing");
}

/// The byte-exact check (P-107): an answer that would lose, repeat or split
/// a byte leaves the block exactly as it is without a highlighter, recorded
/// as not exact. A highlighter returns ranges, so it cannot alter a byte;
/// these are the ways ranges go wrong.
#[test]
fn a_highlighter_that_drops_or_alters_bytes_leaves_the_block_plain() {
    let wrong: [(&str, Answer); 5] = [
        ("overlapping", |t| vec![span(0, 3), span(2, t.len())]),
        ("past the end", |t| vec![span(0, t.len() + 1)]),
        ("backwards", |t| vec![span(4, t.len()), span(0, 2)]),
        ("inside a character", |_| vec![span(0, 1), span(1, 2)]),
        ("reversed range", |t| vec![span(t.len(), 0)]),
    ];
    let pages = [page("p", &block("sh", "é = 1; echo \"x\""))];
    let plain = derive(&pages, None);
    for (name, f) in wrong {
        let site = derive(&pages, Some(Arc::new(Wrong(f))));
        assert_eq!(site.pages["p"].html, plain.pages["p"].html, "{name}");
        assert!(!site.pages["p"].html.contains("org-code-"), "{name}");
        assert_eq!(
            site.plain_blocks.iter().collect::<Vec<_>>(),
            [&PlainBlock {
                page: "p".into(),
                language: Some("sh".into()),
                reason: Plain::NotExact,
            }],
            "{name}"
        );
    }
}

/// D21: a block over the cap is plain without the highlighter ever seeing
/// it; a block exactly at the cap is highlighted.
#[test]
fn a_block_over_the_size_cap_is_plain_and_one_at_it_is_highlighted() {
    // The value is a measurement's conclusion: see its doc before changing.
    assert_eq!(MAX_HIGHLIGHT_BYTES, 32 * 1024);
    for (len, highlighted) in [
        (MAX_HIGHLIGHT_BYTES, true),
        (MAX_HIGHLIGHT_BYTES + 1, false),
    ] {
        let h = Arc::new(Everything::default());
        let pages = [page("p", &block("sh", &"z".repeat(len)))];
        let site = derive(&pages, Some(h.clone()));
        let html = &site.pages["p"].html;
        assert_eq!(block_texts(html), ["z".repeat(len)], "{len}");
        assert_eq!(html.contains("org-code-keyword"), highlighted, "{len}");
        let asked = h.0.lock().unwrap().clone();
        let too_large = PlainBlock {
            page: "p".into(),
            language: Some("sh".into()),
            reason: Plain::TooLarge,
        };
        if highlighted {
            assert_eq!(asked, [len]);
            assert!(site.plain_blocks.is_empty());
        } else {
            assert_eq!(asked, Vec::<usize>::new(), "the highlighter saw it");
            assert!(site.plain_blocks.contains(&too_large));
        }
    }
}

/// P-108: unknown, typo and undeclared languages are plain and exact, the
/// page still publishes, and each is recorded by page and language, once.
#[test]
fn unknown_undeclared_and_typo_languages_are_plain_exact_and_recorded() {
    let body = [
        block("pyhton", "def f(): return 1"),
        block("emacs-lisp", "(setq x 1)"),
        block("emacs-lisp", "(setq y 2)"),
        "#+begin_src\nno language at all\n#+end_src\n".to_string(),
        block("python", "def f(): return 1"),
    ]
    .concat();
    let pages = [page("p", &body), page("q", &block("nope", "x"))];
    let plain = derive(&pages, None);
    let site = derive(&pages, Some(Arc::new(Words)));
    assert!(site.withheld.is_empty(), "{:?}", site.withheld);
    let (html, want) = (&site.pages["p"].html, &plain.pages["p"].html);
    assert_eq!(block_texts(html), block_texts(want));
    // Only the python block is highlighted.
    assert_eq!(html.matches("org-code-").count(), 4, "{html}");
    let rec = |page: &str, language: Option<&str>, reason| PlainBlock {
        page: page.into(),
        language: language.map(str::to_string),
        reason,
    };
    let got: Vec<PlainBlock> = site.plain_blocks.iter().cloned().collect();
    let mut want = vec![
        rec("p", Some("emacs-lisp"), Plain::Unknown),
        rec("p", Some("pyhton"), Plain::Unknown),
        rec("p", None, Plain::Undeclared),
        rec("q", Some("nope"), Plain::Unknown),
    ];
    want.sort();
    assert_eq!(got, want);
    // No highlighter: nothing highlighted, nothing recorded.
    assert!(plain.plain_blocks.is_empty());
    assert!(!plain.pages["p"].html.contains("org-code-"));
}

/// D19: a block's text that spells markup stays text, highlighted or plain.
#[test]
fn markup_in_a_block_is_inert_highlighted_or_plain() {
    let hostile = "</code><script>alert(1)</script></pre><p class=\"mae-x\">&amp;";
    let pages = [page("p", &block("sh", hostile))];
    let highlighters: [Option<Arc<dyn Highlighter>>; 3] = [
        None,
        Some(Arc::new(Words)),
        Some(Arc::new(Everything::default())),
    ];
    for h in highlighters {
        let on = h.is_some();
        let html = derive(&pages, h).pages["p"].html.clone();
        assert!(!html.to_lowercase().contains("<script"), "{html}");
        assert!(!html.contains("<p class"), "{html}");
        assert_eq!(block_texts(&html), [hostile], "{html}");
        assert_eq!(html.contains("org-code-"), on, "{html}");
    }
}

/// The sanitiser admits the nine token classes on `span` only, and no other
/// `org-code-` class anywhere.
#[test]
fn the_token_classes_pass_the_sanitiser_on_spans_only() {
    for role in CodeRole::ALL {
        let c = role.class();
        let kept = format!(r#"<code><span class="{c}">x</span></code>"#);
        assert_eq!(sanitize_fragment(&kept), kept, "{c}");
        for tag in ["p", "code", "pre", "a", "div"] {
            let out = sanitize_fragment(&format!(r#"<{tag} class="{c}">x</{tag}>"#));
            assert!(!out.contains("org-code"), "{tag}: {out}");
        }
    }
    // `org-code-` is reserved for tokens; `org-code` alone is just another
    // `org-` class, which the allow-list admits as it admits them all.
    for bogus in ["org-code-bogus", "org-code-", "org-code-keyword-x"] {
        let out = sanitize_fragment(&format!(r#"<span class="{bogus} org-link">x</span>"#));
        assert_eq!(out, r#"<span class="org-link">x</span>"#, "{bogus}");
    }
}

/// P-105: blocks are found wherever mae-org renders them: under nested
/// headlines (what the reader folds), in list items and quotes.
#[test]
fn a_block_anywhere_in_the_page_is_highlighted() {
    let src = block("sh", "echo deep");
    let body = format!(
        "* One\n** Two\n*** Three\n{src}\n- item\n  #+begin_src sh\n  echo listed\n  #+end_src\n\n#+begin_quote\n{src}#+end_quote\n"
    );
    let site = derive(&[page("p", &body)], Some(Arc::new(Words)));
    let html = &site.pages["p"].html;
    assert_eq!(html.matches("<pre class=\"org-src\">").count(), 3, "{html}");
    assert_eq!(
        html.matches(r#"<span class="org-code-keyword">echo</span>"#)
            .count(),
        3,
        "{html}"
    );
}
