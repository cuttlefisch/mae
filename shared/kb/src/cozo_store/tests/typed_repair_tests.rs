//! Typed edges are written back into the bodies that lost them (#854, #941):
//! a store made before mae-org ingest holds typed `links` rows that no stored
//! body declares (the full-reimport side path wrote them; ingest stripped the
//! `?rel=` from the body). The open-time pass must declare each in its node's
//! body, through a real node write, before anything can drop it.

use super::rederive::DERIVATION_VERSION_KEY;
use super::*;
use std::collections::BTreeSet;

// Real-shaped ids: the stores this repairs address nodes by uuid.
const A: &str = "6f1c2a8e-4b1d-4c55-9a3e-0d8f7b1e2c11";
const B: &str = "b7e0d3f2-9c4a-4e1b-8f6d-2a5c9e8b1d22";
const C: &str = "c2d4e6f8-1a3b-4c5d-8e7f-9a0b1c2d3e33";
const D: &str = "d9e8f7a6-5b4c-4d3e-8f2a-1b0c9d8e7f44";
const E: &str = "e1f2a3b4-c5d6-4e7f-8a9b-0c1d2e3f4a55";
const F: &str = "f0e9d8c7-b6a5-4f3e-8d2c-1b0a9f8e7d66";
const G: &str = "a5b6c7d8-e9f0-4a1b-8c2d-3e4f5a6b7c77";
const URL: &str = "https://example.invalid/spec";

/// The typed rows of the fixture, as the side path left them:
/// `(src, dst, rel, weight, confidence)`.
fn typed_rows() -> Vec<(&'static str, &'static str, &'static str, f64, f64)> {
    vec![
        // a: a store-form link to B, untyped; a non-default weight and
        // confidence must survive.
        (A, B, "cites", 0.5, 0.75),
        // a: one raw-org link to C, TWO relations to it.
        (A, C, "elaborates", 1.0, 1.0),
        (A, C, "depends_on", 1.0, 1.0),
        // a: no link to D in the body at all.
        (A, D, "next", 1.0, 1.0),
        // e (CRDT-backed): one link to B.
        (E, B, "part_of", 1.0, 1.0),
        // f already declares its relation: must not be touched.
        (F, B, "cites", 1.0, 1.0),
        // g: an external URL row; its body's web link is not an edge.
        (G, URL, "cites", 1.0, 1.0),
    ]
}

fn bodies() -> Vec<(&'static str, String)> {
    vec![
        (
            A,
            format!("See [[{B}|the spec]] and [[id:{C}][design]].\nMore prose.\n"),
        ),
        (B, String::new()),
        (C, String::new()),
        (D, String::new()),
        (E, format!("Then [[{B}|B]].\n")),
        (F, format!("Declared: [[{B}?rel=cites|B]].\n")),
        (G, format!("Read [[{URL}][the spec]].\n")),
    ]
}

/// A store as the pre-#854-fix binary left a native KB: stripped bodies,
/// typed rows beside them, no derivation stamp.
fn write_pre_repair_store(path: &std::path::Path) {
    let store = CozoKbStore::open(path).unwrap();
    for (id, body) in bodies() {
        let mut n = Node::new(id, id, NodeKind::Note, body.as_str());
        if id == E {
            n.crdt_doc = Some(mae_sync::kb::KbNodeDoc::new(id, id, &body, &[]).encode_state());
        }
        store.insert_node(&n).unwrap();
    }
    for (s, d, r, w, c) in typed_rows() {
        store.add_typed_link_with_confidence(s, d, r, w, c).unwrap();
    }
    store
        .run_mut_params(
            "?[key] <- [[$k]] :rm instance_meta {key}",
            BTreeMap::from([("k".to_string(), DataValue::from(DERIVATION_VERSION_KEY))]),
        )
        .unwrap();
}

fn typed_set(store: &CozoKbStore) -> BTreeSet<(String, String, String)> {
    store
        .run_immut("?[s, d, r] := *links{src: s, dst: d, rel_type: r}, r != 'references'")
        .unwrap()
        .rows
        .iter()
        .map(|r| {
            let s = |i: usize| r[i].get_str().unwrap().to_string();
            (s(0), s(1), s(2))
        })
        .collect()
}

