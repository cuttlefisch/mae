//! The backup envelope: it opens only with its passphrase, only unaltered,
//! only as version 1, and its bytes are exactly the documented format (an
//! independent recomputation, so phase B can import it from the doc alone).
//! Seeds and passphrases are fresh per test; most cases use the floor
//! parameters, which are what an attacker would pick and cost the least.

use super::*;
use crate::member::MemberKey;

const FAST: KdfParams = KdfParams::MIN;

fn seed() -> [u8; 32] {
    *MemberKey::generate().unwrap().seed()
}

fn passphrase() -> String {
    let mut b = [0u8; 12];
    rng::fill(&mut b).unwrap();
    format!("pass-{}", STANDARD.encode(b))
}

#[test]
fn it_round_trips_over_varied_seeds_and_passphrases() {
    let phrases = [
        "x".to_string(),
        passphrase(),
        "correct horse battery staple".to_string(),
        "contraseña ñandú 鍵 🔑".to_string(),
        "\"quoted\\backslash\"\n".to_string(),
        "a".repeat(MAX_PASSPHRASE_BYTES),
    ];
    for p in &phrases {
        let s = seed();
        let env = seal(&s, p, FAST).unwrap();
        assert_eq!(*open(&env, p).unwrap(), s, "passphrase {p:?}");
    }
}

#[test]
fn the_default_parameters_round_trip_and_are_what_is_written() {
    let s = seed();
    let p = passphrase();
    let env = seal(&s, &p, KdfParams::DEFAULT).unwrap();
    assert!(env.starts_with("{\"v\":1,\"kdf\":\"argon2id\",\"m\":65536,\"t\":3,\"p\":4,"));
    assert_eq!(params_of(&env), Ok(KdfParams::DEFAULT));
    assert_eq!(*open(&env, &p).unwrap(), s);
}

/// The bytes, recomputed from the format description with the primitives
/// called directly: the header is a literal, the key is Argon2id over the
/// documented inputs, and the ciphertext is XChaCha20-Poly1305 with the
/// header as associated data. Without the associated data it does not open.
#[test]
fn the_envelope_is_exactly_the_documented_format() {
    let s = seed();
    let p = passphrase();
    let env = seal(&s, &p, FAST).unwrap();
    let v: serde_json::Value = serde_json::from_str(&env).unwrap();
    let field = |k: &str| STANDARD.decode(v[k].as_str().unwrap()).unwrap();
    let (salt, nonce, ct) = (field("salt"), field("nonce"), field("ct"));
    assert_eq!((salt.len(), nonce.len(), ct.len()), (16, 24, 48));
    let header = format!(
        "{{\"v\":1,\"kdf\":\"argon2id\",\"m\":19456,\"t\":2,\"p\":1,\"salt\":\"{}\",\"nonce\":\"{}\"}}",
        STANDARD.encode(&salt),
        STANDARD.encode(&nonce)
    );
    assert_eq!(
        env,
        format!(
            "{},\"ct\":\"{}\"}}",
            header.strip_suffix('}').unwrap(),
            STANDARD.encode(&ct)
        )
    );
    let mut key = [0u8; 32];
    argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(19456, 2, 1, Some(32)).unwrap(),
    )
    .hash_password_into(p.as_bytes(), &salt, &mut key)
    .unwrap();
    let nonce: [u8; 24] = nonce.try_into().unwrap();
    let c = XChaCha20Poly1305::new_from_slice(&key).unwrap();
    let expect = c
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &s,
                aad: header.as_bytes(),
            },
        )
        .unwrap();
    assert_eq!(ct, expect, "the ciphertext is the documented construction");
    assert!(
        c.decrypt(&XNonce::from(nonce), ct.as_slice()).is_err(),
        "the header is bound in as associated data"
    );
}

#[test]
fn fresh_salt_and_nonce_every_seal() {
    let s = seed();
    let p = passphrase();
    let a = seal(&s, &p, FAST).unwrap();
    let b = seal(&s, &p, FAST).unwrap();
    assert_ne!(a, b);
    let va: serde_json::Value = serde_json::from_str(&a).unwrap();
    let vb: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_ne!(va["salt"], vb["salt"]);
    assert_ne!(va["nonce"], vb["nonce"]);
}

