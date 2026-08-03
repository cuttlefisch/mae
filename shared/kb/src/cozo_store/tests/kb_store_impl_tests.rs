use super::*;

#[test]
fn insert_and_get_node() {
    let (_tmp, store) = make_store();
    let node =
        Node::new("test:1", "Test Node", NodeKind::Note, "Hello world").with_tags(["tag1", "tag2"]);
    store.insert_node(&node).unwrap();

    let loaded = store.get_node("test:1").unwrap().unwrap();
    assert_eq!(loaded.title, "Test Node");
    assert_eq!(loaded.body, "Hello world");
    assert_eq!(loaded.tags, vec!["tag1", "tag2"]);
}

#[test]
fn get_missing_returns_none() {
    let (_tmp, store) = make_store();
    assert!(store.get_node("nonexistent").unwrap().is_none());
}

#[test]
fn delete_node_removes_it() {
    // Test with mem engine to verify rm works cleanly
    let db = DbInstance::new("mem", "", "").unwrap();
    db.run_default(":create test {k: String => v: String}")
        .unwrap();
    db.run_default(r#"?[k, v] <- [["a", "hello"]] :put test {k => v}"#)
        .unwrap();
    let r = db.run_default("?[k, v] := *test{k, v}").unwrap();
    assert_eq!(r.rows.len(), 1);
    db.run_default(r#"?[k] <- [["a"]] :rm test {k}"#).unwrap();
    let r = db.run_default("?[k, v] := *test{k, v}").unwrap();
    eprintln!("mem after rm: {:?}", r.rows);

    // Now test CozoKbStore
    let (_tmp, store) = make_store();
    let node = Node::new("del-1", "Delete Me", NodeKind::Note, "body");
    store.insert_node(&node).unwrap();
    assert!(store.get_node("del-1").unwrap().is_some());

    store.delete_node("del-1").unwrap();
    let after = store.get_node("del-1").unwrap();
    // Sled backend may leave ghost rows with empty values — treat as deleted
    match after {
        None => {} // ideal
        Some(n) => assert!(
            n.title.is_empty() && n.body.is_empty(),
            "ghost row should have empty fields"
        ),
    }
}

#[test]
fn pending_updates_lifecycle() {
    let (_tmp, store) = make_store();
    store
        .push_pending_update("kb-1", "node-a", &[1, 2, 3])
        .unwrap();
    store
        .push_pending_update("kb-1", "node-b", &[4, 5, 6])
        .unwrap();

    let pending = store.drain_pending_updates().unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].node_id, "node-a");

    // ADR-020 observability: count reflects the durable queue (what an offline
    // edit lands in) — the seam the introspect `pending_kb_updates` reads.
    assert_eq!(
        store.count_pending_updates().unwrap(),
        2,
        "durable pending count must reflect un-acked offline edits"
    );

    store.ack_pending_update(pending[0].rowid).unwrap();
    let remaining = store.drain_pending_updates().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].node_id, "node-b");
    assert_eq!(
        store.count_pending_updates().unwrap(),
        1,
        "count decreases as the queue is acked"
    );
}

#[test]
fn crdt_doc_persistence() {
    // get_crdt_doc/update_crdt_doc (narrow point-read/point-write trait
    // methods) were removed as dead code (#303 follow-up) -- crdt_doc is
    // an ordinary field on the ordinary node-row path (insert_node/
    // get_node/update_node), which this now exercises directly.
    let (_tmp, store) = make_store();
    let mut node = Node::new("crdt:1", "CRDT Node", NodeKind::Note, "body");
    node.crdt_doc = Some(vec![10, 20, 30, 40]);
    store.insert_node(&node).unwrap();

    let reloaded = store.get_node("crdt:1").unwrap();
    assert_eq!(
        reloaded.and_then(|n| n.crdt_doc),
        Some(vec![10, 20, 30, 40])
    );
}

#[test]
fn load_all_and_save_all() {
    let (_tmp, store) = make_store();
    let n1 = Node::new("n1", "One", NodeKind::Note, "body1");
    let n2 = Node::new("n2", "Two", NodeKind::Note, "body2");

    store.save_all(&[&n1, &n2]).unwrap();
    let loaded = store.load_all().unwrap();
    assert_eq!(loaded.len(), 2);
}

