//! ADR-116 P1, the chain: any changed byte of any entry fails verification; a
//! reordered or removed entry fails; pruned content still verifies with its
//! metadata intact; different entries never share a hash; stored content is
//! accepted only in its one canonical encoding.

use std::collections::HashMap;

use proptest::prelude::*;

use super::*;
use crate::kb::history::*;

/// The test KB's history key.
fn key() -> HistoryKey {
    HistoryKey::from_bytes([5; 32])
}

fn seal_k(
    kb: &str,
    node: &str,
    prev: Option<&HistoryEntry>,
    d: Draft,
) -> Result<HistoryEntry, SealError> {
    seal(kb, node, prev, d, &key())
}

fn verify_chain_k(
    kb: &str,
    node: &str,
    entries: &[HistoryEntry],
    head: Option<(u64, Hash)>,
) -> Result<(), VerifyError> {
    verify_chain(kb, node, entries, head, Some(&key()))
}

const KB: &str = "kb-1";
const NODE: &str = "n-1";

fn alice() -> Vec<Editor> {
    vec![Editor {
        kind: EditorKind::Principal,
        principal: "SHA256:alice".into(),
    }]
}

/// A draft of `kind` holding `node`, made valid for position `seq`.
fn draft(kind: VersionKind, seq: u64, node: MaterializedNode) -> Draft {
    let kind = if kind == VersionKind::Rollback && seq == 1 {
        VersionKind::Edit
    } else {
        kind
    };
    Draft {
        kind,
        created_at: 1_700_000_000 + seq as i64,
        editors: alice(),
        wal_from: seq * 10,
        wal_to: seq * 10 + 3,
        summary: seq.is_multiple_of(2).then(|| format!("change {seq}")),
        name: (kind == VersionKind::Named).then(|| format!("v{seq}")),
        restored_from: (kind == VersionKind::Rollback).then_some(1),
        content: Some(node),
    }
}

fn chain_of(versions: Vec<(VersionKind, MaterializedNode)>) -> Vec<HistoryEntry> {
    let mut out: Vec<HistoryEntry> = Vec::new();
    for (i, (kind, node)) in versions.into_iter().enumerate() {
        let e = seal_k(KB, NODE, out.last(), draft(kind, i as u64 + 1, node)).unwrap();
        out.push(e);
    }
    out
}

fn arb_text() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(&["a", "b", "ñ", "😀", " ", "\n", ""][..]),
        0..6,
    )
    .prop_map(|v| v.concat())
}

fn arb_node() -> impl Strategy<Value = MaterializedNode> {
    let lists = (
        prop::collection::vec(arb_text(), 0..3),
        prop::collection::vec(arb_text(), 0..3),
        prop::collection::vec(arb_text(), 0..3),
    );
    let opts = (
        prop::option::of(arb_text()),
        prop::option::of(arb_text()),
        prop::option::of(any::<u32>()),
        prop::option::of(any::<i64>()),
        prop::option::of((
            prop::collection::vec(arb_text(), 0..3),
            any::<bool>(),
            any::<bool>(),
        )),
    );
    let props = prop::collection::hash_map(arb_text(), arb_text(), 0..3);
    (arb_text(), arb_text(), lists, opts, props).prop_map(
        |(title, body, (tags, links, aliases), (kind, source, sv, created, header), properties)| {
            MaterializedNode {
                id: NODE.into(),
                title,
                body,
                tags,
                links,
                kind: kind.clone(),
                todo_state: kind.map(|_| "TODO".into()),
                priority: None,
                aliases,
                properties,
                source_version: sv,
                source,
                created_at: created,
                org_header: header.map(|(property_order, t, f)| OrgHeader {
                    property_order,
                    has_title_line: t,
                    has_filetags_line: f,
                }),
            }
        },
    )
}

fn arb_chain() -> impl Strategy<Value = Vec<HistoryEntry>> {
    let kinds = prop::sample::select(&VersionKind::ALL[..7]);
    prop::collection::vec((kinds, arb_node()), 1..6).prop_map(chain_of)
}

