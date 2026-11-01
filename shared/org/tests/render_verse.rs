//! Verse blocks lose the indentation their lines share, as Org's export
//! does (`org-remove-indentation`; ADR-112 D3).
//!
//! Each case was exported by `emacs -Q --batch` (Emacs 30.2, Org 9.7.11);
//! `want` is the number of no-break spaces Emacs put at the start of each
//! line (`None` for a line Emacs left empty). Org re-indents with
//! `indent-tabs-mode` on, so a remaining indentation of 8 columns or more
//! comes out as tabs, one no-break space each; MAE gives one per column.
//! Those cases are marked (listed in `conformance/render-divergences.toml`).

use mae_org::{parse, render_html, semantics, ParseOptions, RenderOptions, Resolver};

/// The no-break spaces leading each line of the first verse in `doc`, and
/// `None` for an empty line.
fn verse_lines(doc: &str) -> Vec<Option<usize>> {
    let d = parse(doc, &ParseOptions::default());
    let html = render_html(
        &d,
        &semantics(&d),
        &RenderOptions::default(),
        &Resolver::default(),
    );
    let start = html.find("<p class=\"org-verse\">").expect("a verse") + 21;
    let body = &html[start..start + html[start..].find("</p>").expect("closed")];
    body.split("<br>\n")
        .map(|l| {
            let n = l.chars().take_while(|c| *c == '\u{a0}').count();
            (!l.is_empty()).then_some(n)
        })
        .collect()
}

#[test]
fn shared_indentation_is_removed_as_org_removes_it() {
    let cases: &[(&str, &[Option<usize>])] = &[
        (
            "#+begin_verse\n    a\n      b\n\n    c\n#+end_verse\n",
            &[Some(0), Some(2), None, Some(0)],
        ),
        (
            "- item\n  #+begin_verse\n    x\n     y\n  #+end_verse\n",
            &[Some(0), Some(1)],
        ),
        (
            "#+begin_verse\nflush\n  two\n#+end_verse\n",
            &[Some(0), Some(2)],
        ),
        (
            "#+begin_verse\n\ttab\n  sp\n#+end_verse\n",
            &[Some(6), Some(0)],
        ),
        (
            "#+begin_verse\n  *bold*\n    \\alpha z\n#+end_verse\n",
            &[Some(0), Some(2)],
        ),
        ("#+begin_verse\n   \n  a\n#+end_verse\n", &[None, Some(0)]),
        // DIVERGENCE: Emacs gives 7 and 1 (tabs from indent-tabs-mode).
        (
            "#+begin_verse\n\t\tx\n  y\n#+end_verse\n",
            &[Some(14), Some(0)],
        ),
        (
            "#+begin_verse\n\t\t\tp\n\t\tq\n#+end_verse\n",
            &[Some(8), Some(0)],
        ),
    ];
    for (doc, want) in cases {
        assert_eq!(verse_lines(doc), want.to_vec(), "{doc:?}");
    }
}
