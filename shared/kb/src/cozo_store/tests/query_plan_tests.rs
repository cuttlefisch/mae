//! Guards against the query shape that silently costs a full relation scan.
//!
//! CozoDB compiles `*rel{k}, k = $x` (a *post-filter* equality) and
//! `*rel{k}, is_in(k, $ids)` to `load_stored` with a filter — i.e. it reads
//! every row of the relation and throws most of them away. It compiles
//! `k = $x, *rel{k}` and `*rel{k: $x}` (a *pre-bound* key) to
//! `stored_prefix_join` — a prefix seek. The mechanism is
//! `cozo-0.7.6/src/query/compile.rs:247-265`: a relation atom's argument
//! becomes `IndexPositionUse::Join` only if the variable is already in
//! `seen_variables`, and `logical.rs`'s normalization emits an inline bind's
//! unification *before* the relation atom. `ra.rs:1509 join_is_prefix` then
//! requires the joined positions to be exactly `0..n` — so binding a
//! *non-contiguous* subset of key columns (e.g. `links`'s `src` and
//! `rel_type`, skipping `dst`) degrades to `stored_mat_join`, which is a full
//! scan again. Binding *fewer* columns can therefore be strictly faster.
//!
//! Three tests here, in increasing order of durability:
//!   - a `::explain` oracle pinning the plan operator for the hot lookups;
//!   - a self-calibrating timing test (a missing-id `get_node` measured
//!     against a deliberate full scan of the same relation, so the assertion
//!     is a ratio and does not depend on machine speed or load);
//!   - a source scan that fails on any *new* post-filter-on-a-bindable-key
//!     query anywhere in `cozo_store/`, which is the actual regression guard.

use super::*;

/// Corpus size for the scaling tests. Large enough that a full scan dominates
/// the fixed per-query cost, small enough that setup stays a few seconds.
const N: usize = 6_000;

fn seeded_store(n: usize) -> (tempfile::TempDir, CozoKbStore) {
    let (tmp, store) = make_store();
    // Bulk `:put` rather than `n` calls to `insert_node`: these tests only read
    // id/title/body, and one transaction per node makes the fixture dominate
    // the runtime of the thing being measured.
    let rows: Vec<DataValue> = (0..n)
        .map(|i| {
            DataValue::List(vec![
                DataValue::from(format!("bench:{i:06}")),
                DataValue::from(format!("Title number {i}")),
                DataValue::from("concept"),
                DataValue::from(format!("Body text for node {i} with some filler words.")),
                DataValue::from("[]"),
                DataValue::from(""),
                DataValue::from(""),
                DataValue::from("manual"),
                DataValue::from(0),
                DataValue::from("[]"),
                DataValue::from("{}"),
                DataValue::Bytes(vec![]),
                DataValue::from(false),
                DataValue::from(""),
                DataValue::from(""),
                DataValue::from(0),
                DataValue::from(""),
                DataValue::from(0),
                DataValue::from(0),
            ])
        })
        .collect();
    store
        .run_mut_params(
            r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                 aliases_json, properties_json, crdt_doc, has_crdt, origin_instance, assignee,
                 due_date, sprint, created_at, updated_at] <- $rows
               :put nodes {id => title, kind, body, tags_json, todo_state, priority, source,
                 source_version, aliases_json, properties_json, crdt_doc, has_crdt,
                 origin_instance, assignee, due_date, sprint, created_at, updated_at}"#,
            crate::cozo_store::util::btree_params([("rows", DataValue::List(rows))]),
        )
        .unwrap();
    (tmp, store)
}

/// The plan operator for a query, as reported by `::explain`. Returns every
/// `op` column in stratum order so a caller can assert on the whole plan.
fn plan_ops(store: &CozoKbStore, query: &str) -> Vec<String> {
    let explained = store
        .run_immut(&format!("::explain {{ {query} }}"))
        .unwrap_or_else(|e| panic!("::explain failed for {query}: {e}"));
    explained
        .rows
        .iter()
        .filter_map(|row| row.get(4).and_then(|v| v.get_str()).map(str::to_string))
        .collect()
}

fn median(mut samples: Vec<std::time::Duration>) -> std::time::Duration {
    samples.sort();
    samples[samples.len() / 2]
}

#[test]
fn get_node_plan_is_a_prefix_seek_not_a_relation_scan() {
    let (_tmp, store) = make_store();
    let node = crate::Node::new("concept:x", "X", NodeKind::Concept, "body");
    store.insert_node(&node).unwrap();

    // The shape `get_node` must NOT have: a bare scan carrying an `eq` filter.
    let post_filter = plan_ops(
        &store,
        r#"?[id, title] := *nodes{id, title}, id = "concept:x""#,
    );
    assert!(
        post_filter.contains(&"load_stored".to_string())
            && !post_filter.iter().any(|op| op.contains("prefix_join")),
        "the post-filter form is supposed to be a scan; if cozo ever starts \
         seeking it, this whole guard is obsolete — plan was {post_filter:?}"
    );

    // The shape `get_node` must have.
    let bound = plan_ops(
        &store,
        r#"?[id, title] := id = "concept:x", *nodes{id, title}"#,
    );
    assert!(
        bound.contains(&"stored_prefix_join".to_string()),
        "pre-binding the key must seek, not scan — plan was {bound:?}"
    );
}

