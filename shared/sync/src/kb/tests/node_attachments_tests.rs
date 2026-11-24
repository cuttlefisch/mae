//! ADR-115 D4: a node's `attachments` map. Old documents read unchanged; the
//! map is part of the node's version; concurrent writers converge; a hostile
//! peer's malformed entry is reported and ignored, never fatal and never at
//! the expense of a valid entry; the NAME rule is one plain file name.

use std::collections::BTreeMap;

use proptest::prelude::*;
use yrs::{Any, Doc, Map, ReadTxn, Text, Transact};

use super::*;
use crate::kb::history::{decode_content, encode_content};

/// The NAME rule, collisions, the sweep's reader and the per-entry type check.
#[path = "node_attachment_names_tests.rs"]
mod names;

/// A node document stored by the code before the map existed (commit
/// 6c77b740, `KbNodeDoc::new_with_client_id` and three v2 setters), with the
/// version, rebirth hash and history content hash that code computed for it.
const BEFORE: &str = include_str!("data/node_v2_before_attachments.hex");
const BEFORE_VERSION: &str = "c3e2a1f8ae56a6ce573b9dfba16a6ad2";
const BEFORE_REBIRTH: &str = "403120b37e0febdc4012d2b55bca8b2d003d5ba7c1fd69df71504a22b6654e64";
const BEFORE_CONTENT: &str = "9a83c8b7484eec93a149f20b2a88e6e28b40e189b87619f237504bd92fbb3894";

fn before_bytes() -> Vec<u8> {
    hex::decode(BEFORE.trim()).unwrap()
}

fn sha(seed: u8) -> String {
    hex::encode([seed; 32])
}

fn png(seed: u8) -> AttachmentRef {
    AttachmentRef::new(sha(seed), 1000 + seed as u64, "image/png").unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

fn root_count(n: &KbNodeDoc) -> usize {
    n.doc().transact().root_refs().count()
}

// --- old documents -------------------------------------------------------

#[test]
fn a_document_stored_before_the_map_opens_reads_and_versions_as_before() {
    let bytes = before_bytes();
    let n = KbNodeDoc::from_bytes(&bytes).unwrap();
    let roots = root_count(&n);
    assert_eq!(n.read_attachments(), AttachmentsRead::default());
    assert!(n.attachment("a.png").is_none());
    assert!(n.materialize().attachments.is_empty());
    assert_eq!(n.version(), BEFORE_VERSION);
    assert_eq!(n.rebirth_hash(), BEFORE_REBIRTH);
    assert_eq!(
        sha256_hex(&encode_content(&n.materialize())),
        BEFORE_CONTENT
    );
    assert!(n.field_type_violations().is_empty());
    // Reading authored nothing and created no root (ADR-093 D3).
    assert_eq!(n.encode(), bytes, "a read must not change the document");
    assert_eq!(root_count(&n), roots, "a read must not create the root");
}

/// The case a nested map at `node.attachments` would lose (ADR-093 D4): three
/// peers attach the FIRST images to an old node concurrently. With no
/// container to create, all three survive.
#[test]
fn three_peers_attaching_first_images_to_an_old_node_all_survive() {
    let base = before_bytes();
    let mut peers: Vec<KbNodeDoc> = (0..3)
        .map(|i| KbNodeDoc::from_bytes_with_client_id(&base, 10 + i).unwrap())
        .collect();
    let updates: Vec<Vec<u8>> = peers
        .iter_mut()
        .enumerate()
        .map(|(i, p)| {
            p.set_attachment(&format!("p{i}.png"), &png(i as u8))
                .unwrap()
        })
        .collect();
    for (i, p) in peers.iter_mut().enumerate() {
        for (j, u) in updates.iter().enumerate() {
            if i != j {
                p.apply_update(u).unwrap();
            }
        }
    }
    let expected: BTreeMap<String, AttachmentRef> =
        (0..3).map(|i| (format!("p{i}.png"), png(i))).collect();
    for p in &peers {
        assert_eq!(p.attachments(), expected);
    }
}

// --- version ---------------------------------------------------------------

#[test]
fn the_version_moves_on_replace_rename_and_remove_and_not_on_a_no_op() {
    let mut n = KbNodeDoc::new_with_client_id("n", "T", "[[attachment:d.png]]", &[], 7);
    let bare = n.version();
    let _ = n.set_attachment("d.png", &png(1)).unwrap();
    let one = n.version();
    assert_ne!(one, bare, "adding an image is a new version");

    let sv = n.state_vector();
    let _ = n.set_attachment("d.png", &png(1)).unwrap();
    assert_eq!(n.state_vector(), sv, "a no-op set authors no op");
    assert_eq!(n.version(), one);

    let _ = n.set_attachment("d.png", &png(2)).unwrap();
    let replaced = n.version();
    assert_ne!(
        replaced, one,
        "a new image under the same name is a new version"
    );

    let _ = n.rename_attachment("d.png", "e.png").unwrap();
    let renamed = n.version();
    assert_ne!(renamed, replaced, "a rename is a new version");
    assert_eq!(n.attachments().keys().collect::<Vec<_>>(), ["e.png"]);

    let sv = n.state_vector();
    let _ = n.rename_attachment("e.png", "e.png").unwrap();
    let _ = n.remove_attachment("absent.png");
    assert_eq!(
        n.state_vector(),
        sv,
        "a no-op rename or remove authors no op"
    );

    let _ = n.remove_attachment("e.png");
    assert_eq!(
        n.version(),
        bare,
        "with the map empty again, the node is as it was"
    );
}

#[test]
fn equal_text_with_different_maps_is_a_different_version() {
    let make = |size: u64, media: &str| {
        let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 7);
        let a = AttachmentRef::new(sha(1), size, media).unwrap();
        let _ = n.set_attachment("d.png", &a).unwrap();
        n.version()
    };
    let base = make(10, "image/png");
    assert_ne!(base, make(11, "image/png"), "size");
    assert_ne!(base, make(10, "image/jpeg"), "media type");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Two maps differ iff the versions of otherwise equal nodes differ, and
    /// the digest does not depend on insertion order.
    #[test]
    fn the_version_tells_maps_apart(
        a in prop::collection::btree_map(0u8..4, 0u8..3, 0..4),
        b in prop::collection::btree_map(0u8..4, 0u8..3, 0..4),
    ) {
        let doc = |m: &BTreeMap<u8, u8>, rev: bool| {
            let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 7);
            let mut items: Vec<_> = m.iter().collect();
            if rev { items.reverse(); }
            for (name, h) in items {
                let _ = n.set_attachment(&format!("{name}.png"), &png(*h)).unwrap();
            }
            n.version()
        };
        prop_assert_eq!(doc(&a, false), doc(&a, true));
        prop_assert_eq!(a == b, doc(&a, false) == doc(&b, false));
    }
}