#[test]
fn a_wrong_passphrase_is_refused() {
    let s = seed();
    let p = "Correct Horse 7".to_string();
    let env = seal(&s, &p, FAST).unwrap();
    for wrong in [
        "correct Horse 7".to_string(),
        "Correct Horse 7 ".to_string(),
        "Correct Horse".to_string(),
        "Correct Horse 8".to_string(),
        passphrase(),
    ] {
        assert_eq!(open(&env, &wrong), Err(BackupError::Open), "{wrong:?}");
    }
    assert_eq!(*open(&env, &p).unwrap(), s, "control: the right one opens");
}

/// Every byte of the envelope, altered one bit at a time (the bit varies
/// with the position), is refused, and never yields a seed.
#[test]
fn every_altered_byte_is_refused() {
    let s = seed();
    let p = passphrase();
    let env = seal(&s, &p, FAST).unwrap();
    let bytes = env.as_bytes();
    for pos in 0..bytes.len() {
        let mut t = bytes.to_vec();
        t[pos] ^= 1 << (pos % 7);
        let Ok(text) = String::from_utf8(t) else {
            continue; // not a &str: refused before it reaches `open`
        };
        assert!(open(&text, &p).is_err(), "byte {pos} altered, still opened");
    }
}

/// A header that is valid on its own but is not the one sealed: another
/// envelope's salt or nonce, or other in-range parameters.
#[test]
fn a_different_header_is_refused() {
    let p = passphrase();
    let a = seal(&seed(), &p, FAST).unwrap();
    let b = seal(&seed(), &p, FAST).unwrap();
    let va: serde_json::Value = serde_json::from_str(&a).unwrap();
    let vb: serde_json::Value = serde_json::from_str(&b).unwrap();
    let swap = |k: &str| a.replace(va[k].as_str().unwrap(), vb[k].as_str().unwrap());
    for (name, t) in [
        ("salt from b", swap("salt")),
        ("nonce from b", swap("nonce")),
        ("ct from b", swap("ct")),
        ("m + 1", a.replace("\"m\":19456", "\"m\":19457")),
        ("t + 1", a.replace("\"t\":2", "\"t\":3")),
        ("p + 1", a.replace("\"p\":1", "\"p\":2")),
    ] {
        assert_ne!(t, a, "{name}: the case changed nothing");
        assert_eq!(open(&t, &p), Err(BackupError::Open), "{name}");
    }
}

#[test]
fn an_unknown_version_is_refused_as_such() {
    let p = passphrase();
    let env = seal(&seed(), &p, FAST).unwrap();
    for v in [0u64, 2, 10, u64::MAX] {
        let t = env.replacen("\"v\":1", &format!("\"v\":{v}"), 1);
        assert_eq!(open(&t, &p), Err(BackupError::UnknownVersion(v)));
        assert_eq!(params_of(&t), Err(BackupError::UnknownVersion(v)));
    }
}

#[test]
fn an_unknown_kdf_or_out_of_range_parameters_are_refused() {
    let p = passphrase();
    let env = seal(&seed(), &p, FAST).unwrap();
    for kdf in ["argon2i", "argon2d", "scrypt", ""] {
        let t = env.replace("\"argon2id\"", &format!("\"{kdf}\""));
        assert_eq!(open(&t, &p), Err(BackupError::UnknownKdf), "{kdf}");
    }
    for (from, to) in [
        ("\"m\":19456", "\"m\":19455"),
        ("\"m\":19456", "\"m\":262145"),
        ("\"m\":19456", "\"m\":4294967295"),
        ("\"t\":2", "\"t\":1"),
        ("\"t\":2", "\"t\":11"),
        ("\"p\":1", "\"p\":0"),
        ("\"p\":1", "\"p\":5"),
    ] {
        let t = env.replace(from, to);
        assert_eq!(open(&t, &p), Err(BackupError::Params), "{to}");
    }
    // Sealing refuses the same ranges.
    for bad in [
        KdfParams { m: 19455, ..FAST },
        KdfParams { m: 262145, ..FAST },
        KdfParams { t: 1, ..FAST },
        KdfParams { t: 11, ..FAST },
        KdfParams { p: 0, ..FAST },
        KdfParams { p: 5, ..FAST },
    ] {
        assert_eq!(seal(&seed(), &p, bad), Err(BackupError::Params), "{bad:?}");
    }
    // The bounds themselves are in range.
    assert_eq!(KdfParams::MIN.check(), Ok(()));
    assert_eq!(KdfParams::MAX.check(), Ok(()));
    assert_eq!(KdfParams::DEFAULT.check(), Ok(()));
}