fn expected_typed() -> BTreeSet<(String, String, String)> {
    typed_rows()
        .into_iter()
        .map(|(s, d, r, _, _)| (s.to_string(), d.to_string(), r.to_string()))
        .collect()
}

fn body(store: &CozoKbStore, id: &str) -> String {
    store.get_node(id).unwrap().unwrap().body
}

fn declared(store: &CozoKbStore, id: &str) -> BTreeSet<(String, String)> {
    let n = store.get_node(id).unwrap().unwrap();
    CozoKbStore::derived_links(&n)
        .into_iter()
        .map(|(d, r, _, _)| (d, r))
        .collect()
}

fn edge(store: &CozoKbStore, src: &str, dst: &str, rel: &str) -> Option<(f64, f64)> {
    store
        .links_from(src)
        .unwrap()
        .into_iter()
        .find(|l| l.dst == dst && l.rel_type == rel)
        .map(|l| (l.weight, l.confidence))
}

#[test]
fn typed_rows_no_body_declares_are_written_into_the_bodies_and_survive_a_write() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("kb");
    write_pre_repair_store(&path);

    let store = CozoKbStore::open(&path).unwrap();
    assert_eq!(
        typed_set(&store),
        expected_typed(),
        "no typed row may be lost"
    );
    // Every KB row is now declared by its node's body.
    for (s, d, r, _, _) in typed_rows() {
        // A CRDT-backed body is never edited by the repair (each peer would
        // insert the same text under its own client id); its row is kept.
        if d == URL || s == E {
            continue;
        }
        assert!(
            declared(&store, s).contains(&(d.to_string(), r.to_string())),
            "{s} must declare {d} {r}; body: {:?}",
            body(&store, s)
        );
    }
    assert_eq!(edge(&store, A, B, "cites"), Some((0.5, 0.75)));
    // The untyped link to B was rewritten, not duplicated: no `references`
    // row is left beside the typed one (#854's duplicate).
    assert_eq!(edge(&store, A, B, "references"), None);
    // Prose is untouched; only links changed.
    assert!(body(&store, A).contains("More prose.\n"));
    // The CRDT-backed node is untouched, body and document alike, and its
    // typed row is kept (checked by `typed_set` above).
    let original_e: std::collections::HashMap<_, _> = bodies().into_iter().collect();
    let e = store.get_node(E).unwrap().unwrap();
    assert_eq!(e.body, original_e[E]);
    let doc = mae_sync::kb::KbNodeDoc::from_bytes(e.crdt_doc.as_ref().unwrap()).unwrap();
    assert_eq!(doc.body(), original_e[E]);
    assert!(store.node_history(E, 10).unwrap().is_empty());
    // History recorded the authored-looking change (ADR-106).
    assert!(!store.node_history(A, 10).unwrap().is_empty());
    // Untouched: a body that already declares its rel, and an external row.
    let original: std::collections::HashMap<_, _> = bodies().into_iter().collect();
    assert_eq!(body(&store, F), original[F]);
    assert_eq!(body(&store, G), original[G]);
    assert!(store.node_history(F, 10).unwrap().is_empty());

    // #854's repro: an ordinary write of the node keeps its typed edges --
    // unchanged text, then a property-only change (what kb_set_role writes).
    let a = store.get_node(A).unwrap().unwrap();
    store.insert_node(&a).unwrap();
    let mut a = store.get_node(A).unwrap().unwrap();
    a.properties.insert("role".into(), "atom".into());
    store.update_node(&a).unwrap();
    assert_eq!(typed_set(&store), expected_typed());

    // Re-open: nothing to do, nothing moves.
    let snapshot: Vec<String> = bodies().iter().map(|(id, _)| body(&store, id)).collect();
    let versions = store.node_history(A, 10).unwrap().len();
    drop(store);
    let store = CozoKbStore::open(&path).unwrap();
    let again: Vec<String> = bodies().iter().map(|(id, _)| body(&store, id)).collect();
    assert_eq!(again, snapshot);
    assert_eq!(store.node_history(A, 10).unwrap().len(), versions);
    assert_eq!(typed_set(&store), expected_typed());
}

