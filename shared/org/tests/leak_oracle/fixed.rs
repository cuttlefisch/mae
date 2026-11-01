//! Fixed cases: the confirmed leaks (a)–(e) as hand-planted documents, and
//! the consumer fixtures, each with the Emacs 30.2 behaviour measured for
//! it. A `«…»` token plants a marker: `«leaf»` (strict), `«meta:leaf»`
//! (allowed in metadata), `«href:leaf»` (allowed in hrefs); `«leaf=k»`
//! reuses the marker labelled `k`.

use super::gen::{Allow, Doc, Site, Sites};
use super::view::marker;
use std::collections::BTreeMap;

/// (name, document). The comment above each is Emacs's measured output.
pub const CASES: &[(&str, &str)] = &[
    // Emacs: the block opened by {{{b}}} runs to the literal #+end_comment
    // ({{{e}}} inside it is never expanded): only "after" shows.
    ("known-a-macro-comment-pair", "#+MACRO: b #+begin_comment\n#+MACRO: e #+end_comment\n\n{{{b}}}\n{{{e}}}\n«a-secret»\n#+end_comment\nafter «a-visible»\n"),
    // Emacs: <h2>Foo</h2>; the word inside the hidden src block is nowhere.
    ("known-b-macro-in-heading-anchor", "#+MACRO: o src_sh[:exports none]{echo\n\n* Foo {{{o}}} «b-secret»}\nbody «b-visible»\n"),
    // Emacs: <a id="orgXXXXXXX"></a>, link text "No description for this
    // link"; the target name appears nowhere.
    ("known-c-target-name", "Text <<«c-target=t»>> here: [[«c-target=t»]] and [[«c-target=t»][go «c-visible»]].\n"),
    // Emacs: "A citation (??, ????) in the text." (no key).
    ("known-d-citation-key", "A citation [cite:@«d-cite-key»] in the text «d-visible».\n"),
    // Emacs (the id path stood in for): <a href="https://kb.invalid/">typed</a>.
    ("known-e-rel-value", ":PROPERTIES:\n:ID: aaaa\n:END:\n[[id:aaaa?rel=«e-link-rel»&w=1e9;x][typed «e-visible»]]\n"),
    // Consumer 01. Emacs: <a id="orgXXXXXXX"> and href="#orgXXXXXXX"; the
    // name appears nowhere.
    ("consumer-01-target-name-in-id", "#+title: Target name in id\nText with a hidden target <<«target-name=s»>> here, and a link to it: [[«target-name=s»][go to target]].\n"),
    // Consumer 02. Emacs: user-error "Definition not found for footnote c".
    ("consumer-02-footnote-def-in-comment-subtree", "#+title: Footnote defined only in a COMMENT subtree\nA reference[fn:c] here.\n\n* COMMENT Hidden\n[fn:c] The definition «fn-definition», inside a comment subtree.\n"),
    // Consumer 03. Emacs: descriptions exported as written (a file:
    // description becomes an <img>).
    ("consumer-03-file-link-in-description", "#+title: file: text inside a link description\n- [[https://example.org][desc file:/home/usr/«desc-plain»]]\n- [[https://example.org][file:/home/usr/«desc-img».png]]\n- [[https://example.org][desc <file:/home/usr/«desc-angle»> x]]\n"),
    // Consumer 04. Emacs: the outer link ends at the first ]]; the path is
    // literal text.
    ("consumer-04-nested-link-in-description", "#+title: Nested link inside a description\nSee [[https://example.org][see [[file:/home/usr/«nested-file»][x]] y]] after.\n"),
    // Consumer 05. Emacs: "A /home/usr/SECprop C" (the value, through the
    // macro).
    ("consumer-05-property-macro-exposes-drawer", "#+title: property macro exposes drawer values\n* Heading\n:PROPERTIES:\n:SRC: /home/usr/«property»\n:END:\nA {{{property(SRC)}}} C\n"),
    // Consumer 06. Emacs: the keyword's value, although it is set in a
    // :noexport: subtree.
    ("consumer-06-keyword-macro-from-noexport", "#+title: keyword macro reads a keyword set inside a noexport subtree\nValue: {{{keyword(SECKW)}}}\n\n* Hidden                                                         :noexport:\n#+SECKW: «keyword-value»\n"),
    // Consumer 07. Emacs: all three are links (href="https:SEC…").
    ("consumer-07-https-without-slashes", "#+title: https: without //\n- [[https:«web-path»][no slashes]]\n- [[https:/«web-path»][one slash]]\n- [[https:///«web-path»][three slashes]]\n"),
    // Consumer 08. Emacs: "A citation (??, ????) in the text."
    ("consumer-08-citation-key", "#+title: Citation key\nA citation [cite:@«cite-key»] in the text.\n"),
    // Consumer 09. Emacs (path stood in for): the description only. MAE
    // slugs the fragment into the href by design.
    ("consumer-09-fragment-on-id-link", "#+title: Fragment text on an id link (MAE KB semantics)\n:PROPERTIES:\n:ID: aaaaaaaa-0000-0000-0000-000000000001\n:END:\nLink: [[id:aaaaaaaa-0000-0000-0000-000000000001#/home/usr/«href:id-fragment»][self with fragment]]\n"),
    // Consumer 10. Emacs (path stood in for): the description only.
    ("consumer-10-rel-value-not-a-word", "#+title: rel value that is not a link type (MAE ADR-030 typed links)\n:PROPERTIES:\n:ID: aaaaaaaa-0000-0000-0000-000000000002\n:END:\n[[id:aaaaaaaa-0000-0000-0000-000000000002?rel=«link-rel»&w=1e9;x][typed]]\n"),
    // Consumer 11. Emacs: link text "No description for this link"; the
    // name appears nowhere.
    ("consumer-11-target-link-no-description", "#+title: Target link with no description\nHidden target <<«target-name=b»>> here; a link to it with no description: [[«target-name=b»]].\n"),
];

