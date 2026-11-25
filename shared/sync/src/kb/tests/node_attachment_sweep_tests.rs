//! The sweep's reader finds a hash wherever an entry holds it, within a
//! bound past which the document is unreadable rather than empty (ADR-115
//! D5); rebirth copies the `attachments` root as stored, so no reference the
//! sweep needs is lost with it.

use std::collections::BTreeSet;

use yrs::types::ToJson;
use yrs::{ArrayPrelim, In, MapPrelim, Out, TextPrelim};

use super::*;
use crate::kb::{UnwalkableAttachments, MAX_REFERENCE_DEPTH, MAX_REFERENCE_VALUES};

/// A node with one valid entry, `ok.png` bound to `sha(1)`.
fn honest() -> Vec<u8> {
    let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    let _ = n.set_attachment("ok.png", &png(1)).unwrap();
    n.encode()
}

/// A peer's update putting `value`, of any shape, at `name`.
fn put(base: &[u8], name: &str, value: In) -> Vec<u8> {
    let doc = KbNodeDoc::from_bytes_with_client_id(base, 668).unwrap();
    let sv = doc.state_vector();
    let map = doc.doc().get_or_insert_map(ATTACHMENTS_ROOT);
    map.insert(&mut doc.doc().transact_mut(), name, value);
    doc.encode_diff(&sv).unwrap()
}

fn with_update(base: &[u8], update: &[u8]) -> KbNodeDoc {
    let mut d = KbNodeDoc::from_bytes(base).unwrap();
    d.apply_update(update).unwrap();
    d
}

fn map(fields: &[(&str, In)]) -> In {
    In::Map(MapPrelim::from_iter(
        fields.iter().map(|(k, v)| (k.to_string(), v.clone())),
    ))
}

/// Every shape a peer could hold hash `h` in: any key, any depth, any
/// container, with or without surrounding whitespace.
fn holders(h: &str) -> Vec<(&'static str, In)> {
    let s = |v: &str| In::Any(Any::from(v));
    let keyed = |k: &str| In::Any(entry(&[(k, Any::from(h))]));
    let text = || In::from(TextPrelim::new(h));
    vec![
        ("a bare string", s(h)),
        (
            "blob.sha256",
            In::Any(entry(&[("blob", entry(&[("sha256", Any::from(h))]))])),
        ),
        ("a sha key", keyed("sha")),
        ("a hash key", keyed("hash")),
        ("a digest key", keyed("digest")),
        ("a Y.Text", text()),
        ("whitespace around it", s(&format!(" \t{h}\r\n"))),
        ("uppercase", s(&h.to_uppercase())),
        (
            "an array of maps",
            In::Any(Any::Array(
                vec![Any::Null, entry(&[("variant", Any::from(h))])].into(),
            )),
        ),
        (
            "a Y.Array holding a Y.Map holding a Y.Text",
            In::Array(ArrayPrelim::from([map(&[("inner", text())])])),
        ),
        (
            "a Y.Map, three deep",
            map(&[("a", map(&[("b", map(&[("c", s(h))]))]))]),
        ),
        ("the hash as a key", In::Any(entry(&[(h, Any::Bool(true))]))),
    ]
}

/// Every shape a peer could hold a hash in, at any depth and under any key,
/// is counted by the sweep's reader, lowercased; a string that is not a hash
/// adds nothing.
#[test]
fn the_sweep_reader_finds_a_hash_wherever_an_entry_holds_it() {
    let base = honest();
    // Letters in it, so the uppercase case is not the same string.
    let h = sha(0xab);
    let s = |v: &str| In::Any(Any::from(v));
    let mut cases: Vec<(&str, In, Option<String>)> = holders(&h)
        .into_iter()
        .map(|(what, v)| (what, v, Some(h.clone())))
        .collect();
    cases.extend([
        (
            "the raw 32 bytes",
            In::Any(Any::Buffer([0xab_u8; 32].into())),
            Some(h.clone()),
        ),
        ("63 digits", s(&h[1..]), None),
        ("65 digits", s(&format!("{h}0")), None),
        (
            "inner whitespace",
            s(&format!("{} {}", &h[..32], &h[32..])),
            None,
        ),
        ("not hex", s(&"g".repeat(64)), None),
    ]);
    for (what, value, hash) in cases {
        let doc = with_update(&base, &put(&base, "x.png", value));
        let mut expected: BTreeSet<String> = [sha(1)].into();
        expected.extend(hash);
        assert_eq!(doc.referenced_hashes(), Ok(expected), "{what}");
        // The display reader rejects every one: the two readers fail in
        // opposite directions.
        assert_eq!(doc.attachments().len(), 1, "{what}");
    }
}

/// Any::Map nested `depth` levels, a hash at the bottom.
fn nested_any(depth: usize) -> Any {
    let mut v = Any::from(sha(7).as_str());
    for _ in 0..depth {
        v = entry(&[("k", v)]);
    }
    v
}

/// Y.Map nested `depth` levels, a hash at the bottom.
fn nested_ymap(depth: usize) -> In {
    let mut v = In::Any(Any::from(sha(7).as_str()));
    for _ in 0..depth {
        v = map(&[("k", v)]);
    }
    v
}