/// #854's repro, on a file-backed KB: the typed link in the file is the edge,
/// with no `references` duplicate, after an ingest AND after a write.
#[test]
fn a_typed_link_in_a_file_survives_ingest_and_a_later_write() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.org"),
        ":PROPERTIES:\n:ID: a\n:END:\n#+title: A\n\nSee [[id:b?rel=answers][b]].\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("b.org"),
        ":PROPERTIES:\n:ID: b\n:END:\n#+title: B\n\nBody.\n",
    )
    .unwrap();
    let store = CozoKbStore::open(tmp.path().join("kb")).unwrap();
    crate::federation::import_org_dir_to_store(&dir, &store, &crate::IngestMode::Full).unwrap();
    let rels = |store: &CozoKbStore| -> Vec<String> {
        let mut v: Vec<String> = store
            .links_from("a")
            .unwrap()
            .into_iter()
            .filter(|l| l.dst == "b")
            .map(|l| l.rel_type)
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        rels(&store),
        vec!["answers".to_string()],
        "after a full ingest"
    );

    let mut a = store.get_node("a").unwrap().unwrap();
    a.properties.insert("role".into(), "atom".into());
    store.update_node(&a).unwrap();
    assert_eq!(
        rels(&store),
        vec!["answers".to_string()],
        "after a write of a"
    );

    // The editor's save path: a single-file re-ingest, then a store write.
    let mut kb = crate::KnowledgeBase::new();
    kb.ingest_org_file(dir.join("a.org"));
    store.insert_node(kb.get("a").unwrap()).unwrap();
    assert_eq!(
        rels(&store),
        vec!["answers".to_string()],
        "after a re-ingest"
    );
}

fn reopen_and_count_appended(path: &std::path::Path, src: &str, dst: &str) -> usize {
    let store = CozoKbStore::open(path).unwrap();
    body(&store, src).matches(&format!("[[{dst}?rel=")).count()
}

/// An interruption between batches loses nothing, the next open completes
/// it, and a node already repaired is not repaired twice (no duplicate
/// appended link).
#[test]
fn an_interrupted_repair_resumes_without_loss_or_duplicates() {
    use crate::cozo_store::test_hooks::set_fail_typed_repair_at;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("kb");
    let n = 70; // more than one repair batch
    {
        let store = CozoKbStore::open(&path).unwrap();
        store
            .insert_node(&Node::new(D, "D", NodeKind::Note, ""))
            .unwrap();
        for i in 0..n {
            let id = format!("n{i:03}");
            store
                .insert_node(&Node::new(&id, &id, NodeKind::Note, "prose\n"))
                .unwrap();
            store.add_typed_link(&id, D, "next", 1.0).unwrap();
        }
        store
            .run_mut_params(
                "?[key] <- [[$k]] :rm instance_meta {key}",
                BTreeMap::from([("k".to_string(), DataValue::from(DERIVATION_VERSION_KEY))]),
            )
            .unwrap();
    }
    set_fail_typed_repair_at(Some(1));
    let store = CozoKbStore::open(&path);
    set_fail_typed_repair_at(None);
    let store = store.expect("an interrupted repair must not block open");
    assert_eq!(typed_set(&store).len(), n, "nothing dropped mid-pass");
    let done = (0..n)
        .filter(|i| declared(&store, &format!("n{i:03}")).contains(&(D.to_string(), "next".into())))
        .count();
    assert!(
        done > 0 && done < n,
        "the first batch was written, not the rest: {done}"
    );
    assert_eq!(store.get_meta(DERIVATION_VERSION_KEY).unwrap(), None);
    drop(store);

    for i in 0..n {
        assert_eq!(reopen_and_count_appended(&path, &format!("n{i:03}"), D), 1);
    }
    let store = CozoKbStore::open(&path).unwrap();
    assert_eq!(typed_set(&store).len(), n);
    assert_eq!(
        store.get_meta(DERIVATION_VERSION_KEY).unwrap().as_deref(),
        Some(DERIVATION_VERSION)
    );
    assert!(store.repair_typed_links().unwrap().is_noop());
    drop(store);
    // The resumed pass reused the interrupted pass's backup: exactly one, and
    // it still holds the PRE-repair text even for a node the interrupted pass
    // had already repaired. (A second copy taken in the same second would have
    // overwritten it with half-repaired state.)
    let found: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().contains(".pre-typed-links.bak-"))
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    let db = DbInstance::new("sqlite", found[0].to_str().unwrap(), "").unwrap();
    let rows = db
        .run_script(
            "?[body] := *nodes{id: 'n000', body}",
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
        .unwrap()
        .rows;
    assert_eq!(
        rows[0][0].get_str().unwrap(),
        "prose\n",
        "the backup is pre-repair"
    );
}