#[test]
fn an_attachments_only_remote_update_is_reported_as_a_change() {
    let mut a = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    let mut b = KbNodeDoc::from_bytes_with_client_id(&a.encode(), 2).unwrap();
    let u = a.set_attachment("d.png", &png(1)).unwrap();
    assert!(b.apply_update(&u).unwrap());
    assert_eq!(b.attachments(), a.attachments());
}

#[test]
fn rebirth_carries_the_map_and_its_hash_covers_it() {
    let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    let bare = n.rebirth_hash();
    let _ = n.set_attachment("d.png", &png(1)).unwrap();
    let _ = n.set_attachment("e.svg", &png(2)).unwrap();
    assert_ne!(n.rebirth_hash(), bare);
    let reborn = n.reborn(99);
    assert_eq!(reborn.attachments(), n.attachments());
    assert_eq!(reborn.rebirth_hash(), n.rebirth_hash());
    assert_eq!(reborn.version(), n.version());
}

// --- convergence -----------------------------------------------------------

/// A seeded shuffle (xorshift), so a failing order is reproducible from its seed.
fn shuffle<T>(seed: u64, v: &mut [T]) {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for i in (1..v.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

/// `n` peers from one base, each making its own edit concurrently, then each
/// receiving every update (its own included, a no-op) in a seeded order.
fn converge(
    n: usize,
    seed: u64,
    edit: impl Fn(usize, &mut KbNodeDoc) -> Vec<u8>,
) -> Vec<BTreeMap<String, AttachmentRef>> {
    let mut origin = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    let _ = origin.set_attachment("keep.png", &png(200)).unwrap();
    let _ = origin.set_attachment("drop.png", &png(201)).unwrap();
    let base = origin.encode();
    let mut peers: Vec<KbNodeDoc> = (0..n)
        .map(|i| KbNodeDoc::from_bytes_with_client_id(&base, 100 + i as u64 * 7919).unwrap())
        .collect();
    let updates: Vec<Vec<u8>> = peers
        .iter_mut()
        .enumerate()
        .map(|(i, p)| edit(i, p))
        .collect();
    for (i, p) in peers.iter_mut().enumerate() {
        let mut order: Vec<usize> = (0..n).collect();
        shuffle(seed ^ (i as u64 + 1), &mut order);
        for j in order {
            p.apply_update(&updates[j]).unwrap();
        }
    }
    peers.iter().map(KbNodeDoc::attachments).collect()
}

#[test]
fn concurrent_edits_to_different_names_merge_everywhere() {
    for n in [3usize, 5] {
        for seed in 0..40u64 {
            let maps = converge(n, seed, |i, p| {
                if i == 0 {
                    // One peer removes an entry nobody else touches.
                    p.remove_attachment("drop.png")
                } else {
                    p.set_attachment(&format!("p{i}.png"), &png(i as u8))
                        .unwrap()
                }
            });
            let mut expected: BTreeMap<String, AttachmentRef> = (1..n)
                .map(|i| (format!("p{i}.png"), png(i as u8)))
                .collect();
            expected.insert("keep.png".into(), png(200));
            for m in &maps {
                assert_eq!(m, &expected, "n={n} seed={seed}");
            }
        }
    }
}

#[test]
fn concurrent_edits_to_one_name_converge_to_one_winner_everywhere() {
    for n in [3usize, 5] {
        let mut winners = std::collections::BTreeSet::new();
        for seed in 0..40u64 {
            let maps = converge(n, seed, |i, p| {
                p.set_attachment("keep.png", &png(i as u8)).unwrap()
            });
            let winner = maps[0]["keep.png"].clone();
            for m in &maps {
                assert_eq!(m, &maps[0], "n={n} seed={seed}: peers disagree");
            }
            assert!(
                (0..n).any(|i| png(i as u8) == winner),
                "the winner is one of the writes, not the old value"
            );
            winners.insert(winner);
        }
        // Last-writer-wins is decided by the CRDT, not by the apply order: the
        // same concurrent writes give the same winner whatever order arrives.
        assert_eq!(
            winners.len(),
            1,
            "n={n}: the winner depended on apply order"
        );
    }
}

// --- hostile peers ---------------------------------------------------------

/// An update a hostile peer makes from `base`: a raw value at `name`.
fn hostile(base: &[u8], name: &str, value: Any) -> Vec<u8> {
    let doc = KbNodeDoc::from_bytes_with_client_id(base, 666).unwrap();
    let sv = doc.state_vector();
    let map = doc.doc().get_or_insert_map(ATTACHMENTS_ROOT);
    {
        let mut txn = doc.doc().transact_mut();
        map.insert(&mut txn, name, value);
    }
    doc.encode_diff(&sv).unwrap()
}

fn entry(fields: &[(&str, Any)]) -> Any {
    Any::Map(std::sync::Arc::new(
        fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    ))
}

fn good_fields() -> Vec<(&'static str, Any)> {
    vec![
        ("sha256", Any::from(sha(9).as_str())),
        ("size", Any::Number(5.0)),
        ("media_type", Any::from("image/png")),
    ]
}

fn with(field: &'static str, value: Option<Any>) -> Any {
    let mut f: Vec<(&str, Any)> = good_fields()
        .into_iter()
        .filter(|(k, _)| *k != field)
        .collect();
    if let Some(v) = value {
        f.push((field, v));
    }
    entry(&f)
}

/// Every malformed shape, each named for what is wrong with it.
fn malformed() -> Vec<(&'static str, String, Any)> {
    let ok = |n: &str| n.to_string();
    vec![
        ("missing hash", ok("m1"), with("sha256", None)),
        (
            "non-hex hash",
            ok("m2"),
            with("sha256", Some(Any::from("z".repeat(64).as_str()))),
        ),
        (
            "short hash",
            ok("m3"),
            with("sha256", Some(Any::from(&sha(9)[..63]))),
        ),
        (
            "long hash",
            ok("m4"),
            with("sha256", Some(Any::from(format!("{}0", sha(9)).as_str()))),
        ),
        (
            "uppercase hash",
            ok("m5"),
            with("sha256", Some(Any::from(sha(0xab).to_uppercase().as_str()))),
        ),
        (
            "hash not a string",
            ok("m6"),
            with("sha256", Some(Any::Number(1.0))),
        ),
        (
            "huge size",
            ok("m7"),
            with("size", Some(Any::Number(1e300))),
        ),
        (
            "size 2^53",
            ok("m8"),
            with("size", Some(Any::Number(9_007_199_254_740_992.0))),
        ),
        (
            "huge bigint size",
            ok("m9"),
            with("size", Some(Any::BigInt(i64::MAX))),
        ),
        (
            "negative size",
            ok("m10"),
            with("size", Some(Any::Number(-1.0))),
        ),
        (
            "fractional size",
            ok("m11"),
            with("size", Some(Any::Number(1.5))),
        ),
        (
            "NaN size",
            ok("m12"),
            with("size", Some(Any::Number(f64::NAN))),
        ),
        ("missing size", ok("m13"), with("size", None)),
        (
            "bad media type",
            ok("m14"),
            with("media_type", Some(Any::from("text/html; x=\"<\""))),
        ),
        ("missing media type", ok("m15"), with("media_type", None)),
        ("not a map", ok("m16"), Any::from("sha256")),
        ("null", ok("m17"), Any::Null),
        ("slash in name", ok("a/b.png"), entry(&good_fields())),
        ("traversal name", ok(".."), entry(&good_fields())),
        ("backslash in name", ok("a\\b.png"), entry(&good_fields())),
        ("control in name", ok("a\u{0}b.png"), entry(&good_fields())),
        ("empty name", ok(""), entry(&good_fields())),
        ("long name", "x".repeat(256), entry(&good_fields())),
    ]
}

#[test]
fn a_malformed_entry_from_sync_is_reported_ignored_and_harms_no_valid_entry() {
    let mut honest = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    let _ = honest.set_attachment("ok.png", &png(1)).unwrap();
    let _ = honest.set_attachment("also.png", &png(2)).unwrap();
    let valid = honest.attachments();
    let version = honest.version();
    let base = honest.encode();
    for (what, name, value) in malformed() {
        let mut peer = KbNodeDoc::from_bytes(&base).unwrap();
        let update = hostile(&base, &name, value);
        peer.apply_update(&update).unwrap();
        let read = peer.read_attachments();
        assert_eq!(read.entries, valid, "{what}: a valid entry changed");
        assert_eq!(peer.attachments(), valid, "{what}");
        assert_eq!(peer.version(), version, "{what}: readers see the same node");
        assert!(
            read.problems
                .iter()
                .any(|p| matches!(p, AttachmentProblem::Rejected { name: n, .. } if *n == name)),
            "{what}: not reported: {:?}",
            read.problems
        );
        assert!(peer.attachment(&name).is_none(), "{what}");
        // The daemon refuses to let a writer introduce it.
        assert_eq!(
            KbNodeDoc::field_type_introduced(&base, &update).unwrap(),
            Some("attachments"),
            "{what}"
        );
        // A reader never rewrites it: the document is what arrived.
        let encoded = peer.encode();
        let _ = peer.read_attachments();
        assert_eq!(peer.encode(), encoded, "{what}: reading wrote");
        // And it can be cleared by name.
        let _ = peer.remove_attachment(&name);
        assert!(peer.read_attachments().problems.is_empty(), "{what}");
    }
}

#[test]
fn unknown_fields_are_reported_and_the_entry_still_reads() {
    let base = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1).encode();
    let mut fields = good_fields();
    fields.push(("width", Any::Number(640.0)));
    let update = hostile(&base, "new.png", entry(&fields));
    let mut peer = KbNodeDoc::from_bytes(&base).unwrap();
    peer.apply_update(&update).unwrap();
    let read = peer.read_attachments();
    assert_eq!(
        read.entries.get("new.png"),
        Some(&AttachmentRef::new(sha(9), 5, "image/png").unwrap())
    );
    assert_eq!(
        read.problems,
        vec![AttachmentProblem::UnknownFields {
            name: "new.png".into(),
            fields: vec!["width".into()]
        }]
    );
    assert_eq!(
        KbNodeDoc::field_type_introduced(&base, &update).unwrap(),
        None
    );
    // A no-op set keeps the newer client's field.
    let sv = peer.state_vector();
    let _ = peer.set_attachment(
        "new.png",
        &AttachmentRef::new(sha(9), 5, "image/png").unwrap(),
    );
    assert_eq!(peer.state_vector(), sv);
}

/// A size written as a JS BigInt, or as an integral double, reads the same.
#[test]
fn a_size_reads_the_same_however_the_writer_encoded_it() {
    let base = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1).encode();
    for size in [Any::Number(5.0), Any::BigInt(5)] {
        let update = hostile(&base, "s.png", with("size", Some(size)));
        let mut peer = KbNodeDoc::from_bytes(&base).unwrap();
        peer.apply_update(&update).unwrap();
        assert_eq!(peer.attachment("s.png").unwrap().size, 5);
    }
}