/// The pin computed from the code before `digest` existed: moving the digest
/// onto `MaterializedNode` changed no `version()` (stored fingerprints use it).
#[test]
fn version_is_unchanged_and_is_the_digest_prefix() {
    let mut n = KbNodeDoc::new_created_at(
        "n-1",
        "Título",
        "cuerpo\nñ\n",
        &["a".into(), "b".into()],
        1_700_000_000,
    );
    let _ = n.set_kind(Some("Concept"));
    let _ = n.set_todo_state(Some("TODO"));
    let _ = n.set_aliases(&["alias".into()]);
    let props = HashMap::from([
        ("ID".to_string(), "n-1".to_string()),
        ("ZED".into(), "z".into()),
    ]);
    let _ = n.set_properties(&props);
    let _ = n.set_org_header(&OrgHeader {
        property_order: vec!["ID".into(), "ZED".into()],
        has_title_line: true,
        has_filetags_line: false,
    });
    assert_eq!(n.version(), "050fd9873625605bcfe39af80acaae1c");
    assert_eq!(hex::encode(&n.materialize().digest()[..16]), n.version());
}

/// Changes field `field` of `e` (a different value, always), using `salt` to
/// pick where.
fn mutate(e: &mut HistoryEntry, field: usize, salt: usize) {
    fn flip(s: &mut String, salt: usize) {
        let mut b = std::mem::take(s).into_bytes();
        match b
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_ascii())
            .map(|(i, _)| i)
            .nth(salt % b.len().max(1))
        {
            Some(i) => b[i] ^= 1,
            None => b.push(b'x'),
        }
        *s = String::from_utf8(b).unwrap();
    }
    fn bit(b: &mut [u8], salt: usize) {
        let i = salt % b.len();
        b[i] ^= 1 << (salt % 8);
    }
    match field {
        0 => flip(&mut e.kb_id, salt),
        1 => flip(&mut e.node_id, salt),
        2 => e.seq += 1 + salt as u64 % 3,
        3 => {
            e.kind = VersionKind::ALL
                [(VersionKind::ALL.iter().position(|k| *k == e.kind).unwrap() + 1 + salt % 7) % 8]
        }
        4 => e.created_at ^= 1 << (salt % 63),
        5 => flip(&mut e.editors[0].principal, salt),
        6 => {
            e.editors[0].kind = if e.editors[0].kind == EditorKind::Internal {
                EditorKind::Principal
            } else {
                EditorKind::Internal
            }
        }
        7 => e.wal_from ^= 1 << (salt % 63),
        8 => e.wal_to ^= 1 << (salt % 63),
        9 => match &mut e.summary {
            Some(s) => flip(s, salt),
            None => e.summary = Some(String::new()),
        },
        10 => match &mut e.name {
            Some(s) => flip(s, salt),
            None => e.name = Some("n".into()),
        },
        11 => e.restored_from = Some(e.restored_from.map_or(1, |s| s ^ 1)),
        12 => bit(e.content.as_mut().unwrap(), salt),
        13 => bit(&mut e.content_hash, salt),
        14 => bit(&mut e.prev_hash, salt),
        _ => bit(&mut e.entry_hash, salt),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// The content encoding round-trips, and is the one encoding of its node.
    #[test]
    fn content_round_trips(node in arb_node()) {
        let bytes = encode_content(&node);
        let back = decode_content(&bytes).unwrap();
        prop_assert_eq!(&back, &node);
        prop_assert_eq!(encode_content(&back), bytes);
    }

    /// Any changed field of any entry fails verification, at that entry. Kills
    /// a hash that leaves a field out, a skipped link or content check.
    #[test]
    fn any_change_to_any_entry_fails(chain in arb_chain(), pick in any::<prop::sample::Index>(),
                                      field in 0usize..16, salt in any::<usize>()) {
        prop_assert!(verify_chain_k(KB, NODE, &chain, None).is_ok());
        let i = pick.index(chain.len());
        let mut bad = chain.clone();
        mutate(&mut bad[i], field, salt);
        prop_assert_ne!(&bad[i], &chain[i]);
        let err = verify_chain_k(KB, NODE, &bad, None).unwrap_err();
        prop_assert!(err.seq == i as u64 + 1 || err.broken == Broken::Seq, "{:?} at {}", err, i);
    }

    /// A reordered entry fails; a removed entry fails, the last one only
    /// against the head (the chain alone cannot see it, which is why the ADR
    /// has heads).
    #[test]
    fn a_reordered_or_removed_entry_fails(chain in arb_chain(), a in any::<prop::sample::Index>(),
                                          b in any::<prop::sample::Index>()) {
        let last = chain.last().unwrap();
        let head = Some((last.seq, last.entry_hash));
        prop_assert!(verify_chain_k(KB, NODE, &chain, head).is_ok());
        let (i, j) = (a.index(chain.len()), b.index(chain.len()));
        if i != j {
            let mut swapped = chain.clone();
            swapped.swap(i, j);
            prop_assert!(verify_chain_k(KB, NODE, &swapped, None).is_err());
        }
        let mut removed = chain.clone();
        removed.remove(i);
        if i + 1 == chain.len() {
            prop_assert!(verify_chain_k(KB, NODE, &removed, None).is_ok());
            prop_assert_eq!(verify_chain_k(KB, NODE, &removed, head).unwrap_err().broken, Broken::Head);
        } else {
            prop_assert!(verify_chain_k(KB, NODE, &removed, head).is_err());
        }
    }

    /// Pruning drops only the content: the chain still verifies against its
    /// head, and every other field of a pruned entry is unchanged.
    #[test]
    fn pruned_content_still_verifies(chain in arb_chain(), mask in any::<u8>()) {
        let mut pruned = chain.clone();
        for (i, e) in pruned.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                e.prune();
            }
        }
        let last = chain.last().unwrap();
        prop_assert!(verify_chain_k(KB, NODE, &pruned, Some((last.seq, last.entry_hash))).is_ok());
        for (p, e) in pruned.iter().zip(&chain) {
            prop_assert_eq!(HistoryEntry { content: e.content.clone(), ..p.clone() }, e.clone());
        }
    }

    /// Different entries never share a hash, even built from fields that only
    /// differ in where one ends and the next begins.
    #[test]
    fn different_entries_have_different_hashes(
        a in (0usize..4, 0usize..4, 0usize..4, 0usize..4),
        b in (0usize..4, 0usize..4, 0usize..4, 0usize..4),
    ) {
        const PARTS: [&str; 4] = ["", "a", "ab", "b"];
        let make = |(kb, summary, p1, p2): (usize, usize, usize, usize)| {
            let mut d = draft(VersionKind::Edit, 1, MaterializedNode { id: NODE.into(), ..node_with("x") });
            d.summary = Some(PARTS[summary].into());
            d.editors = vec![Editor { kind: EditorKind::Principal, principal: PARTS[p1].into() },
                             Editor { kind: EditorKind::Internal, principal: PARTS[p2].into() }];
            seal_k(&format!("kb{}", PARTS[kb]), NODE, None, d).unwrap()
        };
        let (ea, eb) = (make(a), make(b));
        prop_assert_eq!(a == b, ea.entry_hash == eb.entry_hash);
    }
}