/// The rows the bootstrap re-seeds are not written into a body, and a seed
/// node's body is never edited; both are kept.
#[test]
fn seeded_rows_and_seed_nodes_are_left_alone_and_kept() {
    let store = CozoKbStore::open_mem().unwrap();
    store
        .insert_node(&Node::new("index", "Index", NodeKind::Note, "Start.\n"))
        .unwrap();
    let mut seed = Node::new(A, "A", NodeKind::Note, format!("[[{B}|B]]\n"));
    seed.source = Some(crate::NodeSource::Seed);
    store.insert_node(&seed).unwrap();
    store.seed_typed_relationships().unwrap();
    store.add_typed_link(A, B, "cites", 1.0).unwrap();
    let before = typed_set(&store);
    let report = store.repair_typed_links().unwrap();
    assert!(report.is_noop(), "{report:?}");
    assert_eq!(
        report.rows_left, 1,
        "the seed node's row is counted, not declared"
    );
    assert_eq!(body(&store, "index"), "Start.\n");
    assert_eq!(body(&store, A), format!("[[{B}|B]]\n"));
    store.rederive_all_links().unwrap();
    assert_eq!(
        typed_set(&store),
        before,
        "the pass keeps what it cannot declare"
    );
}

/// A store the repair changes gets one backup beside it, holding the
/// pre-repair state; a store with nothing to repair gets none.
#[test]
fn a_store_is_backed_up_once_before_its_first_repair() {
    let backups = |dir: &std::path::Path| -> Vec<std::path::PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().contains(".pre-typed-links.bak-"))
            .collect()
    };
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("kb");
    write_pre_repair_store(&path);
    drop(CozoKbStore::open(&path).unwrap());
    let found = backups(tmp.path());
    assert_eq!(found.len(), 1, "{found:?}");
    // The backup is the pre-repair store: opened through cozo alone (no
    // repair), its body for A has no relation in it.
    let db = DbInstance::new("sqlite", found[0].to_str().unwrap(), "").unwrap();
    let rows = db
        .run_script(
            "?[body] := *nodes{id: $id, body}",
            BTreeMap::from([("id".to_string(), DataValue::from(A))]),
            ScriptMutability::Immutable,
        )
        .unwrap()
        .rows;
    assert!(!rows[0][0].get_str().unwrap().contains("rel="));
    drop(db);
    // Re-opening repairs nothing and takes no second copy.
    drop(CozoKbStore::open(&path).unwrap());
    assert_eq!(backups(tmp.path()).len(), 1);

    let clean = tempfile::tempdir().unwrap();
    drop(CozoKbStore::open(clean.path().join("kb")).unwrap());
    assert!(backups(clean.path()).is_empty());
}

fn wanted(dst: &str, rel: &str, w: f64, c: f64) -> crate::org_ingest::WantedEdge {
    crate::org_ingest::WantedEdge {
        dst: dst.into(),
        rel: rel.into(),
        weight: w,
        confidence: c,
    }
}