/// Plants the `«…»` tokens of `text`.
pub fn plant(name: &str, text: &'static str) -> Doc {
    let mut sites = Sites::new();
    let mut labels: BTreeMap<String, u32> = BTreeMap::new();
    let mut out = String::new();
    let mut rest = text;
    let mut next = 1;
    while let Some(i) = rest.find('«') {
        out.push_str(&rest[..i]);
        let after = &rest[i + '«'.len_utf8()..];
        let end = after.find('»').expect("closed token");
        let n = token(&after[..end], &mut sites, &mut labels, &mut next);
        out.push_str(&marker(n));
        rest = &after[end + '»'.len_utf8()..];
    }
    out.push_str(rest);
    Doc {
        name: name.to_string(),
        text: out,
        sites,
    }
}

fn token(
    t: &'static str,
    sites: &mut Sites,
    labels: &mut BTreeMap<String, u32>,
    next: &mut u32,
) -> u32 {
    let (spec, label) = t.split_once('=').map_or((t, None), |(s, l)| (s, Some(l)));
    if let Some(n) = label.and_then(|l| labels.get(l)) {
        return *n;
    }
    let (allow, leaf) = match spec.split_once(':') {
        Some(("meta", l)) => (Allow::Meta(None), l),
        Some(("href", l)) => (Allow::Href, l),
        _ => (Allow::Strict, spec),
    };
    let n = *next;
    *next += 1;
    let path = "fixed".to_string();
    sites.insert(
        n,
        Site {
            leaf,
            path,
            allow,
            exposure: None,
        },
    );
    if let Some(l) = label {
        labels.insert(l.to_string(), n);
    }
    n
}

/// Every fixed case, planted.
pub fn docs() -> Vec<Doc> {
    CASES.iter().map(|(n, t)| plant(n, t)).collect()
}
