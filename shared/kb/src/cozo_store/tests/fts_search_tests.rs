//! FTS retrieval-correctness property tests for `nodes:fts` (see
//! `NODES_FTS_DDL` in `shared/kb/src/cozo_store/schema.rs`): every term
//! present in a node's title or body must retrieve that node, case
//! variations must retrieve it too, a term unique to one node must rank it
//! first, and a term shared by several nodes must return every owner.
//!
//! Split out of `kb_store_impl_tests.rs` (that file's original, single-term
//! `fts_search_finds_nodes` "unicorn" test is kept here as a marker — see its
//! own comment — pointing at `fts_every_indexed_term_retrieves_its_node` as
//! the real oracle). Query-grammar edge cases, ranking under a realistic
//! multi-node corpus, and the update-lifecycle/raw-Tantivy tests live in the
//! sibling `fts_query_tests`.

use super::*;

#[test]
fn fts_search_finds_nodes() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "n1",
            "Quantum Physics",
            NodeKind::Note,
            "Entanglement is spooky.",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "n2",
            "Classical Mechanics",
            NodeKind::Note,
            "Newton was right.",
        ))
        .unwrap();

    let hits = store.fts_search("quantum", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "n1");

    // The above is the ORIGINAL form of this test, kept only as a marker. It
    // passed throughout the entire period `fts_search` was dropping terms,
    // because `quantum` is the FIRST word of the title and the defect welded
    // the title's LAST word to the body's FIRST word. Asserting one
    // hand-picked term is the "unicorn value" antipattern CLAUDE.md principle
    // #14 names. `fts_every_indexed_term_retrieves_its_node` below is the real
    // oracle — do not let this stub stand in for it.
    assert_eq!(
        store.fts_search("physics", 10).unwrap().len(),
        1,
        "the term the unicorn test never asked about"
    );
    assert_eq!(store.fts_search("entanglement", 10).unwrap().len(), 1);
}

/// Every node in the corpus, paired with the terms a user could reasonably type
/// to find it. Named cases (the `crates/core/src/grapheme.rs` nasty-corpus
/// shape) so a failure reports *which* linguistic mechanism broke, not an
/// inscrutable index.
struct FtsCase {
    name: &'static str,
    id: &'static str,
    title: &'static str,
    body: &'static str,
}

const FTS_CORPUS: &[FtsCase] = &[
    // The DECISIONS_FOR_REVIEW item-10 reproducer itself. `physics` (last title
    // token) and `entanglement` (first body token) are the pair the extractor
    // defect destroyed.
    FtsCase {
        name: "title_body_boundary",
        id: "concept:quantum",
        title: "Quantum Physics",
        body: "Entanglement is spooky.",
    },
    // Multi-word title, several tokens either side of the join.
    FtsCase {
        name: "long_multiword_title",
        id: "concept:alpha",
        title: "alpha beta gamma delta",
        body: "epsilon zeta eta theta",
    },
    // Case variation: indexed lowercase, queried in the author's casing.
    FtsCase {
        name: "mixed_case",
        id: "concept:case",
        title: "ScreamING SnakeCase TITLE",
        body: "MiXeD BodyText HERE",
    },
    // Punctuation adjacent to tokens, including a token-terminal period at the
    // join and an em-dash.
    FtsCase {
        name: "punctuation_heavy",
        id: "concept:punct",
        title: "Ends with period.",
        body: "Semicolons; commas, parens (inside) — dashes.",
    },
    // Non-ASCII: precomposed NFC accents, cedilla, umlaut.
    FtsCase {
        name: "latin1_accents_nfc",
        id: "concept:accents",
        title: "Café Naïve",
        body: "Ünicode École façade.",
    },
    // Non-ASCII, non-Latin: CJK + Kana, which `Simple` keeps as alphanumeric
    // runs rather than splitting per character.
    FtsCase {
        name: "cjk_and_kana",
        id: "concept:cjk",
        title: "日本語 テスト",
        body: "漢字 mixed with kanji.",
    },
    // Cyrillic + Greek, to prove the tokenizer is not ASCII-only.
    FtsCase {
        name: "cyrillic_greek",
        id: "concept:scripts",
        title: "Привет Мир",
        body: "Δοκιμή ελληνικά text.",
    },
    // Digits and alphanumeric mixes — `1` and `5` are separate tokens because
    // `.` splits, and `rfc2119` stays whole because it has no separator.
    FtsCase {
        name: "digits_and_alnum",
        id: "concept:versions",
        title: "Version 1.5 Notes",
        body: "See rfc2119 and utf8 for MUST.",
    },
    // Underscores are NOT alphanumeric, so `foo_bar` indexes as two tokens.
    FtsCase {
        name: "underscores_and_hyphens",
        id: "concept:idents",
        title: "well-known limits",
        body: "read-only mode; foo_bar baz.",
    },
    // Single-word title: the join lands between the only title token and the
    // only body token, the tightest version of the boundary case.
    FtsCase {
        name: "single_word_title",
        id: "concept:terse",
        title: "Terse",
        body: "Minimal",
    },
];