/// The exact text the repair writes: the relation goes into the link that
/// is there, in whichever form it is written; `w`/`c` only off-default.
#[test]
fn declare_edges_rewrites_the_link_in_place_in_either_form() {
    use crate::org_ingest::declare_edges;
    let d = declare_edges(
        &format!("a [[{B}|the spec]] b [[id:{C}][design]] c\n"),
        &[
            wanted(B, "cites", 0.5, 1.0),
            wanted(C, "elaborates", 1.0, 0.25),
        ],
    )
    .unwrap();
    assert_eq!(
        d.body,
        format!(
            "a [[{B}?rel=cites&w=0.5|the spec]] b [[id:{C}?rel=elaborates&c=0.25][design]] c\n"
        )
    );
    assert_eq!((d.rewritten, d.appended), (2, 0));
    // A second relation to the same target, with its one link taken,
    // is appended as kb_add_link appends it.
    let d = declare_edges(
        &format!("[[{C}]]\n"),
        &[
            wanted(C, "depends_on", 1.0, 1.0),
            wanted(C, "elaborates", 1.0, 1.0),
        ],
    )
    .unwrap();
    assert_eq!(
        d.body,
        format!("[[{C}?rel=depends_on]]\n\n[[{C}?rel=elaborates&w=1][{C}]]")
    );
    // A link that already carries a query or a fragment is not taken.
    let d = declare_edges(&format!("[[{C}#h]]\n"), &[wanted(C, "next", 1.0, 1.0)]).unwrap();
    assert_eq!(d.appended, 1);
    // An external URL cannot be an edge: nothing is written.
    assert!(!wanted(URL, "cites", 1.0, 1.0).is_declarable());
    assert_eq!(
        declare_edges(&format!("[[{URL}][x]]"), &[wanted(URL, "cites", 1.0, 1.0)]),
        None
    );
    // A target the link grammar cannot hold: the result is checked, not
    // trusted, so it is refused rather than half-written.
    assert!(!wanted("x]y", "next", 1.0, 1.0).is_declarable());
    assert_eq!(
        declare_edges("prose\n", &[wanted("x]y", "next", 1.0, 1.0)]),
        None
    );
}

/// The appended text is kb_add_link's, byte for byte.
#[test]
fn the_appended_link_is_the_add_link_format() {
    assert_eq!(
        crate::org_ingest::typed_link_line("x", "cites", 0.5, None),
        "\n[[x?rel=cites&w=0.5][x]]"
    );
}

// ---- Legacy bar-form links in table rows (#941) ----------------------------
//
// The old ingest stored every KB link as `[[TARGET|DESCRIPTION]]`. In an Org
// table the bar is the cell separator, so mae-org reads such a link as two
// cells of text: a native KB (no file to re-ingest) would lose it. Measured on
// a real store: 77 links in 17 nodes.

fn table_body(link: &str) -> String {
    format!("| name | link |\n|------+------|\n| x | {link} |\n")
}

#[test]
fn a_bar_form_link_in_a_table_row_is_rewritten_and_becomes_an_edge() {
    let before = table_body(&format!("[[{B}|the spec]]"));
    assert!(
        crate::org_ingest::edges_of_body(&before).is_empty(),
        "precondition: the bar splits the cell, so no edge (the defect)"
    );
    let (after, n) = crate::org_ingest::repair_table_pipe_links(&before).unwrap();
    assert_eq!(n, 1);
    assert_eq!(after, table_body(&format!("[[{B}][the spec]]")));
    let edges = crate::org_ingest::edges_of_body(&after);
    assert_eq!(edges.len(), 1);
    assert_eq!(
        (edges[0].target.as_str(), edges[0].display.as_str()),
        (B, "the spec")
    );
    assert!(
        crate::org_ingest::repair_table_pipe_links(&after).is_none(),
        "idempotent: a second pass finds nothing"
    );
}

#[test]
fn only_real_table_rows_and_only_kb_links_are_rewritten() {
    let cases = [
        // A table-looking line inside a block is not a table.
        format!("#+begin_src text\n| [[{B}|B]] |\n#+end_src\n"),
        // Outside a table the bar form already reads as a link.
        format!("See [[{B}|B]].\n"),
        // Org's own form in a table is already a link.
        table_body(&format!("[[id:{B}][B]]")),
        // A web link is not a KB edge.
        table_body("[[https://example.invalid/x|x]]"),
        // A description holding brackets is not the store form.
        table_body(&format!("[[{B}|a [b] c]]")),
    ];
    for body in cases {
        assert_eq!(
            crate::org_ingest::repair_table_pipe_links(&body),
            None,
            "must be left as written:\n{body}"
        );
    }
}