#[test]
fn a_root_of_another_type_reads_as_an_empty_map_without_panicking() {
    use yrs::updates::decoder::Decode;
    let base = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1).encode();
    let doc = Doc::with_client_id(666);
    doc.transact_mut()
        .apply_update(yrs::Update::decode_v1(&base).unwrap())
        .unwrap();
    let sv = doc.transact().state_vector();
    let text = doc.get_or_insert_text(ATTACHMENTS_ROOT);
    text.insert(&mut doc.transact_mut(), 0, "not a map");
    let update = doc.transact().encode_state_as_update_v1(&sv);
    let mut peer = KbNodeDoc::from_bytes(&base).unwrap();
    peer.apply_update(&update).unwrap();
    assert!(peer.attachments().is_empty());
    let _ = peer.set_attachment("d.png", &png(1)).unwrap();
    assert_eq!(peer.attachment("d.png"), Some(png(1)));
}

#[test]
fn a_writer_refuses_a_bad_name_or_entry_and_writes_nothing() {
    let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    let sv = n.state_vector();
    assert!(n.set_attachment("a/b.png", &png(1)).is_err());
    let bad = AttachmentRef {
        sha256: "nope".into(),
        size: 1,
        media_type: "image/png".into(),
    };
    assert_eq!(
        n.set_attachment("a.png", &bad),
        Err(AttachmentError::Sha256)
    );
    assert_eq!(
        n.rename_attachment("missing.png", "b.png"),
        Err(AttachmentError::NoSuchAttachment)
    );
    let _ = n.set_attachment("a.png", &png(1)).unwrap();
    let sv2 = n.state_vector();
    assert!(n.rename_attachment("a.png", "..").is_err());
    assert_eq!(n.state_vector(), sv2);
    assert_ne!(sv, sv2);
}