fn node_with(title: &str) -> MaterializedNode {
    KbNodeDoc::new(NODE, title, "", &[]).materialize()
}

/// The boundary cases a concatenation without length prefixes would merge.
#[test]
fn shifted_boundaries_do_not_collide() {
    let tags = |t: &[&str]| MaterializedNode {
        tags: t.iter().map(|s| s.to_string()).collect(),
        ..node_with("x")
    };
    assert_ne!(
        encode_content(&tags(&["ab"])),
        encode_content(&tags(&["a", "b"]))
    );
    assert_ne!(tags(&["ab"]).digest(), tags(&["a", "b"]).digest());
    let with = |summary: Option<&str>| {
        let mut d = draft(VersionKind::Edit, 1, node_with("x"));
        d.summary = summary.map(String::from);
        seal_k(KB, NODE, None, d).unwrap().entry_hash
    };
    assert_ne!(with(None), with(Some("")));
    let id_split = |kb: &str, node: &str| {
        let d = draft(
            VersionKind::Edit,
            1,
            MaterializedNode {
                id: node.into(),
                ..node_with("x")
            },
        );
        seal_k(kb, node, None, d).unwrap().entry_hash
    };
    assert_ne!(id_split("ab", "c"), id_split("a", "bc"));
}

/// Stored content is accepted only as its one encoding: a second spelling of
/// the same node (properties reordered, a flag of 2, a trailing byte) is
/// refused, and a chain holding one fails at that entry.
#[test]
fn a_non_canonical_content_is_refused() {
    let node = MaterializedNode {
        properties: HashMap::from([("a".to_string(), "1".to_string()), ("b".into(), "2".into())]),
        ..node_with("x")
    };
    let good = encode_content(&node);
    let pair = |k: &str, v: &str| {
        [
            &1u64.to_le_bytes()[..],
            k.as_bytes(),
            &1u64.to_le_bytes(),
            v.as_bytes(),
        ]
        .concat()
    };
    let (pa, pb) = (pair("a", "1"), pair("b", "2"));
    let at = good
        .windows(pa.len())
        .position(|w| w == pa.as_slice())
        .unwrap();
    let mut swapped = good.clone();
    swapped[at..at + pa.len() + pb.len()].copy_from_slice(&[pb.clone(), pa.clone()].concat());
    assert_eq!(
        decode_content(&swapped),
        Err(ContentError::Malformed("property order"))
    );
    let mut trailing = good.clone();
    trailing.push(0);
    assert_eq!(decode_content(&trailing), Err(ContentError::Trailing));
    let mut flag = good.clone();
    let last = flag.len() - 1; // the org_header presence flag
    flag[last] = 2;
    assert_eq!(
        decode_content(&flag),
        Err(ContentError::Malformed("flag value"))
    );
    assert_eq!(
        decode_content(&good[..good.len() - 3]),
        Err(ContentError::Truncated)
    );
    let mut huge = good[..good.len()].to_vec();
    let tags_count_at = 8 + 10 + 3 * 8 + NODE.len() + 1; // magic, id, title "x", body ""
    huge[tags_count_at + 8..tags_count_at + 16].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(decode_content(&huge), Err(ContentError::Truncated));

    let mut chain = chain_of(vec![
        (VersionKind::Edit, node.clone()),
        (VersionKind::Edit, node),
    ]);
    chain[1].content = Some(swapped);
    let err = verify_chain_k(KB, NODE, &chain, None).unwrap_err();
    assert_eq!((err.seq, err.broken), (2, Broken::Content));
}