#[test]
fn links_typed_binds_only_the_contiguous_key_prefix() {
    let (_tmp, store) = make_store();
    store.add_link("a", "b", None).unwrap();

    // `links` is keyed (src, dst, rel_type). Binding src AND rel_type skips
    // dst, so the joined positions are {0, 2} — not a prefix — and cozo
    // materializes the whole relation. Binding src alone seeks.
    let both = plan_ops(
        &store,
        r#"?[src, dst, rel_type] := src = "a", rel_type = "r", *links{src, dst, rel_type}"#,
    );
    assert!(
        both.contains(&"stored_mat_join".to_string()),
        "binding a non-contiguous key subset is NOT a prefix seek — plan was {both:?}"
    );

    let prefix_only = plan_ops(
        &store,
        r#"?[src, dst, rel_type] := src = "a", *links{src, dst, rel_type}, rel_type = "r""#,
    );
    assert!(
        prefix_only.contains(&"stored_prefix_join".to_string()),
        "src alone is a key prefix and must seek — plan was {prefix_only:?}"
    );
}

#[test]
fn fts_candidate_hydration_seeks_per_candidate() {
    let (_tmp, store) = make_store();
    let node = crate::Node::new("concept:x", "X", NodeKind::Concept, "body");
    store.insert_node(&node).unwrap();

    let is_in_form = plan_ops(
        &store,
        r#"?[id, title, body] := *nodes{id, title, body}, is_in(id, ["concept:x"])"#,
    );
    assert!(
        !is_in_form.iter().any(|op| op.contains("prefix_join")),
        "`is_in` cannot seek — plan was {is_in_form:?}"
    );

    let keyed_join = plan_ops(
        &store,
        r#"cand[id, score] <- [["concept:x", 1.0]]; ?[id, title, body, score] := cand[id, score], *nodes{id, title, body}"#,
    );
    assert!(
        keyed_join.contains(&"stored_prefix_join".to_string()),
        "a candidate relation joined on the key must seek — plan was {keyed_join:?}"
    );
}

/// The B2 regression guard, stated as a ratio so it is independent of machine
/// speed and ambient load: fetching one node — even a *missing* one, which is
/// the case that cannot be explained away by deserialization cost — must be
/// dramatically cheaper than reading the relation it lives in.
///
/// Before the fix `get_node` compiled to exactly that full scan, so the two
/// medians were within a small constant factor of each other (the scan of a
/// single column was in fact *cheaper*, since `get_node` also projects 13
/// columns including the `crdt_doc` blob).
#[test]
fn get_node_on_a_missing_id_is_far_cheaper_than_a_relation_scan() {
    let (_tmp, store) = seeded_store(N);

    // Warm caches for both shapes before timing either.
    for _ in 0..3 {
        store.get_node("bench:000010").unwrap();
        store.run_immut(r#"?[id] := *nodes{id}"#).unwrap();
    }

    let reps = 9;
    let mut missing = Vec::with_capacity(reps);
    let mut present = Vec::with_capacity(reps);
    let mut scan = Vec::with_capacity(reps);
    for rep in 0..reps {
        let t = std::time::Instant::now();
        assert!(store
            .get_node(&format!("bench:absent-{rep}"))
            .unwrap()
            .is_none());
        missing.push(t.elapsed());

        let t = std::time::Instant::now();
        assert!(store
            .get_node(&format!("bench:{:06}", rep * 37))
            .unwrap()
            .is_some());
        present.push(t.elapsed());

        let t = std::time::Instant::now();
        let rows = store
            .run_immut(r#"?[id] := *nodes{id}"#)
            .unwrap()
            .rows
            .len();
        assert_eq!(rows, N);
        scan.push(t.elapsed());
    }

    let (missing, present, scan) = (median(missing), median(present), median(scan));
    eprintln!(
        "N={N} get_node(missing)={missing:?} get_node(present)={present:?} full_scan={scan:?}"
    );
    assert!(
        missing * 8 < scan,
        "get_node on a MISSING id cost {missing:?} against a {scan:?} full scan of the same \
         {N}-row relation — that ratio means it is scanning, not seeking"
    );
    assert!(
        present * 8 < scan,
        "get_node on a present id cost {present:?} against a {scan:?} full scan of the same \
         {N}-row relation — that ratio means it is scanning, not seeking"
    );
}
