//! ADR-116 P3, the signed head: it verifies only as signed, and a chain
//! checked against it fails when a tip it names is gone or different. Later
//! versions are expected; a consistently re-sealed history is not.

use std::collections::BTreeMap;

use super::*;
use crate::kb::history::*;

const KB: &str = "kb-1";
const SEED: [u8; 32] = [7; 32];

fn key() -> HistoryKey {
    HistoryKey::from_bytes([5; 32])
}

fn chain(node: &str, titles: &[&str]) -> Vec<HistoryEntry> {
    let mut out: Vec<HistoryEntry> = Vec::new();
    for (i, t) in titles.iter().enumerate() {
        let d = Draft {
            kind: VersionKind::Edit,
            created_at: 1_700_000_000 + i as i64,
            editors: Vec::new(),
            wal_from: i as u64,
            wal_to: i as u64 + 1,
            summary: None,
            name: None,
            restored_from: None,
            content: Some(KbNodeDoc::new_created_at(node, t, "", &[], 1).materialize()),
        };
        let e = seal(KB, node, out.last(), d, &key()).unwrap();
        out.push(e);
    }
    out
}

fn tips(chains: &BTreeMap<String, Vec<HistoryEntry>>) -> Vec<Tip> {
    chains
        .iter()
        .filter_map(|(n, c)| {
            c.last().map(|t| Tip {
                node_id: n.clone(),
                seq: t.seq,
                entry_hash: t.entry_hash,
            })
        })
        .collect()
}

fn kb() -> BTreeMap<String, Vec<HistoryEntry>> {
    BTreeMap::from([
        ("a".to_string(), chain("a", &["a1", "a2", "a3"])),
        ("b".to_string(), chain("b", &["b1"])),
    ])
}

/// A head verifies as signed; any change to what it covers does not.
#[test]
fn a_head_verifies_only_as_signed() {
    let chains = kb();
    let head = SignedHead::sign(KB, 100, tips(&chains), &SEED).unwrap();
    assert!(head.verify_signature());
    let mut moved = head.clone();
    moved.tips[0].seq -= 1;
    assert!(
        !moved.verify_signature(),
        "a tip changed under the signature"
    );
    let mut later = head.clone();
    later.at += 1;
    assert!(!later.verify_signature(), "the signing time is covered");
    let mut other = head.clone();
    other.signer = SignedHead::sign(KB, 100, tips(&chains), &[8; 32])
        .unwrap()
        .signer;
    assert!(
        !other.verify_signature(),
        "another key's name on this signature"
    );
    assert_eq!(
        verify_against_head(&head, "kb-2", &chains, Some(&key())),
        Err(HeadError::Kb)
    );
}

/// Versions cut after the head are expected; a removed tip, a node's history
/// gone, and a history re-sealed consistently from an earlier version are
/// not. The last is what a chain alone cannot see.
#[test]
fn a_chain_is_checked_as_a_prefix_of_the_head() {
    let chains = kb();
    let head = SignedHead::sign(KB, 100, tips(&chains), &SEED).unwrap();
    assert_eq!(
        verify_against_head(&head, KB, &chains, Some(&key())),
        Ok(())
    );

    let mut grown = chains.clone();
    grown.insert("a".into(), chain("a", &["a1", "a2", "a3", "a4"]));
    grown.insert("c".into(), chain("c", &["c1"]));
    assert_eq!(
        verify_against_head(&head, KB, &grown, Some(&key())),
        Ok(()),
        "later versions are expected"
    );

    let mut cut = chains.clone();
    cut.get_mut("a").unwrap().pop();
    let tip_a3 = HeadError::Tip {
        node_id: "a".into(),
        seq: 3,
    };
    assert_eq!(
        verify_against_head(&head, KB, &cut, Some(&key())),
        Err(tip_a3.clone())
    );
    assert!(
        verify_chain(KB, "a", &cut["a"], None, Some(&key())).is_ok(),
        "which the chain alone accepts"
    );

    let mut gone = chains.clone();
    gone.remove("b");
    assert_eq!(
        verify_against_head(&head, KB, &gone, Some(&key())),
        Err(HeadError::Tip {
            node_id: "b".into(),
            seq: 1
        })
    );

    let mut resealed = chains.clone();
    resealed.insert("a".into(), chain("a", &["a1", "rewritten", "a3"]));
    assert!(
        verify_chain(KB, "a", &resealed["a"], None, Some(&key())).is_ok(),
        "consistent on its own"
    );
    assert_eq!(
        verify_against_head(&head, KB, &resealed, Some(&key())),
        Err(tip_a3)
    );

    let mut broken = chains.clone();
    broken.get_mut("b").unwrap()[0].summary = Some("x".into());
    assert!(matches!(
        verify_against_head(&head, KB, &broken, Some(&key())),
        Err(HeadError::Chain { .. })
    ));
}

/// A head survives its file form exactly, and a damaged file is refused
/// rather than read as some other head.
#[test]
fn a_head_round_trips_through_its_file() {
    let head = SignedHead::sign(KB, 100, tips(&kb()), &SEED).unwrap();
    let back = SignedHead::from_json(&head.to_json()).unwrap();
    assert_eq!(back, head);
    assert!(back.verify_signature());
    let mut v = head.to_json();
    v["digest"] = serde_json::json!("00");
    assert!(SignedHead::from_json(&v).is_err());
    let mut v = head.to_json();
    v["format"] = serde_json::json!("other");
    assert!(SignedHead::from_json(&v).is_err());
}