/// `seal` refuses every combination `verify_chain` would reject, so a writer
/// cannot store an entry its own verifier fails.
#[test]
fn seal_refuses_what_verification_would_reject() {
    let base = || draft(VersionKind::Edit, 2, node_with("x"));
    let first = seal_k(KB, NODE, None, base()).unwrap();
    let cases: Vec<(&str, Draft)> = vec![
        (
            "other node's content",
            Draft {
                content: Some(KbNodeDoc::new("other", "x", "", &[]).materialize()),
                ..base()
            },
        ),
        (
            "audit with content",
            Draft {
                kind: VersionKind::Pruned,
                ..base()
            },
        ),
        (
            "version without content",
            Draft {
                content: None,
                ..base()
            },
        ),
        (
            "named without name",
            Draft {
                kind: VersionKind::Named,
                ..base()
            },
        ),
        (
            "name on an edit",
            Draft {
                name: Some("n".into()),
                ..base()
            },
        ),
        (
            "rollback without target",
            Draft {
                kind: VersionKind::Rollback,
                ..base()
            },
        ),
        (
            "rollback to itself",
            Draft {
                kind: VersionKind::Rollback,
                restored_from: Some(2),
                ..base()
            },
        ),
        (
            "reversed WAL",
            Draft {
                wal_from: 9,
                wal_to: 1,
                ..base()
            },
        ),
    ];
    for (what, d) in cases {
        assert!(seal_k(KB, NODE, Some(&first), d).is_err(), "{what}");
    }
    assert!(
        seal_k("kb-2", NODE, Some(&first), base()).is_err(),
        "previous from another KB"
    );
    let audit = Draft {
        kind: VersionKind::Pruned,
        content: None,
        summary: Some("policy".into()),
        ..base()
    };
    let a = seal_k(KB, NODE, Some(&first), audit).unwrap();
    assert!(verify_chain_k(KB, NODE, &[first, a], None).is_ok());
}

