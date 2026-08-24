//! Document growth is the CRDT's one unbounded resource, so it gets a test.
//!
//! Growth in a Yjs/yrs document is monotonic in **operations**, not characters,
//! and there is no safe general compaction — the upstream position is that
//! flattening "would destroy the document integrity" and that merging old edits
//! "needs a consensus algorithm". Production reports on the same `Y.Map` shape
//! MAE uses describe documents needing gigabytes to load.
//!
//! That makes every avoidable write a permanent cost, and makes "how much does a
//! no-op cost?" a question worth asserting on rather than reasoning about.

use mae_sync::kb::KbNodeDoc;

/// Repeatedly setting a field to the value it ALREADY HAS must not grow the
/// document at all.
///
/// This is the no-churn rule (ADR-092 D2) as a measurement rather than a
/// convention. Every setter diffs before writing; if one ever stops, an idle
/// save loop turns into unbounded growth and nothing else would catch it.
#[test]
fn setting_fields_to_their_current_values_does_not_grow_the_document() {
    let mut doc = KbNodeDoc::new("n1", "Title", "Body", &["tag".into()]);
    let _ = doc.set_kind(Some("concept"));
    let _ = doc.set_todo_state(Some("TODO"));
    let _ = doc.set_priority(Some("A"));
    let _ = doc.set_aliases(&["alias".into()]);
    let mut props = std::collections::HashMap::new();
    props.insert("k".to_string(), "v".to_string());
    let _ = doc.set_properties(&props);

    let baseline = doc.encode_state().len();

    for _ in 0..200 {
        let _ = doc.set_title("Title");
        let _ = doc.set_body("Body");
        let _ = doc.set_tags(&["tag".into()]);
        let _ = doc.set_kind(Some("concept"));
        let _ = doc.set_todo_state(Some("TODO"));
        let _ = doc.set_priority(Some("A"));
        let _ = doc.set_aliases(&["alias".into()]);
        let _ = doc.set_properties(&props);
    }

    assert_eq!(
        doc.encode_state().len(),
        baseline,
        "1600 no-op setter calls changed the document size — a setter stopped \
         diffing before writing, and an idle save loop is now unbounded growth"
    );
}

/// `schema_v` is a constant, so a document that changes fields many times must
/// not pay for it many times.
///
/// It used to be re-inserted on EVERY real field change, making it the hottest
/// key in the document. Each overwrite of a `Y.Map` key retires the previous
/// Item permanently — yrs cannot reclaim those — so the stamp cost scaled with
/// edit count for zero information.
///
/// Measured on this change, 100 alternating scalar edits:
///
/// | | bytes/edit | total |
/// |---|---|---|
/// | re-stamping every change | **23.03** | 2303 |
/// | stamping only when it changes | **0.41** | 41 |
///
/// A **56x** reduction, because the constant dominated the actual edit. The
/// bound below sits between the two so it fails on a regression rather than
/// merely describing today — an earlier draft used 40 bytes, which passed under
/// BOTH behaviours and therefore proved nothing.
#[test]
fn a_constant_schema_stamp_is_not_rewritten_on_every_edit() {
    let mut doc = KbNodeDoc::new("n1", "T", "B", &[]);
    let _ = doc.set_kind(Some("concept")); // first real change stamps schema_v
    assert_eq!(
        doc.schema_version(),
        2,
        "the stamp is present after a v2 write"
    );

    let after_first = doc.encode_state().len();

    // 100 DISTINCT changes — each is a real edit, so each legitimately grows the
    // document. What must not grow is a second cost per edit for the constant.
    for i in 0..100 {
        let _ = doc.set_todo_state(Some(if i % 2 == 0 { "TODO" } else { "DONE" }));
    }
    let per_edit = (doc.encode_state().len() - after_first) as f64 / 100.0;

    assert_eq!(doc.schema_version(), 2, "the stamp is still correct");
    assert!(
        per_edit < 2.0,
        "each edit cost {per_edit:.2} bytes; measured 0.41 when the constant is \
         stamped once and 23.03 when it is rewritten per edit, so this is a \
         regression to unconditional stamping"
    );
}

/// The stamp must still appear for a document that has never had one — otherwise
/// making the write conditional would silently leave v2 documents reading as v1,
/// which is the failure mode a naive "only write once" would introduce.
#[test]
fn a_v1_document_still_gains_the_stamp_on_its_first_v2_write() {
    let mut doc = KbNodeDoc::new("n1", "T", "B", &[]);
    assert_eq!(
        doc.schema_version(),
        1,
        "a fresh text-only document reads as v1 — the stamp is written lazily"
    );

    let _ = doc.set_priority(Some("B"));

    assert_eq!(
        doc.schema_version(),
        2,
        "the first v2 write must stamp the version, or tolerant readers will \
         treat a v2 document as v1 forever"
    );
}