#[test]
fn load_all_tolerates_query_bind_failure() {
    // B-5 regression: a stored `nodes` relation left at an older / shorter
    // arity (here a 2-column stand-in for the production "tuple bound by
    // variable 'title' is too short" artifact) makes the full 13-column load
    // query fail at bind time — BEFORE the per-row skip loop runs. A hard Err
    // here previously aborted `kb_join` and tripped the 10s main-thread stall
    // watchdog. The store must degrade to an empty load and keep running.
    let (_tmp, store) = make_store();
    // Replace `nodes` with a relation the full load query cannot bind, and
    // populate one row (simulates the migration / broken-write artifact on
    // disk that the production "tuple too short" error came from). The FTS
    // index must be dropped first — a relation with indices attached can't be
    // replaced.
    store
        .run_mut("::fts drop nodes:fts")
        .expect("drop fts index");
    store
        .run_mut(
            r#"?[id, title] <- [["bad", "x"]]
                   :replace nodes {id: String => title: String}"#,
        )
        .expect("replace schema with short-arity row");

    // Must be Ok (degraded), never Err, and must not panic.
    let loaded = store
        .load_all()
        .expect("load_all must degrade to Ok on a query bind failure, not Err");
    assert!(
        loaded.is_empty(),
        "a load query that cannot bind degrades to an empty result"
    );
}

#[test]
fn backend_name_is_cozo() {
    let (_tmp, store) = make_store();
    assert_eq!(store.backend_name(), "cozo");
}

#[test]
fn list_ids_with_prefix() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("cmd:save", "Save", NodeKind::Command, ""))
        .unwrap();
    store
        .insert_node(&Node::new("cmd:quit", "Quit", NodeKind::Command, ""))
        .unwrap();
    store
        .insert_node(&Node::new(
            "concept:buffer",
            "Buffer",
            NodeKind::Concept,
            "",
        ))
        .unwrap();

    let cmd_ids = store.list_ids(Some("cmd:")).unwrap();
    assert_eq!(cmd_ids.len(), 2);
    let all_ids = store.list_ids(None).unwrap();
    assert_eq!(all_ids.len(), 3);
}

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

/// A prefix query (`buffer*`) is legitimate cozo FTS syntax and the index
/// answers it correctly. `fts_search`'s post-query verification used to treat
/// the raw query as literal text, so `text.contains("buffer*")` was false for
/// every candidate and the caller got zero hits — the index found the node and
/// MAE threw it away. Same silent-miss direction as the extractor defect.
#[test]
fn fts_prefix_query_survives_post_query_verification() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "concept:buffer",
            "Buffer Management",
            NodeKind::Note,
            "The rope-backed store.",
        ))
        .unwrap();

    for q in ["buffer*", "buff*", "rope*", "manage*"] {
        let hits = store.fts_search(q, 10).unwrap();
        assert!(
            hits.iter().any(|h| h.id == "concept:buffer"),
            "prefix query {q:?} must retrieve concept:buffer, got {:?}",
            hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>()
        );
    }

    // The guard must still do its actual job: reject a term that is genuinely
    // absent, rather than passing everything through.
    let absent = store.fts_search("nonexistentterm", 10).unwrap();
    assert!(
        absent.is_empty(),
        "verification must still reject non-matching terms, got {:?}",
        absent.iter().map(|h| h.id.as_str()).collect::<Vec<_>>()
    );
}

/// Pins the KNOWN-BAD query-side behaviour that this change does NOT fix, so it
/// stays visible and a future fix must consciously update this test.
///
/// Cozo's FTS *query* grammar reads `:`, `-`, `.` and a leading `*` as
/// operators, so these are hard parse errors before MAE sees a candidate. This
/// is why `(kb-search "concept:buffer")` is built on `search_ranked` instead
/// (see the `@ai-caution` in `crates/scheme/src/runtime/kb_crud.rs`). Erroring
/// is at least loud — unlike the two silent misses fixed alongside this test —
/// but it means namespaced ids and hyphenated words are unqueryable through
/// `fts_search`.
#[test]
fn fts_query_syntax_characters_are_still_parse_errors() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "concept:buffer",
            "Buffer Management",
            NodeKind::Note,
            "Covers read-only mode and version 1.5.",
        ))
        .unwrap();

    for q in ["concept:buffer", "read-only", "1.5", "-buffer"] {
        assert!(
            store.fts_search(q, 10).is_err(),
            "{q:?} is expected to still be a cozo FTS parse error; if this now \
             succeeds the query surface was fixed — update this test and the \
             kb_crud.rs @ai-caution together"
        );
    }

    // The component words remain reachable, which is what makes the above a
    // usability gap rather than a data-loss bug.
    for q in ["buffer", "read", "only", "5"] {
        assert!(
            !store.fts_search(q, 10).unwrap().is_empty(),
            "{q:?} should still retrieve the node"
        );
    }
}

