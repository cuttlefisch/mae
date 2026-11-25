//! The chain format against the committed fixture (ADR-113 E12: a real chain
//! signed with a fixed TEST key), and the verifier against every one-byte
//! change. Kills a canonicalizer that leaves keys unsorted or escapes
//! differently, an entry hash without its domain, a verifier that trusts the
//! export's own key, and one that skips any step of E10.

use super::*;

const FIXTURE: &str = include_str!("../../../docs/verification/fixtures/review-api/events.json");

/// The fixture's test key: seed bytes 0x01..0x20.
fn fixture_identity() -> Identity {
    let seed: [u8; 32] = std::array::from_fn(|i| i as u8 + 1);
    Identity::from_seed(&seed, "fixture")
}

fn fixture_lines() -> Vec<Value> {
    let v: Value = serde_json::from_str(FIXTURE).unwrap();
    v["export_jsonl"].as_array().unwrap().clone()
}

fn as_text(lines: &[Value]) -> String {
    lines
        .iter()
        .map(|l| serde_json::to_string(l).unwrap() + "\n")
        .collect()
}

fn key_of(id: &Identity) -> [u8; 32] {
    id.public().to_bytes()
}

#[test]
fn the_fixture_chain_verifies_under_its_test_key() {
    let id = fixture_identity();
    let header_key = fixture_lines()[0]["daemon_key"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        parse_key(&header_key),
        Some(key_of(&id)),
        "the fixture's key"
    );
    let v = verify_export(&as_text(&fixture_lines()), &key_of(&id), None).unwrap();
    assert_eq!(v.events, 4);
}

/// Our canonicalizer writes exactly the fixture's canonical bytes for the
/// objects they encode, so an event we seal exports to the same bytes a
/// third party's JCS would (E10: "A producer must use JCS").
#[test]
fn the_canonicalizer_reproduces_the_fixture_bytes() {
    for line in fixture_lines() {
        let Some(c) = line.get("canonical").and_then(Value::as_str) else {
            continue;
        };
        let bytes = b64_decode(c).unwrap();
        let parsed: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(canonical(&parsed).unwrap(), bytes);
    }
}

/// RFC 8785's own rules on the cases a fixture does not show: key order,
/// escapes, non-ASCII text, integers at the limit, and the values E10 refuses.
#[test]
fn canonical_bytes_follow_rfc_8785_on_the_restricted_set() {
    let v = json!({"b": [true, null, -9007199254740991_i64], "a": "é\u{1}\"\\\n/\u{7f}", "A": {}});
    assert_eq!(
        String::from_utf8(canonical(&v).unwrap()).unwrap(),
        "{\"A\":{},\"a\":\"é\\u0001\\\"\\\\\\n/\u{7f}\",\"b\":[true,null,-9007199254740991]}"
    );
    assert_eq!(canonical(&json!(1.5)), Err(CanonError::Float));
    assert_eq!(
        canonical(&json!(9007199254740992_i64)),
        Err(CanonError::IntegerOutOfRange)
    );
    assert_eq!(
        canonical(&json!(u64::MAX)),
        Err(CanonError::IntegerOutOfRange)
    );
    assert!(matches!(
        canonical(&json!({"ключ": 1})),
        Err(CanonError::NonAsciiKey(_))
    ));
}

/// E13: the fixture chain fails after any byte of any line is changed.
#[test]
#[ignore = "exhaustive, about two minutes in debug: run with --ignored"]
fn every_one_byte_change_to_the_fixture_fails() {
    // Exhaustive: every byte of every line. About 5,400 full verifications,
    // two minutes in a debug build, so it runs on demand (`--ignored`), not
    // in the merge gate; the two tests below cover every field of every line
    // and a seeded sample of bytes there.
    let lines = fixture_text_lines();
    for (li, line) in lines.iter().enumerate() {
        for bi in 0..line.len() {
            assert_flip_fails(&lines, li, bi);
        }
    }
}

/// One byte changed inside each field's value, of each line, fails: the
/// header's key and KB, and each event's seq, prev_hash, canonical bytes,
/// entry_hash and signature.
#[test]
fn a_change_in_any_field_of_any_line_fails() {
    let lines = fixture_text_lines();
    let mut checked = 0;
    for (li, line) in lines.iter().enumerate() {
        let v: Value = serde_json::from_str(line).unwrap();
        for (key, value) in v.as_object().unwrap() {
            let needle = format!("\"{key}\":{}", serde_json::to_string(value).unwrap());
            let at = line.find(&needle).expect("the field as serialized");
            // A byte inside the value, past its opening quote when it has one.
            let bi = at + needle.len() - 2;
            assert_flip_fails(&lines, li, bi);
            checked += 1;
        }
    }
    assert!(
        checked >= 6 * 4,
        "every field of every line was checked: {checked}"
    );
}