/// Editors arrive in whatever order the editor record yields; `seal` stores
/// them sorted and once each, so the same set always hashes alike.
#[test]
fn seal_sorts_and_dedups_editors() {
    let ed = |kind, p: &str| Editor {
        kind,
        principal: p.into(),
    };
    let with = |editors: Vec<Editor>| {
        let d = Draft {
            editors,
            ..draft(VersionKind::Edit, 1, node_with("x"))
        };
        seal_k(KB, NODE, None, d).unwrap()
    };
    let a = with(vec![
        ed(EditorKind::Internal, "job"),
        ed(EditorKind::Principal, "bob"),
        ed(EditorKind::Principal, "alice"),
        ed(EditorKind::Principal, "bob"),
    ]);
    let b = with(vec![
        ed(EditorKind::Principal, "alice"),
        ed(EditorKind::Principal, "bob"),
        ed(EditorKind::Internal, "job"),
    ]);
    assert_eq!(a.editors, b.editors);
    assert_eq!(a.entry_hash, b.entry_hash);
    assert_eq!(a.editors.len(), 3);
    assert!(verify_chain_k(KB, NODE, &[a], None).is_ok());
}

/// The head is one digest over every node's tip, independent of the order the
/// nodes are given in and sensitive to each part of each tip.
#[test]
fn the_kb_head_covers_every_tip_in_node_order() {
    let h = |n: &[(&str, u64, Hash)]| kb_head(KB, n.iter().copied()).unwrap();
    let (x, y) = ([1u8; 32], [2u8; 32]);
    let base = h(&[("a", 1, x), ("b", 2, y)]);
    assert_eq!(base, h(&[("b", 2, y), ("a", 1, x)]));
    for other in [
        h(&[("a", 1, x), ("b", 3, y)]),
        h(&[("a", 1, y), ("b", 2, y)]),
        h(&[("a", 1, x), ("c", 2, y)]),
        h(&[("a", 1, x)]),
        kb_head("kb-2", [("a", 1, x), ("b", 2, y)]).unwrap(),
    ] {
        assert_ne!(other, base);
    }
    assert!(kb_head(KB, [("a", 1, x), ("a", 2, y)]).is_err());
}

fn four(prefix: &str) -> Vec<HistoryEntry> {
    chain_of(
        (0..4)
            .map(|i| (VersionKind::Edit, node_with(&format!("{prefix}{i}"))))
            .collect(),
    )
}

fn at(err: VerifyError) -> (u64, Broken) {
    (err.seq, err.broken)
}