/// Each guard on its own: a body mixing a repairable link with one that must
/// stay as written gets the first repaired, not the whole body refused.
#[test]
fn a_mixed_body_repairs_what_it_should_and_leaves_the_rest() {
    // A table link beside a web link in the same table.
    let web = format!(
        "{}| w | [[https://example.invalid/x|x]] |\n",
        table_body(&format!("[[{B}|B]]"))
    );
    let (out, n) = crate::org_ingest::repair_table_pipe_links(&web).unwrap();
    assert_eq!(n, 1, "only the KB link");
    assert!(out.contains("[[https://example.invalid/x|x]]"), "{out}");
    // A table link beside a table-looking line in a block.
    let block = format!(
        "{}#+begin_src text\n| [[{D}|D]] |\n#+end_src\n",
        table_body(&format!("[[{B}|B]]"))
    );
    let (out, n) = crate::org_ingest::repair_table_pipe_links(&block).unwrap();
    assert_eq!(n, 1, "only the real table row");
    assert!(out.contains(&format!("| [[{D}|D]] |")), "{out}");
}

#[test]
fn opening_a_native_store_repairs_its_table_links_and_keeps_typed_rows() {
    let store = CozoKbStore::open_mem().unwrap();
    // A native node: two bar-form table links (one also the source of a typed
    // row) and a table-looking line in a block that must stay as written.
    let native = format!(
        "{}| y | [[{C}|C]] |\n#+begin_src text\n| [[{D}|D]] |\n#+end_src\n",
        table_body(&format!("[[{B}|B]]"))
    );
    store
        .insert_node(&Node::new(A, "A", NodeKind::Note, native.as_str()))
        .unwrap();
    // A node with table links and NO typed row: planned for its tables alone.
    store
        .insert_node(&Node::new(
            F,
            "F",
            NodeKind::Note,
            table_body(&format!("[[{C}|C]]")).as_str(),
        ))
        .unwrap();
    // A CRDT-backed node with only a table link (no typed row): left alone,
    // and counted, never skipped silently.
    let shared_body = table_body(&format!("[[{B}|B]]"));
    let mut shared = Node::new(G, "G", NodeKind::Note, shared_body.as_str());
    shared.crdt_doc = Some(mae_sync::kb::KbNodeDoc::new(G, G, &shared_body, &[]).encode_state());
    store.insert_node(&shared).unwrap();
    let mut seed = Node::new(
        E,
        "E",
        NodeKind::Note,
        table_body(&format!("[[{B}|B]]")).as_str(),
    );
    seed.source = Some(crate::NodeSource::Seed);
    store.insert_node(&seed).unwrap();
    for id in [B, C, D] {
        store
            .insert_node(&Node::new(id, id, NodeKind::Note, ""))
            .unwrap();
    }
    store.add_typed_link(A, B, "cites", 1.0).unwrap();
    // The seed node also has a typed row, so the typed-row half of the plan
    // reaches it: only the seed guard keeps its table from being rewritten.
    store.add_typed_link(E, C, "cites", 1.0).unwrap();
    assert!(
        edge(&store, A, C, "references").is_none(),
        "precondition: no edge"
    );

    let report = store.repair_typed_links().unwrap();
    store.rederive_all_links().unwrap();

    assert_eq!(report.table_links_repaired, 3, "{report:?}");
    assert_eq!(report.crdt_nodes_skipped, 1, "{report:?}");
    assert_eq!(body(&store, G), shared_body);
    assert_eq!(body(&store, F), table_body(&format!("[[{C}][C]]")));
    assert!(edge(&store, F, C, "references").is_some());
    let a = body(&store, A);
    assert!(a.contains(&format!("| x | [[{B}][B]] |")), "{a}");
    assert!(a.contains(&format!("| y | [[{C}][C]] |")), "{a}");
    assert!(
        a.contains(&format!("| [[{D}|D]] |")),
        "the block's line is untouched:\n{a}"
    );
    assert!(
        edge(&store, A, C, "references").is_some(),
        "the table link is an edge"
    );
    assert!(
        edge(&store, A, B, "cites").is_some(),
        "the typed row is kept and declared"
    );
    assert!(declared(&store, A).contains(&(B.to_string(), "cites".to_string())));
    assert_eq!(
        body(&store, E),
        table_body(&format!("[[{B}|B]]")),
        "seed content is regenerated from the binary, never edited"
    );
    assert!(
        store.repair_typed_links().unwrap().is_noop(),
        "a second pass changes nothing"
    );
}

