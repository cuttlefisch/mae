//! #710 — provenance must cross the wire.
//!
//! `NodeSource::Seed` is the **only enforced read-only mechanism** for shipped
//! content. Before `source` joined the ADR-093 schema, a shared node arrived at
//! the peer re-stamped `Federation`, so a read-only corpus became fully editable
//! the moment it was shared — and the receiving peer could not reconstruct what
//! it was never sent.
//!
//! The general form is worse than the system-KB case that surfaced it: **any**
//! provenance distinction a KB relies on was lost on sharing.
//!
//! Per principle #14 the primary test is the negative one — not "the round-trip
//! works" but "the thing that must not happen, does not".

use mae_sync::kb::KbNodeDoc;

/// The attacker-shaped case: a `Seed` node must NOT arrive editable.
///
/// Written against the wire payload rather than an in-process clone, because the
/// defect was specifically that the *encoded* form omitted provenance — a test
/// that copies a struct would have passed throughout.
#[test]
fn a_seed_node_does_not_arrive_at_a_peer_as_federation() {
    let mut origin = KbNodeDoc::new("concept:x", "Shipped", "release-owned body", &[]);
    let _ = origin.set_source(Some("seed"));

    // Exactly what a peer receives.
    let wire = origin.encode_state();
    let peer = KbNodeDoc::from_bytes(&wire).expect("peer decodes the node");

    assert_eq!(
        peer.source().as_deref(),
        Some("seed"),
        "a Seed node arrived at the peer without its provenance — it is now \
         editable content that MAE ships as read-only (#710)"
    );
    assert_ne!(
        peer.source().as_deref(),
        Some("federation"),
        "provenance was replaced with Federation, which is the exact defect"
    );
}

/// Every variant must survive, not just `Seed`. A fix that special-cased the one
/// variant with a guard attached would pass the test above and still lose the
/// distinction the issue is actually about.
#[test]
fn every_provenance_variant_survives_the_wire() {
    for source in ["seed", "user_org", "manual", "federation", "promoted"] {
        let mut origin = KbNodeDoc::new("n", "T", "B", &[]);
        let _ = origin.set_source(Some(source));
        let peer = KbNodeDoc::from_bytes(&origin.encode_state()).unwrap();
        assert_eq!(
            peer.source().as_deref(),
            Some(source),
            "provenance '{source}' did not survive the wire"
        );
    }
}

/// Tolerant reader (ADR-093): a document authored before `source` joined the
/// schema must read as `None` — NOT as some default.
///
/// Defaulting is how read-only content becomes editable: a guessed `Federation`
/// looks like a legitimate answer and silently strips the marking. `None` means
/// "this document does not say", and the caller keeps what it already had.
#[test]
fn a_document_without_provenance_reads_as_none_rather_than_a_default() {
    let doc = KbNodeDoc::new("n", "T", "B", &[]);
    assert_eq!(
        doc.source(),
        None,
        "a document that carries no provenance must not invent one"
    );
}

/// Concurrent edits must not resurrect stripped provenance or lose it: two peers
/// editing different fields of the same node must both keep `Seed`.
#[test]
fn provenance_survives_concurrent_edits_from_two_peers() {
    let mut origin = KbNodeDoc::new("concept:x", "Shipped", "body", &[]);
    let _ = origin.set_source(Some("seed"));
    let base = origin.encode_state();

    let mut a = KbNodeDoc::from_bytes(&base).unwrap();
    let mut b = KbNodeDoc::from_bytes(&base).unwrap();
    let ua = a.set_title("edited by A");
    let ub = b.set_priority(Some("B"));

    // Merge both ways; convergence must not depend on order.
    a.apply_update(&ub).unwrap();
    b.apply_update(&ua).unwrap();

    for (name, doc) in [("A", &a), ("B", &b)] {
        assert_eq!(
            doc.source().as_deref(),
            Some("seed"),
            "peer {name} lost provenance after a concurrent edit"
        );
    }
}