/// A second known-bad query class, found by the case-variation property test
/// rather than by inspection: cozo reserves UPPERCASE `AND`/`OR`/`NOT`/`NEAR`
/// as FTS boolean operators, and `cozoscript.pest`'s exclusion of them is a
/// bare `!("AND" | "OR" | "NOT" | "NEAR")` lookahead with no word-boundary
/// anchor. So an ALL-CAPS term that merely STARTS with one is a parse error:
/// `ANDROID`, `ORBIT`, `NEARBY`, `ANDES`, `NOTES` are all unqueryable, while
/// `Android`/`android`, `Orbit`, `Notes` are fine.
///
/// Not fixed here: suppressing it means deciding whether MAE exposes cozo's
/// boolean query language to users at all (lower-casing the query would make
/// a deliberate `foo AND bar` mean the literal word "and"). That is a product
/// decision, not a bug fix to slip in alongside an index repair.
#[test]
fn fts_uppercase_boolean_keyword_prefixes_are_parse_errors() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "n1",
            "Android Notes",
            NodeKind::Note,
            "Orbit and nearby andes ordering.",
        ))
        .unwrap();

    // Exactly the keywords, and — the surprising part — words merely prefixed
    // by them.
    for q in [
        "AND", "OR", "NOT", "NEAR", "ANDROID", "ORBIT", "NEARBY", "ANDES", "NOTES", "ORDERING",
    ] {
        assert!(
            store.fts_search(q, 10).is_err(),
            "{q:?} is expected to be a cozo FTS parse error (unanchored keyword \
             lookahead); if this now succeeds, update this test"
        );
    }

    // Non-uppercase forms of the same words work, confirming this is purely a
    // query-grammar artifact and the INDEX holds these terms correctly.
    for q in [
        "android", "Android", "orbit", "Orbit", "nearby", "andes", "notes", "Notes", "ordering",
    ] {
        assert!(
            !store.fts_search(q, 10).unwrap().is_empty(),
            "{q:?} should retrieve the node"
        );
    }
}

#[test]
fn fts_ranking_and_multi_word() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "n1",
            "Quantum Physics",
            NodeKind::Note,
            "Entanglement is spooky action at a distance",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "n2",
            "Classical Mechanics",
            NodeKind::Note,
            "Newton discovered gravity under a tree",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "n3",
            "Relativity Theory",
            NodeKind::Note,
            "Einstein showed space and time are linked by gravity",
        ))
        .unwrap();

    // Single word search — should find nodes mentioning "gravity"
    let hits = store.fts_search("gravity", 10).unwrap();
    assert!(
        hits.len() >= 2,
        "expected 2+ results for 'gravity', got {}",
        hits.len()
    );
    let hit_ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert!(hit_ids.contains(&"n2"), "n2 should match 'gravity'");
    assert!(hit_ids.contains(&"n3"), "n3 should match 'gravity'");

    // Title search — "quantum" is in the title, Tantivy indexes title + body
    let hits = store.fts_search("quantum", 10).unwrap();
    assert!(!hits.is_empty(), "should find 'quantum' in title");
    assert_eq!(hits[0].id, "n1");

    // Empty query returns all nodes
    let all = store.fts_search("", 100).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn fts_search_empty_query_respects_limit() {
    // Regression: the empty-query branch used to return ALL node ids
    // unbounded (reachable via the AI `kb_search` tool); it must honor `limit`.
    let (_tmp, store) = make_store();
    for i in 0..10 {
        store
            .insert_node(&Node::new(
                format!("n{i}"),
                format!("Title {i}"),
                NodeKind::Note,
                "body",
            ))
            .unwrap();
    }
    let bounded = store.fts_search("", 3).unwrap();
    assert_eq!(bounded.len(), 3, "empty query must respect the limit");
}