// ---- Concurrency and backup safety (review of #941) -------------------------
//
// The pass runs after the creation lock is released, so another process (the
// daemon, a second editor) may open the store, run its own pass, or write a
// node while this pass is between its plan and its writes. Each test puts that
// other process exactly there, through `set_after_typed_repair_plan`.

/// A store holding one node whose body lost its `?rel=`, as the old ingest left
/// it: A links to B, the `cites` row beside it, no derivation stamp.
fn small_pre_repair_store(path: &std::path::Path) {
    let store = CozoKbStore::open(path).unwrap();
    store
        .insert_node(&Node::new(
            A,
            "A",
            NodeKind::Note,
            format!("See [[{B}|B]].\n").as_str(),
        ))
        .unwrap();
    store
        .insert_node(&Node::new(B, "B", NodeKind::Note, ""))
        .unwrap();
    store.add_typed_link(A, B, "cites", 1.0).unwrap();
    store
        .run_mut_params(
            "?[key] <- [[$k]] :rm instance_meta {key}",
            BTreeMap::from([("k".to_string(), DataValue::from(DERIVATION_VERSION_KEY))]),
        )
        .unwrap();
}

#[test]
fn an_edge_declared_after_the_plan_is_not_declared_twice() {
    use crate::cozo_store::test_hooks::set_after_typed_repair_plan;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("kb");
    small_pre_repair_store(&path);
    // After the plan, another process (an ordinary write, not a pass) gives
    // A's link its relation itself.
    let other = path.clone();
    set_after_typed_repair_plan(move || {
        let db = DbInstance::new("sqlite", other.to_str().unwrap(), "").unwrap();
        db.run_script(
            "?[id, body] <- [[$id, $b]] :update nodes {id => body}",
            BTreeMap::from([
                ("id".to_string(), DataValue::from(A)),
                (
                    "b".to_string(),
                    DataValue::from(format!("See [[{B}?rel=cites|B]].\n").as_str()),
                ),
            ]),
            ScriptMutability::Mutable,
        )
        .unwrap();
    });
    let store = CozoKbStore::open(&path).unwrap();
    let a = body(&store, A);
    assert_eq!(a.matches("rel=cites").count(), 1, "declared once: {a:?}");
    assert!(edge(&store, A, B, "cites").is_some());
}
#[test]
fn a_node_written_after_the_plan_keeps_that_write() {
    use crate::cozo_store::test_hooks::set_after_typed_repair_plan;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("kb");
    small_pre_repair_store(&path);
    // Another process edits A's text after the plan was made (raw store write,
    // so it runs no pass of its own).
    let other = path.clone();
    set_after_typed_repair_plan(move || {
        let db = DbInstance::new("sqlite", other.to_str().unwrap(), "").unwrap();
        db.run_script(
            "?[id, body] <- [[$id, $b]] :update nodes {id => body}",
            BTreeMap::from([
                ("id".to_string(), DataValue::from(A)),
                (
                    "b".to_string(),
                    DataValue::from(format!("User edit. See [[{B}|B]].\n").as_str()),
                ),
            ]),
            ScriptMutability::Mutable,
        )
        .unwrap();
    });
    let store = CozoKbStore::open(&path).unwrap();
    let a = body(&store, A);
    assert!(
        a.starts_with("User edit."),
        "the other write survives: {a:?}"
    );
    assert!(a.contains("rel=cites"), "and the repair still lands: {a:?}");
}