/// A hostile entry that is too deep or too wide makes the document
/// unreadable, never a smaller set; one just inside the bound is read in
/// full. The bound counts the whole document, so many entries that are each
/// small but together too many trip it too.
#[test]
fn a_document_past_the_walk_bound_is_unreadable_not_empty() {
    let base = honest();
    // The entry's value is depth 1, each map level adds one, and the hash
    // under the last map is one deeper.
    let at_bound = MAX_REFERENCE_DEPTH - 1;
    for (what, value) in [
        ("Any::Map", In::Any(nested_any(at_bound))),
        ("Y.Map", nested_ymap(at_bound)),
    ] {
        let doc = with_update(&base, &put(&base, "deep.png", value));
        assert_eq!(
            doc.referenced_hashes(),
            Ok([sha(1), sha(7)].into()),
            "{what} at the bound"
        );
    }
    for (what, value) in [
        ("Any::Map", In::Any(nested_any(at_bound + 1))),
        ("Y.Map", nested_ymap(at_bound + 1)),
        ("Any::Map, far past", In::Any(nested_any(500))),
    ] {
        let doc = with_update(&base, &put(&base, "deep.png", value));
        assert_eq!(
            doc.referenced_hashes(),
            Err(UnwalkableAttachments::TooDeep),
            "{what}"
        );
    }

    // The root holds `ok.png`'s map and its 3 fields (4 values) and the wide
    // entry's array (1), so the array may hold the rest.
    let room = MAX_REFERENCE_VALUES - 5;
    let wide = |n: usize| {
        let mut items = vec![Any::Null; n];
        items[n - 1] = Any::from(sha(8).as_str());
        In::Any(Any::Array(items.into()))
    };
    let doc = with_update(&base, &put(&base, "wide.png", wide(room)));
    assert_eq!(doc.referenced_hashes(), Ok([sha(1), sha(8)].into()));
    let doc = with_update(&base, &put(&base, "wide.png", wide(room + 1)));
    assert_eq!(
        doc.referenced_hashes(),
        Err(UnwalkableAttachments::TooLarge)
    );
    // Many small entries, none wide on its own.
    let many = KbNodeDoc::from_bytes(&base).unwrap();
    let per = 1000;
    let entries = MAX_REFERENCE_VALUES / per + 1;
    {
        let root = many.doc().get_or_insert_map(ATTACHMENTS_ROOT);
        let mut txn = many.doc().transact_mut();
        for i in 0..entries {
            let items = vec![Any::Null; per - 1];
            root.insert(&mut txn, format!("e{i}"), Any::Array(items.into()));
        }
    }
    assert_eq!(
        many.referenced_hashes(),
        Err(UnwalkableAttachments::TooLarge)
    );
}

// --- rebirth copies the root raw -------------------------------------------

/// Each root value of `d`, by key: a plain value as stored, a shared type as
/// the JSON it reads as.
fn raw_root(d: &KbNodeDoc) -> BTreeMap<String, (bool, Any)> {
    let txn = d.doc().transact();
    let Some(root) = txn.get_map(ATTACHMENTS_ROOT) else {
        return BTreeMap::new();
    };
    root.iter(&txn)
        .map(|(k, v)| {
            let plain = matches!(v, Out::Any(_));
            (k.to_string(), (plain, v.to_json(&txn)))
        })
        .collect()
}

/// A legacy NAME, a malformed entry, a newer peer's entry with extra fields
/// and a shared-type entry are all reborn (ADR-107) exactly as stored,
/// beside the valid entry: the same keys, the same values, so the same
/// references for the sweep and the same view for the display reader.
#[test]
fn rebirth_carries_every_entry_of_the_root_as_stored() {
    let base = honest();
    let mut doc = KbNodeDoc::from_bytes(&base).unwrap();
    let mut newer = good_fields();
    newer.push(("thumb", entry(&[("sha256", Any::from(sha(5).as_str()))])));
    newer.push(("alt", Any::from("a cat")));
    // Each from the document as it stands, so the peers' writes chain.
    type Write = Box<dyn Fn(&[u8]) -> Vec<u8>>;
    let writes: Vec<Write> = vec![
        Box::new(|b| hostile(b, "a/b.png", entry(&good_fields()))),
        Box::new(|b| {
            hostile(
                b,
                "CON.png",
                with("sha256", Some(Any::from(sha(2).as_str()))),
            )
        }),
        Box::new(|b| hostile(b, "m.png", with("size", Some(Any::from("5"))))),
        Box::new(move |b| hostile(b, "n.png", entry(&newer))),
        Box::new(|b| {
            put(
                b,
                "y.png",
                map(&[("blob", map(&[("sha256", In::from(sha(6)))]))]),
            )
        }),
    ];
    for write in writes {
        let update = write(&doc.encode());
        doc.apply_update(&update).unwrap();
    }
    let read = doc.read_attachments();
    assert_eq!(read.entries.len(), 2, "ok.png and n.png");
    assert_eq!(read.problems.len(), 5, "4 rejected, n.png's extra fields");

    let reborn = doc.reborn(99).unwrap();
    assert_eq!(raw_root(&reborn), raw_root(&doc));
    assert_eq!(raw_root(&doc).len(), 6);
    assert_eq!(
        reborn.referenced_hashes(),
        Ok([sha(1), sha(2), sha(5), sha(6), sha(9)].into())
    );
    assert_eq!(reborn.referenced_hashes(), doc.referenced_hashes());
    assert_eq!(reborn.read_attachments(), read);
    assert_eq!(reborn.rebirth_hash(), doc.rebirth_hash());
    // A shared type stays a shared type.
    let txn = reborn.doc().transact();
    let y = txn.get_map(ATTACHMENTS_ROOT).unwrap().get(&txn, "y.png");
    assert!(matches!(y, Some(Out::YMap(_))), "{y:?}");
}

/// A node whose root the sweep's reader cannot walk in full is not reborn:
/// a copy that could miss an entry is refused rather than made.
#[test]
fn a_root_past_the_walk_bound_is_not_reborn() {
    let base = honest();
    let doc = with_update(
        &base,
        &put(&base, "deep.png", In::Any(nested_any(MAX_REFERENCE_DEPTH))),
    );
    assert_eq!(doc.reborn(2).err(), Some(UnwalkableAttachments::TooDeep));
}