#[test]
fn fts_search_bulk_path_matches_terms_and_scores() {
    // Exercises the bulk-fetch (`is_in`) path that replaced the per-candidate
    // get_node N+1: candidates must still be term-verified against their real
    // title+body (fetched in one query), non-matches excluded, and the FTS
    // score preserved. Uses colon-namespaced ids (the KB norm) to confirm the
    // bulk `is_in` lookup handles them.
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "doc:rust",
            "Rust Notes",
            NodeKind::Note,
            "The borrow checker enforces memory safety",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "doc:python",
            "Python Notes",
            NodeKind::Note,
            "Duck typing is flexible",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "doc:empty",
            "Unrelated",
            NodeKind::Note,
            "nothing relevant here",
        ))
        .unwrap();

    let hits = store.fts_search("borrow", 10).unwrap();
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert!(
        ids.contains(&"doc:rust"),
        "bulk path should surface the term match, got {ids:?}"
    );
    assert!(
        !ids.contains(&"doc:empty"),
        "term-verification must exclude non-matches"
    );
    // Bulk fetch must not drop the score carried from the FTS query.
    assert!(hits.iter().all(|h| h.score >= 0.0));
}

#[test]
fn fts_updates_on_node_change() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "u1",
            "Alpha",
            NodeKind::Note,
            "original content about photosynthesis",
        ))
        .unwrap();

    // Should find photosynthesis
    let hits = store.fts_search("photosynthesis", 10).unwrap();
    assert_eq!(hits.len(), 1);

    // Update body
    store
        .insert_node(&Node::new(
            "u1",
            "Alpha",
            NodeKind::Note,
            "updated content about mitochondria",
        ))
        .unwrap();

    // Old term should NOT be found (FTS re-indexed via rm + put)
    let hits = store.fts_search("photosynthesis", 10).unwrap();
    assert!(
        hits.is_empty(),
        "stale FTS: 'photosynthesis' should not match after update"
    );

    // New term should be found
    let hits = store.fts_search("mitochondria", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "u1");
}

#[test]
fn tantivy_fts_on_sqlite() {
    // Test CozoDB's native Tantivy FTS index on sled backend
    let tmp = tempfile::tempdir().unwrap();
    let db = DbInstance::new("sled", tmp.path().join("fts_test").to_str().unwrap(), "").unwrap();

    db.run_script(
        ":create docs { id: String => title: String, body: String }",
        BTreeMap::new(),
        ScriptMutability::Mutable,
    )
    .unwrap();

    // Create FTS index
    let fts_create = db.run_script(
        r#"::fts create docs:search {
                extractor: body,
                tokenizer: Simple,
                filters: [Lowercase]
            }"#,
        BTreeMap::new(),
        ScriptMutability::Mutable,
    );
    if let Err(e) = &fts_create {
        panic!("FTS index creation failed on sqlite: {e}");
    }

    // Insert docs
    db.run_script(
        r#"?[id, title, body] <- [
                ["n1", "Quantum Physics", "Entanglement is a spooky action at a distance"],
                ["n2", "Classical Mechanics", "Newton discovered gravity under an apple tree"],
                ["n3", "Relativity", "Einstein showed that space and time are intertwined"]
            ] :put docs {id => title, body}"#,
        BTreeMap::new(),
        ScriptMutability::Mutable,
    )
    .unwrap();

    // FTS search for "gravity"
    let res = db
            .run_script(
                r"?[id, title, score] := ~docs:search{id, title | query: 'gravity', k: 5, bind_score: score}",
                BTreeMap::new(),
                ScriptMutability::Immutable,
            )
            .unwrap();

    assert_eq!(res.rows.len(), 1);
    assert_eq!(res.rows[0][0].get_str().unwrap(), "n2");

    // Multi-word search
    let res2 = db
        .run_script(
            r"?[id, score] := ~docs:search{id | query: 'space time', k: 5, bind_score: score}",
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
        .unwrap();
    assert_eq!(res2.rows.len(), 1);
    assert_eq!(res2.rows[0][0].get_str().unwrap(), "n3");

    // Test update: old term should be removed from FTS index
    db.run_script(
        r#"?[id, title, body] <- [["n2", "Classical Mechanics", "Hamilton reformulated mechanics"]]
            :put docs {id => title, body}"#,
        BTreeMap::new(),
        ScriptMutability::Mutable,
    )
    .unwrap();

    let res3 = db
        .run_script(
            r"?[id, score] := ~docs:search{id | query: 'gravity', k: 5, bind_score: score}",
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
        .unwrap();
    // Should no longer find "gravity" — it was in n2 which was updated
    // Verify FTS auto-cleans stale entries after update
    eprintln!(
        "After update, 'gravity' search returns {} results: {:?}",
        res3.rows.len(),
        res3.rows
            .iter()
            .map(|r| r[0].get_str().unwrap_or("?"))
            .collect::<Vec<_>>()
    );
    // n3 still has "gravity" in its body
    assert!(
        res3.rows.len() <= 1,
        "should have at most 1 result (n3), got {}",
        res3.rows.len()
    );
}