/// The forger D4 names, who holds the store and the KB's history key and can
/// recompute any hash: each forgery of entry 2 is caught at entry 2 by the
/// check made for it, or, where entry 2 is consistent by itself, at the link
/// to entry 3. A forger WITHOUT the key cannot make a content verify at all:
/// the keyed hash catches it at entry 2. Kills a verifier missing the id,
/// content-id or link check, a hash leaving out `content_hash`, and an
/// unkeyed content hash.
#[test]
fn a_forger_who_recomputes_hashes_is_caught_where_they_forged() {
    let chain = four("t");
    let forge = |f: &dyn Fn(&mut HistoryEntry), rehash: bool| {
        let mut c = chain.clone();
        f(&mut c[1]);
        if rehash {
            c[1].entry_hash = c[1].compute_hash();
        }
        at(verify_chain_k(KB, NODE, &c, None).unwrap_err())
    };
    let other = KbNodeDoc::new("other", "t", "", &[]).materialize();
    let alt = node_with("forged");
    // With the key: the right hash. Without: the best a forger can do.
    let to = |n: &MaterializedNode, keyed: bool| {
        let hash = if keyed {
            key().content_hash(n)
        } else {
            n.digest()
        };
        let bytes = encode_content(n);
        move |e: &mut HistoryEntry| {
            e.content = Some(bytes.clone());
            e.content_hash = hash;
        }
    };
    assert_eq!(forge(&|e| e.kb_id = "kb-2".into(), true), (2, Broken::Ids));
    assert_eq!(forge(&to(&other, true), true), (2, Broken::Content));
    assert_eq!(forge(&to(&alt, true), false), (2, Broken::EntryHash));
    assert_eq!(forge(&to(&alt, true), true), (3, Broken::Link));
    assert_eq!(
        forge(&to(&alt, false), true),
        (2, Broken::ContentHash),
        "no key, no valid hash"
    );
    let pruned_swap = |e: &mut HistoryEntry| {
        e.prune();
        e.content_hash = key().content_hash(&alt);
    };
    assert_eq!(forge(&pruned_swap, true), (3, Broken::Link));
}

/// HMAC-SHA256 matches RFC 4231 (test cases 1, 2 and 6: a short key, a key
/// shorter than the hash, and a key longer than the block).
#[test]
fn hmac_matches_rfc_4231() {
    let h = |k: &[u8], m: &[u8]| hex::encode(hmac_sha256(k, m));
    assert_eq!(
        h(&[0x0b; 20], b"Hi There"),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert_eq!(
        h(b"Jefe", b"what do ya want for nothing?"),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    assert_eq!(
        h(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        ),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

/// The keyed hash confirms no guess without the key: the right content under
/// another key, or unkeyed, does not reproduce it; the same key always does.
/// Without the key a verifier checks the chain and leaves the content alone.
#[test]
fn the_content_hash_is_keyed() {
    let n = node_with("secret 4711");
    let stored = key().content_hash(&n);
    assert_eq!(stored, key().content_hash(&n));
    assert_ne!(stored, HistoryKey::from_bytes([6; 32]).content_hash(&n));
    assert_ne!(stored, n.digest());
    let mut c = four("t");
    c[1].content = Some(encode_content(&node_with("swapped")));
    assert!(
        verify_chain_k(KB, NODE, &c, None).is_err(),
        "with the key: caught"
    );
    assert!(
        verify_chain(KB, NODE, &c, None, None).is_ok(),
        "without the key, content is not checked (the chain still is)"
    );
    c[2].summary = Some("x".into());
    assert!(verify_chain(KB, NODE, &c, None, None).is_err());
}

/// An entry valid by itself, taken from another chain of the same node and
/// position, fails where it is spliced in; re-pointing it and every later link
/// without recomputing their hashes fails too, even against an unchanged head.
/// Kills a verifier without the link check, and a hash leaving out
/// `prev_hash` (which would let the links be re-pointed freely).
#[test]
fn an_entry_spliced_from_another_chain_fails() {
    let (a, b) = (four("t"), four("u"));
    let head = Some((a[3].seq, a[3].entry_hash));
    let mut spliced = a.clone();
    spliced[1] = b[1].clone();
    assert_eq!(
        at(verify_chain_k(KB, NODE, &spliced, None).unwrap_err()),
        (2, Broken::Link)
    );
    spliced[1].prev_hash = a[0].entry_hash;
    spliced[2].prev_hash = spliced[1].entry_hash;
    assert_eq!(
        at(verify_chain_k(KB, NODE, &spliced, head).unwrap_err()),
        (2, Broken::EntryHash)
    );
}