#[test]
fn a_second_opener_is_not_locked_out_while_a_long_pass_runs() {
    use crate::cozo_store::test_hooks::set_after_typed_repair_plan;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("kb");
    small_pre_repair_store(&path);
    let passes_before = crate::cozo_store::test_hooks::passes_run(&path);
    // The first opener's pass takes longer than the 5 s another opener waits
    // for the store-creation lock (tens of seconds were measured on a large store).
    let first = path.clone();
    let t = std::thread::spawn(move || {
        set_after_typed_repair_plan(|| std::thread::sleep(std::time::Duration::from_secs(6)));
        CozoKbStore::open(&first).map(drop)
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    let second = CozoKbStore::open(&path);
    assert!(second.is_ok(), "second opener: {:?}", second.err());
    // It waited for the first pass instead of running its own, so it loaded
    // the repaired store.
    assert!(body(&second.unwrap(), A).contains("rel=cites"));
    t.join().unwrap().unwrap();
    assert_eq!(
        crate::cozo_store::test_hooks::passes_run(&path) - passes_before,
        1,
        "passes are serialized: of the two openers, one ran the pass"
    );
    let store = CozoKbStore::open(&path).unwrap();
    assert_eq!(body(&store, A).matches("rel=cites").count(), 1);
}

#[test]
fn a_partial_or_older_backup_is_never_taken_for_the_pre_repair_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("kb");
    small_pre_repair_store(&path);
    // Left behind: a backup under the pre-versioned name (another derivation
    // version's), and a copy that was cut short.
    let older = tmp.path().join("kb.pre-typed-links.bak-1700000000");
    std::fs::write(&older, b"another version's backup").unwrap();
    let partial = tmp.path().join(".kb.typed-links-backup.partial");
    std::fs::write(&partial, b"cut short").unwrap();

    drop(CozoKbStore::open(&path).unwrap());

    let current: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().contains(".pre-typed-links.bak-v"))
        .collect();
    assert_eq!(current.len(), 1, "one fresh backup: {current:?}");
    assert!(
        !partial.exists(),
        "the cut-short copy is removed, never reused"
    );
    assert_eq!(std::fs::read(&older).unwrap(), b"another version's backup");
    // And the fresh backup is the pre-repair store.
    let db = DbInstance::new("sqlite", current[0].to_str().unwrap(), "").unwrap();
    let rows = db
        .run_script(
            "?[body] := *nodes{id: $id, body}",
            BTreeMap::from([("id".to_string(), DataValue::from(A))]),
            ScriptMutability::Immutable,
        )
        .unwrap()
        .rows;
    assert!(!rows[0][0].get_str().unwrap().contains("rel="));
}

#[test]
fn a_cell_link_that_cannot_be_rewritten_does_not_block_the_others() {
    // A bar-form link inside verbatim markup in one cell (no link either way),
    // beside a plain one in another row.
    let body = format!("| a | =[[{B}|x]]= |\n| b | [[{C}|C]] |\n");
    let (out, n) = crate::org_ingest::repair_table_pipe_links(&body).unwrap();
    assert_eq!(n, 1, "{out}");
    assert!(out.contains(&format!("[[{C}][C]]")), "{out}");
    assert!(
        out.contains(&format!("=[[{B}|x]]=")),
        "left as written: {out}"
    );
}

#[test]
fn a_leftover_derivation_lock_file_never_blocks_an_open() {
    // What a killed holder leaves: the lock file, with whatever it held (here
    // this very process's PID, as a restarted container daemon would see, and
    // a torn variant). The lock is the OS's, released with the holder, so the
    // file's presence or content blocks nothing.
    for content in [
        format!("{{\"pid\": {}}}", std::process::id()),
        "{".to_string(),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("kb");
        small_pre_repair_store(&path);
        std::fs::write(tmp.path().join(".kb.derive.lock"), &content).unwrap();
        let t = std::time::Instant::now();
        let store = CozoKbStore::open(&path).unwrap();
        assert!(
            t.elapsed() < std::time::Duration::from_secs(4),
            "{content}: {:?}",
            t.elapsed()
        );
        assert!(body(&store, A).contains("rel=cites"));
    }
}