/// Tokenize the way cozo's `Simple` tokenizer + `Lowercase` filter do: split on
/// every non-alphanumeric character, lowercase the rest.
///
/// Deliberately written out here rather than shared with `fts_search`'s own
/// term-splitting — a test whose oracle is the code under test proves nothing.
fn expected_terms(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// THE property: for every node, every term appearing in its title or its body
/// retrieves that node. Not "some representative term" — every one.
///
/// This is the test the codebase needed and did not have. The extractor defect
/// (see `NODES_FTS_DDL`) made `fts_search` silently miss any term at the
/// title/body join, on the primary KB read path, for as long as it shipped —
/// and the previous single-term assertion passed the whole time.
#[test]
fn fts_every_indexed_term_retrieves_its_node() {
    let (_tmp, store) = make_store();
    for case in FTS_CORPUS {
        store
            .insert_node(&Node::new(case.id, case.title, NodeKind::Note, case.body))
            .unwrap();
    }
    // Larger than the corpus, so a miss is a genuine miss and never truncation.
    let limit = FTS_CORPUS.len() * 4;

    let mut failures: Vec<String> = Vec::new();
    for case in FTS_CORPUS {
        for (field, text) in [("title", case.title), ("body", case.body)] {
            for term in expected_terms(text) {
                let hits = match store.fts_search(&term, limit) {
                    Ok(h) => h,
                    Err(e) => {
                        failures.push(format!(
                            "[{}] {field} term {term:?} -> ERROR {e}",
                            case.name
                        ));
                        continue;
                    }
                };
                if !hits.iter().any(|h| h.id == case.id) {
                    failures.push(format!(
                        "[{}] {field} term {term:?} did not retrieve {} (got {:?})",
                        case.name,
                        case.id,
                        hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>()
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "terms present in a node's indexed text failed to retrieve it:\n  {}",
        failures.join("\n  ")
    );
}

/// Cozo's FTS query grammar reserves the UPPERCASE words `AND`/`OR`/`NOT`/
/// `NEAR` as boolean operators, and the negative lookahead that excludes them
/// (`fts_phrase_simple` in `cozoscript.pest`) is NOT anchored to a word
/// boundary — so any term merely *beginning* with one of them is unparseable.
/// `ANDROID`, `ORBIT`, `NEARBY` and `NOTES` are all rejected; their
/// lower/title-case forms are fine. See
/// `fts_query_syntax_characters_are_still_parse_errors`.
fn starts_with_fts_keyword(s: &str) -> bool {
    ["AND", "OR", "NOT", "NEAR"]
        .iter()
        .any(|kw| s.starts_with(kw))
}

/// The same property under case variation: a term typed in any casing must
/// retrieve the node, since the index applies a `Lowercase` filter.
#[test]
fn fts_retrieval_is_case_insensitive() {
    let (_tmp, store) = make_store();
    for case in FTS_CORPUS {
        store
            .insert_node(&Node::new(case.id, case.title, NodeKind::Note, case.body))
            .unwrap();
    }
    let limit = FTS_CORPUS.len() * 4;

    let mut failures: Vec<String> = Vec::new();
    for case in FTS_CORPUS {
        for text in [case.title, case.body] {
            for term in expected_terms(text) {
                for variant in [term.to_uppercase(), title_case(&term)] {
                    if variant == term || starts_with_fts_keyword(&variant) {
                        continue;
                    }
                    // Distinguish a miss from an error — `unwrap_or_default()`
                    // here would quietly launder a parse failure into "no
                    // results", which is the reporting failure that let the
                    // original defect hide.
                    match store.fts_search(&variant, limit) {
                        Ok(hits) if hits.iter().any(|h| h.id == case.id) => {}
                        Ok(hits) => failures.push(format!(
                            "[{}] {variant:?} did not retrieve {} (got {:?})",
                            case.name,
                            case.id,
                            hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>()
                        )),
                        Err(e) => failures.push(format!(
                            "[{}] {variant:?} errored: {}",
                            case.name,
                            e.to_string().lines().next().unwrap_or_default()
                        )),
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "case variants failed to retrieve their node:\n  {}",
        failures.join("\n  ")
    );
}

fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Finding everything is worthless if it ranks uselessly. A term unique to one
/// node must rank that node FIRST, not merely somewhere in the list — the
/// failure mode where a fix trades a silent miss for a useless ordering.
#[test]
fn fts_unique_terms_rank_their_own_node_first() {
    let (_tmp, store) = make_store();
    for case in FTS_CORPUS {
        store
            .insert_node(&Node::new(case.id, case.title, NodeKind::Note, case.body))
            .unwrap();
    }
    let limit = FTS_CORPUS.len() * 4;

    // Terms occurring in exactly one node across the whole corpus.
    let mut owner: std::collections::HashMap<String, Vec<&str>> = Default::default();
    for case in FTS_CORPUS {
        let mut seen: std::collections::HashSet<String> = Default::default();
        for text in [case.title, case.body] {
            for term in expected_terms(text) {
                if seen.insert(term.clone()) {
                    owner.entry(term).or_default().push(case.id);
                }
            }
        }
    }

    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for (term, ids) in &owner {
        if ids.len() != 1 {
            continue;
        }
        checked += 1;
        let hits = store.fts_search(term, limit).unwrap_or_default();
        match hits.first() {
            Some(top) if top.id == ids[0] => {}
            other => failures.push(format!(
                "unique term {term:?} should rank {} first, got {:?}",
                ids[0],
                other.map(|h| h.id.as_str())
            )),
        }
    }
    assert!(
        checked > 20,
        "expected a meaningful number of corpus-unique terms, checked only {checked}"
    );
    assert!(
        failures.is_empty(),
        "unique terms ranked the wrong node first:\n  {}",
        failures.join("\n  ")
    );
}

/// A term shared by several nodes must return ALL of them — the defect's other
/// face, where a node is missing from a legitimately multi-hit result set.
#[test]
fn fts_shared_terms_return_every_owner() {
    let (_tmp, store) = make_store();
    let shared = [
        ("s1", "Gravity Basics", "Newton discovered gravity early."),
        ("s2", "Gravity Advanced", "Einstein reframed gravity later."),
        ("s3", "Unrelated Topic", "Gravity appears here too."),
        ("s4", "No Mention", "Nothing relevant in this body."),
    ];
    for (id, title, body) in shared {
        store
            .insert_node(&Node::new(id, title, NodeKind::Note, body))
            .unwrap();
    }

    let ids: Vec<String> = store
        .fts_search("gravity", 50)
        .unwrap()
        .into_iter()
        .map(|h| h.id)
        .collect();
    for expect in ["s1", "s2", "s3"] {
        assert!(
            ids.iter().any(|i| i == expect),
            "{expect} contains 'gravity' but was not returned (got {ids:?})"
        );
    }
    assert!(
        !ids.iter().any(|i| i == "s4"),
        "s4 does not contain 'gravity' and must not be returned (got {ids:?})"
    );
}
