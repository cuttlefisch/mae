//! Issue #942 / ADR-112 Fact 3: the editor's org export publishes only what
//! Org's own export shows. Each test is written for the attacker's case (the
//! content that must NOT reach the output) and checks BOTH formats, since the
//! HTML and Markdown writers read the same rendered tree and a leak in either
//! is a leak.
//!
//! Mutants: `mutants/export.toml`.

use mae_export::document::{export_document, export_subtree, SubtreeError};
use mae_export::html::to_html_page;
use mae_export::markdown::to_markdown;

/// Both outputs for `src`.
fn both(src: &str) -> [(&'static str, String); 2] {
    let e = export_document(src);
    [("html", to_html_page(&e)), ("markdown", to_markdown(&e))]
}

fn assert_absent(src: &str, needles: &[&str]) {
    for (format, out) in both(src) {
        for needle in needles {
            assert!(
                !out.contains(needle),
                "{format} export published {needle:?}:\n{out}"
            );
        }
    }
}

fn assert_present(src: &str, needles: &[&str]) {
    for (format, out) in both(src) {
        for needle in needles {
            assert!(
                out.contains(needle),
                "{format} export lost {needle:?}:\n{out}"
            );
        }
    }
}

/// The issue's own fixture: a `:noexport:` subtree, a COMMENT subtree, and
/// comment and verse blocks. None of the hidden text, and none of the block
/// delimiters, may appear; the visible text around them must.
const LEAKY: &str = "\
#+TITLE: Leak check

Visible intro.

# SECRET-LINE-COMMENT

#+begin_comment
SECRET-COMMENT-BLOCK
#+end_comment

#+begin_verse
  Verse line one
    verse line two
#+end_verse

* Public heading
Public body.
:LOGBOOK:
- SECRET-LOGBOOK
:END:

** Private child :noexport:
SECRET-NOEXPORT-CHILD-BODY
*** SECRET-NOEXPORT-GRANDCHILD
deep secret

* Hidden heading SECRET-NOEXPORT-TITLE :noexport:
SECRET-NOEXPORT-BODY

#+begin_src sh
echo SECRET-NOEXPORT-CODE
#+end_src

* COMMENT SECRET-COMMENT-TITLE
SECRET-COMMENT-SUBTREE-BODY
#+TITLE: SECRET-COMMENTED-TITLE-KEYWORD

* Tail
Tail body.
";

#[test]
fn noexport_and_comment_content_is_absent_in_both_formats() {
    assert_absent(
        LEAKY,
        &[
            "SECRET-NOEXPORT-TITLE",
            "SECRET-NOEXPORT-BODY",
            "SECRET-NOEXPORT-CODE",
            "SECRET-NOEXPORT-CHILD-BODY",
            "SECRET-NOEXPORT-GRANDCHILD",
            "Private child",
            "SECRET-COMMENT-TITLE",
            "SECRET-COMMENT-SUBTREE-BODY",
            "SECRET-COMMENT-BLOCK",
            "SECRET-LINE-COMMENT",
            "SECRET-LOGBOOK",
            // A keyword in a COMMENT subtree is not a setting (Org reads
            // none from there): it must not become the page title.
            "SECRET-COMMENTED-TITLE-KEYWORD",
        ],
    );
    // The selective half: what is not hidden is still there, so the test
    // fails if an "export" publishes nothing at all.
    assert_present(
        LEAKY,
        &[
            "Visible intro.",
            "Public heading",
            "Public body.",
            "Tail body.",
            "Leak check",
        ],
    );
}

#[test]
fn block_delimiters_never_reach_the_output_as_text() {
    let src = "\
#+begin_verse
  Verse line one
    verse line two
#+end_verse

#+begin_center
Centered text
#+end_center

#+begin_details Click here
Special block text
#+end_details

#+BEGIN: clocktable :scope file
Dynamic block text
#+END:

#+begin_comment
SECRET-IN-COMMENT
#+end_comment
";
    assert_absent(
        src,
        &[
            "begin_",
            "end_",
            "BEGIN",
            "END:",
            "clocktable",
            "SECRET-IN-COMMENT",
        ],
    );
    assert_present(
        src,
        &[
            "Verse line one",
            "Centered text",
            "Special block text",
            "Dynamic block text",
        ],
    );
    // Verse is verse, not a paragraph of its source lines: its lines are
    // broken where the source breaks them.
    let e = export_document(src);
    let html = to_html_page(&e);
    assert!(
        html.contains("<p class=\"org-verse\">Verse line one<br>"),
        "{html}"
    );
    let md = to_markdown(&e);
    assert!(
        md.contains("Verse line one\\\n"),
        "a verse line ends in a hard break:\n{md}"
    );
}

#[test]
fn exclude_and_select_tags_are_honoured() {
    // Org's own keyword replaces the default `noexport`.
    let src = "\
#+EXCLUDE_TAGS: private
* Kept :noexport:
kept body
* Gone :private:
SECRET-PRIVATE
";
    assert_absent(src, &["SECRET-PRIVATE", "Gone"]);
    assert_present(src, &["kept body"]);

    // The pre-Org-8 spelling MAE's exporter honoured: its tags join
    // `noexport`, so such a file hides at least what it hid before.
    let legacy = "\
#+EXPORT_EXCLUDE_TAGS: secret
* Gone :secret:
SECRET-LEGACY-EXCLUDED
* Also gone :noexport:
SECRET-STILL-NOEXPORT
* Kept
kept body
";
    assert_absent(legacy, &["SECRET-LEGACY-EXCLUDED", "SECRET-STILL-NOEXPORT"]);
    assert_present(legacy, &["kept body"]);

    // Selection, as Org's exporter does it: with an `:export:` headline,
    // only selected subtrees (and their ancestors' headings) are shown, and
    // the text before the first headline is not.
    let select = "\
SECRET-ZEROTH-SECTION
* Chosen :export:
chosen body
* Not chosen
SECRET-UNSELECTED
";
    assert_absent(select, &["SECRET-ZEROTH-SECTION", "SECRET-UNSELECTED"]);
    assert_present(select, &["chosen body"]);

    let select_kw = "\
#+SELECT_TAGS: pick
* Picked :pick:
picked body
* Other
SECRET-NOT-PICKED
";
    assert_absent(select_kw, &["SECRET-NOT-PICKED"]);
    assert_present(select_kw, &["picked body"]);

    let legacy_select = "\
#+EXPORT_SELECT_TAGS: pick
* Picked :pick:
picked body
* Other
SECRET-NOT-PICKED-LEGACY
";
    assert_absent(legacy_select, &["SECRET-NOT-PICKED-LEGACY"]);
    assert_present(legacy_select, &["picked body"]);
}

/// Babel's `:exports`, resolved as Org resolves it; the existing results are
/// rendered, nothing is evaluated.
#[test]
fn src_block_exports_none_code_results_both() {
    let block = |exports: &str, tag: &str| {
        format!(
            "#+begin_src sh :exports {exports}\necho CODE-{tag}\n#+end_src\n\n\
             #+RESULTS:\n: RESULT-{tag}\n"
        )
    };
    let cases = [
        ("none", false, false),
        ("code", true, false),
        ("results", false, true),
        ("both", true, true),
        // The last of the four words in a value wins, so this hides the
        // block: a writer who reads only the first word publishes its code.
        ("code none", false, false),
        ("none results", false, true),
    ];
    for (exports, code, results) in cases {
        let src = block(exports, exports);
        let code_text = format!("CODE-{exports}");
        let result_text = format!("RESULT-{exports}");
        for (format, out) in both(&src) {
            assert_eq!(
                out.contains(&code_text),
                code,
                ":exports {exports}: code shown in {format}?\n{out}"
            );
            assert_eq!(
                out.contains(&result_text),
                results,
                ":exports {exports}: results shown in {format}?\n{out}"
            );
        }
    }
}

/// Raw HTML is a security surface (stored XSS on whoever opens the page):
/// no export emits an HTML export block, `#+HTML:` line or HTML snippet,
/// raw or escaped, and a Markdown export block (whose content a Markdown
/// renderer would pass through as HTML) is not emitted either.
#[test]
fn raw_html_and_markdown_export_content_is_never_emitted() {
    let src = "\
#+HTML: <script>alert('kw')</script>
Text @@html:<img src=x onerror=alert(2)>@@ here.

#+begin_export html
<script>alert(1)</script>
#+end_export

#+begin_export markdown
<iframe src=evil></iframe>
#+end_export

After.
";
    assert_absent(src, &["<script", "alert", "onerror", "<iframe", "evil"]);
    assert_present(src, &["After."]);
    // Counted, so the editor can say they were left out.
    assert_eq!(export_document(src).raw_html, 2);
}

/// Hostile metadata cannot inject markup into the page shell (audit
/// #596.3), nor break out of the Markdown front matter.
#[test]
fn hostile_metadata_is_escaped_in_the_shell_and_the_front_matter() {
    let payload = r#""><script>alert(1)</script><span x=""#;
    let src = format!(
        "#+TITLE: {payload}\n#+AUTHOR: {payload}\n#+LANGUAGE: {payload}\n\n* H :{}:\n\n\
         #+begin_src {payload}\nx\n#+end_src\n",
        "tag"
    );
    let e = export_document(&src);
    let html = to_html_page(&e);
    assert!(!html.contains("<script"), "{html}");
    assert!(
        html.contains(r#"lang="&quot;&gt;&lt;script&gt;"#),
        "the lang attribute must be escaped:\n{html}"
    );
    assert!(html.contains("&lt;script&gt;"), "{html}");

    // In the Markdown, the metadata is data in the front matter (one
    // JSON-quoted scalar per key), never body text.
    let md = to_markdown(&e);
    let body = md.splitn(3, "---\n").nth(2).unwrap_or_default();
    assert!(!body.contains("<script"), "{md}");
    assert!(
        md.contains("title: \"\\\"><script>"),
        "the title must be one quoted scalar:\n{md}"
    );
    // A title that tries to end the front matter and start a heading stays
    // one quoted scalar on its own line.
    let e = export_document("#+TITLE: x\"\n---\n# pwned\n\nBody.\n");
    let md = to_markdown(&e);
    let front: Vec<&str> = md.lines().take_while(|l| !l.is_empty()).collect();
    assert_eq!(front.first(), Some(&"---"), "{md}");
    assert!(
        front.iter().filter(|l| **l == "---").count() == 2,
        "front matter must be exactly one block:\n{md}"
    );
}

/// Ordinary content still exports: headings, lists, tables, links, and src
/// blocks byte for byte (Markdown's fence outgrows any backtick run inside).
#[test]
fn ordinary_content_round_trips() {
    let code = "fn main() {\n    let s = \"```\";\n\tprintln!(\"{s} *not* [md]\");\n}";
    let src = format!(
        "#+TITLE: Doc\n\n* First\nSee [[https://example.com/a_b][the *site*]].\n\n\
         - one\n- two\n  1. nested\n\n\
         | Name | Value |\n|------+-------|\n| a    | 1     |\n\n\
         #+begin_src rust\n{code}\n#+end_src\n\n** Second\nText.\n"
    );
    let e = export_document(&src);

    let html = to_html_page(&e);
    for needle in [
        "<h1 class=\"title\">Doc</h1>",
        "<h1 id=\"h-first\">First</h1>",
        "<h2 id=\"h-second\">Second</h2>",
        "<a class=\"org-link\" href=\"https://example.com/a_b\">the <strong>site</strong></a>",
        "<li><p>one</p></li>",
        "<th scope=\"col\">Name</th>",
        "<td>a</td>",
        "<code class=\"language-rust\">",
        "<a href=\"#h-first\">First</a>",
    ] {
        assert!(html.contains(needle), "html lacks {needle:?}:\n{html}");
    }

    let md = to_markdown(&e);
    for needle in [
        "title: \"Doc\"",
        "# First",
        "## Second",
        "[the **site**](https://example.com/a_b)",
        "- one\n- two\n  1. nested",
        "| Name | Value |\n| --- | --- |\n| a | 1 |",
    ] {
        assert!(md.contains(needle), "markdown lacks {needle:?}:\n{md}");
    }
    let fenced = format!("````rust\n{code}\n````");
    assert!(
        md.contains(&fenced),
        "the src block must be byte-exact inside a longer fence:\n{md}"
    );
}

#[test]
fn markdown_text_cannot_become_markup() {
    let src = "\
A <script>alert(1)</script> | pipe & [not](a link) *star* _u_ `tick` # hash.

- 1. not an ordered list
";
    let md = to_markdown(&export_document(src));
    // Every `<` is escaped: no tag survives as one.
    assert_eq!(md.matches('<').count(), md.matches("\\<").count(), "{md}");
    assert!(md.contains("\\<script\\>"), "{md}");
    assert!(md.contains("\\[not\\](a link)"), "{md}");
    assert!(md.contains("\\| pipe \\&"), "{md}");
    assert!(md.contains("\\`tick\\`"), "{md}");
    assert!(md.contains("- 1\\. not an ordered list"), "{md}");
}

// --- subtree export ---------------------------------------------------------

const TREE: &str = "\
#+TITLE: Tree
#+PROPERTY: header-args :exports none

* Alpha
alpha body[fn:a]
** Alpha child
alpha child body
** Alpha secret :noexport:
SECRET-ALPHA-CHILD
#+begin_src sh
echo SECRET-INHERITED-NONE
#+end_src
* Beta
beta body[fn:b]
#+begin_src sh
echo SECRET-INHERITED-NONE
#+end_src
* Gamma :noexport:
SECRET-GAMMA
** Gamma child
SECRET-GAMMA-CHILD
* COMMENT Delta
SECRET-DELTA
* Beta
second beta body

[fn:a] Footnote of alpha.
[fn:b] SECRET-FOOTNOTE-OF-BETA.
";

fn offset_of(src: &str, needle: &str) -> usize {
    src.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture"))
}

fn subtree_outputs(src: &str, at: &str) -> [(&'static str, String); 2] {
    let e = export_subtree(src, offset_of(src, at))
        .unwrap_or_else(|err| panic!("subtree at {at:?} refused: {err}"));
    [("html", to_html_page(&e)), ("markdown", to_markdown(&e))]
}

#[test]
fn subtree_export_shows_its_subtree_only() {
    for (format, out) in subtree_outputs(TREE, "alpha body") {
        for needle in [
            "alpha body",
            "Alpha child",
            "alpha child body",
            "Footnote of alpha.",
        ] {
            assert!(out.contains(needle), "{format} lacks {needle:?}:\n{out}");
        }
        for needle in [
            "beta body",
            "Beta",
            "Gamma",
            "SECRET-ALPHA-CHILD",
            "Alpha secret",
            "SECRET-INHERITED-NONE",
            "SECRET-FOOTNOTE-OF-BETA",
            "SECRET-GAMMA",
            "SECRET-DELTA",
        ] {
            assert!(
                !out.contains(needle),
                "{format} published {needle:?}:\n{out}"
            );
        }
    }
    // A nested heading exports itself, not its parent.
    for (format, out) in subtree_outputs(TREE, "alpha child body") {
        assert!(out.contains("alpha child body"), "{format}:\n{out}");
        assert!(!out.contains("alpha body"), "{format}:\n{out}");
    }
}

#[test]
fn subtree_export_of_a_repeated_title_takes_the_right_one() {
    // Two headings titled "Beta": the second's anchor is `h-beta--2`.
    for (format, out) in subtree_outputs(TREE, "second beta body") {
        assert!(out.contains("second beta body"), "{format}:\n{out}");
        assert!(!out.contains("beta body["), "{format}:\n{out}");
        assert!(!out.contains("Footnote"), "{format}:\n{out}");
    }
    for (format, out) in subtree_outputs(TREE, "beta body[") {
        assert!(!out.contains("second beta body"), "{format}:\n{out}");
    }
}

#[test]
fn subtree_export_refuses_a_hidden_heading() {
    for at in [
        "SECRET-GAMMA",
        "SECRET-GAMMA-CHILD",
        "Gamma child",
        "SECRET-DELTA",
        "SECRET-ALPHA-CHILD",
    ] {
        let got = export_subtree(TREE, offset_of(TREE, at)).map(|e| to_html_page(&e));
        assert_eq!(
            got,
            Err(SubtreeError::NotExported),
            "subtree at {at:?} must be refused"
        );
    }
    assert_eq!(export_subtree(TREE, 0).err(), Some(SubtreeError::NoHeading));
}

#[test]
fn subtree_export_refuses_when_a_macro_moves_the_headings() {
    // A macro that makes a heading `:noexport:` hides it from the rendered
    // page while the source still has it: positions no longer line up, and
    // guessing would publish a neighbour.
    let src = "\
#+MACRO: hide :noexport:
* One {{{hide}}}
SECRET-ONE
* Two
two body
";
    let got = export_subtree(src, offset_of(src, "two body"));
    assert_eq!(got.err(), Some(SubtreeError::Unplaced));
    assert_absent(src, &["SECRET-ONE"]);
}

#[test]
fn checkboxes_become_task_list_items() {
    let md = to_markdown(&export_document("- [X] done\n- [ ] open\n- [-] partly\n"));
    assert!(
        md.contains("- [x] done\n- [ ] open\n- `[-]` partly"),
        "{md}"
    );
}

/// Page metadata comes only from keywords the export shows: a `#+TITLE:` or
/// `#+AUTHOR:` inside a `:noexport:` subtree, or outside the selected tags,
/// must not become the page title, author or front matter (independent
/// review of #942: the body was withheld but the metadata path read every
/// keyword outside COMMENT subtrees).
#[test]
fn page_metadata_inside_a_withheld_subtree_is_not_published() {
    let src = "\
#+TITLE: Public title
* Notes :noexport:
#+TITLE: SECRET-TITLE
#+AUTHOR: SECRET-AUTHOR
#+HTML: <b>SECRET-HTML</b>
* Visible
public body
";
    assert_absent(src, &["SECRET-TITLE", "SECRET-AUTHOR", "SECRET-HTML"]);
    assert_present(src, &["Public title", "public body"]);
    assert_eq!(
        export_document(src).raw_html,
        0,
        "an #+HTML: line in a withheld subtree was counted as omitted output"
    );

    let selected = "\
#+SELECT_TAGS: pub
* Pub :pub:
public body
* Other
#+TITLE: SECRET-OUTSIDE-SELECT
other body
";
    assert_absent(selected, &["SECRET-OUTSIDE-SELECT", "other body"]);
}