#[test]
fn anything_but_the_canonical_text_is_not_an_envelope() {
    let p = passphrase();
    let env = seal(&seed(), &p, FAST).unwrap();
    let v: serde_json::Value = serde_json::from_str(&env).unwrap();
    let ct = v["ct"].as_str().unwrap();
    let salt = v["salt"].as_str().unwrap(); // 16 bytes: the one padded field
    assert!(salt.ends_with("=="));
    let pretty = serde_json::to_string_pretty(&v).unwrap();
    let cases = [
        ("not JSON", "a backup".to_string()),
        ("pretty-printed", pretty),
        ("extra key", env.replacen('{', "{\"x\":1,", 1)),
        ("duplicate key", env.replacen('{', "{\"ct\":\"\",", 1)),
        ("missing ct", env.replace(&format!(",\"ct\":\"{ct}\""), "")),
        ("unpadded", env.replace(salt, salt.trim_end_matches('='))),
        ("short ct", env.replace(ct, &STANDARD.encode([0u8; 47]))),
        ("long ct", env.replace(ct, &STANDARD.encode([0u8; 49]))),
        ("space after colon", env.replacen("\"v\":1", "\"v\": 1", 1)),
        ("a JSON array", "[1]".to_string()),
    ];
    for (name, t) in cases {
        assert!(
            matches!(open(&t, &p), Err(BackupError::NotAnEnvelope(_))),
            "{name}: {:?}",
            open(&t, &p)
        );
    }
    assert_eq!(
        open(&" ".repeat(MAX_ENVELOPE_BYTES + 1), &p),
        Err(BackupError::TooLong)
    );
    // Surrounding whitespace (a file's trailing newline) is not an alteration.
    assert!(open(&format!("{env}\n"), &p).is_ok());
}

#[test]
fn passphrase_length_is_bounded_both_ways() {
    let s = seed();
    let long = "a".repeat(MAX_PASSPHRASE_BYTES + 1);
    assert_eq!(seal(&s, "", FAST), Err(BackupError::Passphrase));
    assert_eq!(seal(&s, &long, FAST), Err(BackupError::Passphrase));
    let env = seal(&s, "ok", FAST).unwrap();
    assert_eq!(open(&env, ""), Err(BackupError::Passphrase));
    assert_eq!(open(&env, &long), Err(BackupError::Passphrase));
}

#[test]
fn the_envelope_holds_no_plain_seed() {
    let s = seed();
    let env = seal(&s, &passphrase(), FAST).unwrap();
    for form in [
        STANDARD.encode(s),
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(s),
        s.iter().map(|b| format!("{b:02x}")).collect::<String>(),
    ] {
        assert!(!env.contains(&form));
    }
}

#[test]
fn error_codes_are_distinct() {
    let all = [
        BackupError::TooLong,
        BackupError::NotAnEnvelope(String::new()),
        BackupError::UnknownVersion(2),
        BackupError::UnknownKdf,
        BackupError::Params,
        BackupError::Passphrase,
        BackupError::Open,
        BackupError::RandomUnavailable,
    ];
    let codes: std::collections::BTreeSet<&str> = all.iter().map(|e| e.code()).collect();
    assert_eq!(codes.len(), all.len());
}