// --- history content -------------------------------------------------------

#[test]
fn a_history_version_keeps_its_own_map_when_the_node_changes() {
    let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    let _ = n.set_attachment("d.png", &png(1)).unwrap();
    let frozen = encode_content(&n.materialize());
    let _ = n.set_attachment("d.png", &png(2)).unwrap();
    assert_ne!(encode_content(&n.materialize()), frozen);
    let back = decode_content(&frozen).unwrap();
    assert_eq!(back.attachments["d.png"], png(1));
}

#[test]
fn stored_content_accepts_only_the_canonical_attachments_section() {
    let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    let plain = encode_content(&n.materialize());
    let _ = n.set_attachment("a.png", &png(1)).unwrap();
    let _ = n.set_attachment("b.png", &png(2)).unwrap();
    let good = encode_content(&n.materialize());
    assert!(decode_content(&good).is_ok());

    let field = |b: &[u8]| [(b.len() as u64).to_le_bytes().as_slice(), b].concat();
    let section = |entries: &[(&str, &str, u64, &str)]| {
        let mut out = field(b"attachments");
        out.extend(field(&(entries.len() as u64).to_le_bytes()));
        for (name, h, size, media) in entries {
            out.extend(field(name.as_bytes()));
            out.extend(field(h.as_bytes()));
            out.extend(field(&size.to_le_bytes()));
            out.extend(field(media.as_bytes()));
        }
        out
    };
    let (h1, h2) = (sha(1), sha(2));
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty section", section(&[])),
        (
            "unordered",
            section(&[
                ("b.png", &h2, 1, "image/png"),
                ("a.png", &h1, 1, "image/png"),
            ]),
        ),
        (
            "duplicate",
            section(&[
                ("a.png", &h1, 1, "image/png"),
                ("a.png", &h1, 1, "image/png"),
            ]),
        ),
        ("bad name", section(&[("a/b", &h1, 1, "image/png")])),
        ("bad hash", section(&[("a.png", "00", 1, "image/png")])),
        (
            "bad size",
            section(&[("a.png", &h1, u64::MAX, "image/png")]),
        ),
        ("bad media type", section(&[("a.png", &h1, 1, "PNG")])),
        (
            "wrong magic",
            [field(b"attachmentz"), field(&1u64.to_le_bytes())].concat(),
        ),
    ];
    for (what, tail) in cases {
        let bytes = [plain.clone(), tail].concat();
        assert!(decode_content(&bytes).is_err(), "{what} was accepted");
    }
}