/// A seeded sample of 300 byte positions across the whole fixture fails.
#[test]
fn a_sample_of_one_byte_changes_fails() {
    let lines = fixture_text_lines();
    let total: usize = lines.iter().map(String::len).sum();
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..300 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let mut pos = (x % total as u64) as usize;
        let li = lines
            .iter()
            .position(|l| {
                if pos < l.len() {
                    true
                } else {
                    pos -= l.len();
                    false
                }
            })
            .unwrap();
        assert_flip_fails(&lines, li, pos);
    }
}

fn fixture_text_lines() -> Vec<String> {
    fixture_lines()
        .iter()
        .map(|l| serde_json::to_string(l).unwrap())
        .collect()
}

/// Flips the low bit of byte `bi` of line `li` and asserts the export no
/// longer verifies (a flip that is not UTF-8 is skipped: it is not text).
fn assert_flip_fails(lines: &[String], li: usize, bi: usize) {
    let mut bytes = lines[li].as_bytes().to_vec();
    bytes[bi] ^= 0x01;
    let Ok(changed) = String::from_utf8(bytes) else {
        return;
    };
    let mut all = lines.to_vec();
    all[li] = changed;
    let key = key_of(&fixture_identity());
    assert!(
        verify_export(&all.join("\n"), &key, None).is_err(),
        "line {li} byte {bi} changed and it still verified"
    );
}

fn sealed_chain(id: &Identity, kb: &str, n: u64) -> Vec<Value> {
    let mut lines = vec![header_line(kb, &key_of(id))];
    let mut prev = ZERO_HASH.to_string();
    for seq in 1..=n {
        let body = EventBody {
            kb_id: kb.into(),
            seq,
            prev_hash: prev.clone(),
            kind: "policy_changed".into(),
            at: "2026-11-25T00:00:00.000Z".into(),
            actor: json!({"principal": "p", "kind": "person", "iss": null, "sub": null, "sign_in": null}),
            payload: json!({"n": seq}),
            member_sig: None,
        };
        let s = seal(&body, id).unwrap();
        lines.push(event_line(seq, &prev, &s));
        prev = hex::encode(s.entry_hash);
    }
    lines.push(head_line(kb, n, &prev, "2026-11-25T00:00:01.000Z", id).unwrap());
    lines
}

/// A chain we seal verifies; under another key (also a forged one carrying
/// its own key in the header), a head that is not the last event's, a
/// removed event, swapped events, and a kept head the export does not
/// extend, it does not. Real generated keys, never a fixed one.
#[test]
fn a_sealed_chain_verifies_and_its_forgeries_do_not() {
    let id = Identity::generate("daemon");
    let other = Identity::generate("forger");
    let lines = sealed_chain(&id, "kb", 4);
    let ok = verify_export(&as_text(&lines), &key_of(&id), None).unwrap();
    assert_eq!(ok.events, 4);

    assert!(verify_export(&as_text(&lines), &key_of(&other), None).is_err());
    let forged = sealed_chain(&other, "kb", 4);
    assert!(verify_export(&as_text(&forged), &key_of(&id), None).is_err());

    let mut removed = lines.clone();
    removed.remove(2);
    assert!(verify_export(&as_text(&removed), &key_of(&id), None).is_err());

    let mut swapped = lines.clone();
    swapped.swap(2, 3);
    assert!(verify_export(&as_text(&swapped), &key_of(&id), None).is_err());

    // A truncated chain with a head re-signed over it: verifies alone, and
    // fails against a head kept from the longer chain.
    let short = sealed_chain(&id, "kb", 3);
    assert!(verify_export(&as_text(&short), &key_of(&id), None).is_ok());
    let kept_long = lines.last().unwrap().clone();
    assert!(verify_export(&as_text(&short), &key_of(&id), Some(&kept_long)).is_err());
    let kept_short = short.last().unwrap().clone();
    let extended = verify_export(&as_text(&lines), &key_of(&id), Some(&kept_short)).unwrap();
    assert_eq!(extended.against_seq, Some(3));
    let kept_by_other = sealed_chain(&other, "kb", 3).last().unwrap().clone();
    assert!(verify_export(&as_text(&lines), &key_of(&id), Some(&kept_by_other)).is_err());
}

/// The entry hash carries its domain, and the signature is over the raw hash
/// with its own domain: a signature over the canonical bytes, or over the hex
/// text, does not verify.
#[test]
fn the_hash_and_signature_domains_are_e10s() {
    let id = Identity::generate("daemon");
    let canonical_bytes = b"{\"a\":1}";
    let mut h = Sha256::new();
    h.update(b"maereview/v1\n");
    h.update(canonical_bytes);
    let expected: [u8; 32] = h.finalize().into();
    assert_eq!(entry_hash(canonical_bytes), expected);
    let lines = sealed_chain(&id, "kb", 1);
    let sig = b64_decode(lines[1]["sig"].as_str().unwrap()).unwrap();
    let raw: [u8; 32] = hex::decode(lines[1]["entry_hash"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let pk = id.public();
    assert!(pk.verify(&[b"maereview-sig/v1\n".as_slice(), &raw].concat(), &sig));
    assert!(!pk.verify(&raw, &sig));
}
