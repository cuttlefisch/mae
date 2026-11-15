//! The backup envelope: it opens only with its passphrase, only unaltered,
//! only as version 1, only as the key its header names, and its bytes are
//! exactly the documented format (an independent construction, so phase B
//! can import it from the doc alone). Seeds and passphrases are fresh per
//! test; most cases use the floor parameters, which are what an attacker
//! would pick and cost the least.

use super::*;
use crate::member::MemberKey;

const FAST: KdfParams = KdfParams::MIN;

fn seed() -> [u8; 32] {
    *MemberKey::generate().unwrap().seed()
}

fn fp_of(seed: &[u8; 32]) -> String {
    MemberKey::from_seed(seed).fingerprint()
}

fn passphrase() -> String {
    let mut b = [0u8; 12];
    rng::fill(&mut b).unwrap();
    format!("pass-{}", STANDARD.encode(b))
}

fn field(env: &str, k: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(env).unwrap();
    v[k].as_str().unwrap().to_string()
}

/// An envelope built from the format description alone, with the
/// primitives called directly: the header is a literal, the key is Argon2id
/// over exactly `password` (no normalisation, no floor), and the ciphertext
/// is XChaCha20-Poly1305 with the header as associated data.
fn by_hand(
    seed: &[u8; 32],
    fp: &str,
    password: &[u8],
    salt: &[u8; 16],
    nonce: &[u8; 24],
) -> String {
    let header = format!(
        "{{\"v\":1,\"fingerprint\":\"{fp}\",\"kdf\":\"argon2id\",\"m\":19456,\"t\":2,\"p\":1,\
         \"salt\":\"{}\",\"nonce\":\"{}\"}}",
        STANDARD.encode(salt),
        STANDARD.encode(nonce)
    );
    let mut key = [0u8; 32];
    argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(19456, 2, 1, Some(32)).unwrap(),
    )
    .hash_password_into(password, salt, &mut key)
    .unwrap();
    let ct = XChaCha20Poly1305::new_from_slice(&key)
        .unwrap()
        .encrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: seed,
                aad: header.as_bytes(),
            },
        )
        .unwrap();
    format!(
        "{},\"ct\":\"{}\"}}",
        header.strip_suffix('}').unwrap(),
        STANDARD.encode(ct)
    )
}

#[test]
fn it_round_trips_over_varied_seeds_and_passphrases() {
    let phrases = [
        passphrase(),
        "correct horse battery staple".to_string(),
        "contraseña ñandú 鍵 🔑 ok".to_string(),
        "\"quoted\\backslash\"\n12".to_string(),
        "a".repeat(MAX_PASSPHRASE_BYTES),
    ];
    for p in &phrases {
        let s = seed();
        let env = seal(&s, p, FAST).unwrap();
        assert_eq!(*open(&env, p).unwrap(), s, "passphrase {p:?}");
    }
}

#[test]
fn the_default_parameters_and_the_fingerprint_are_what_is_written() {
    let s = seed();
    let p = passphrase();
    let env = seal(&s, &p, KdfParams::DEFAULT).unwrap();
    let want = format!(
        "{{\"v\":1,\"fingerprint\":\"{}\",\"kdf\":\"argon2id\",\"m\":65536,\"t\":3,\"p\":4,",
        fp_of(&s)
    );
    assert!(env.starts_with(&want), "{env}");
    let h = header_of(&env).unwrap();
    assert_eq!((h.params, h.fingerprint), (KdfParams::DEFAULT, fp_of(&s)));
    assert_eq!(*open(&env, &p).unwrap(), s);
}

/// What `seal` writes is byte for byte what the format description builds
/// from the same salt and nonce.
#[test]
fn the_envelope_is_exactly_the_documented_format() {
    let s = seed();
    let p = passphrase();
    let env = seal(&s, &p, FAST).unwrap();
    let salt: [u8; 16] = STANDARD
        .decode(field(&env, "salt"))
        .unwrap()
        .try_into()
        .unwrap();
    let nonce: [u8; 24] = STANDARD
        .decode(field(&env, "nonce"))
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(env, by_hand(&s, &fp_of(&s), p.as_bytes(), &salt, &nonce));
    // And what the description builds, `open` opens.
    let other = seed();
    let built = by_hand(&other, &fp_of(&other), p.as_bytes(), &salt, &nonce);
    assert_eq!(*open(&built, &p).unwrap(), other);
}

/// L4: the header is associated data. Relabelling an envelope with another
/// VALID fingerprint fails the AEAD tag (`Open`), before any comparison of
/// fingerprints could run; it is never reported as a mismatch.
#[test]
fn a_relabelled_fingerprint_fails_the_aead_not_the_comparison() {
    let p = passphrase();
    let s = seed();
    let env = seal(&s, &p, FAST).unwrap();
    let other = fp_of(&seed());
    let relabelled = env.replace(&fp_of(&s), &other);
    assert_ne!(relabelled, env);
    assert_eq!(header_of(&relabelled).unwrap().fingerprint, other);
    assert_eq!(open(&relabelled, &p), Err(BackupError::Open));
}

/// An envelope sealed (authentically) under a fingerprint that is not its
/// seed's opens the AEAD but is refused: the label must name the key.
#[test]
fn a_seed_that_is_not_the_named_key_is_refused() {
    let p = passphrase();
    let s = seed();
    let env = seal_as(&s, &fp_of(&seed()), &p, FAST).unwrap();
    assert_eq!(open(&env, &p), Err(BackupError::FingerprintMismatch));
}

#[test]
fn fresh_salt_and_nonce_every_seal() {
    let s = seed();
    let p = passphrase();
    let a = seal(&s, &p, FAST).unwrap();
    let b = seal(&s, &p, FAST).unwrap();
    assert_ne!(a, b);
    assert_ne!(field(&a, "salt"), field(&b, "salt"));
    assert_ne!(field(&a, "nonce"), field(&b, "nonce"));
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
/// envelope's salt, nonce or ciphertext, or other in-range parameters.
#[test]
fn a_different_header_is_refused() {
    let p = passphrase();
    let a = seal(&seed(), &p, FAST).unwrap();
    let b = seal(&seed(), &p, FAST).unwrap();
    let swap = |k: &str| a.replace(&field(&a, k), &field(&b, k));
    for (name, t) in [
        ("salt from b", swap("salt")),
        ("nonce from b", swap("nonce")),
        ("ct from b", swap("ct")),
        ("fingerprint from b", swap("fingerprint")),
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
        assert_eq!(header_of(&t), Err(BackupError::UnknownVersion(v)));
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
    assert_eq!(KdfParams::MIN.check(), Ok(()));
    assert_eq!(KdfParams::MAX.check(), Ok(()));
    assert_eq!(KdfParams::DEFAULT.check(), Ok(()));
}

#[test]
fn anything_but_the_canonical_text_is_not_an_envelope() {
    let p = passphrase();
    let env = seal(&seed(), &p, FAST).unwrap();
    let v: serde_json::Value = serde_json::from_str(&env).unwrap();
    let ct = field(&env, "ct");
    let salt = field(&env, "salt"); // 16 bytes: a padded field
    let fp = field(&env, "fingerprint");
    assert!(salt.ends_with("=="));
    let cases = [
        ("not JSON", "a backup".to_string()),
        ("pretty-printed", serde_json::to_string_pretty(&v).unwrap()),
        ("extra key", env.replacen('{', "{\"x\":1,", 1)),
        ("duplicate key", env.replacen('{', "{\"ct\":\"\",", 1)),
        ("missing ct", env.replace(&format!(",\"ct\":\"{ct}\""), "")),
        ("unpadded", env.replace(&salt, salt.trim_end_matches('='))),
        ("short ct", env.replace(&ct, &STANDARD.encode([0u8; 47]))),
        ("long ct", env.replace(&ct, &STANDARD.encode([0u8; 49]))),
        ("space after colon", env.replacen("\"v\":1", "\"v\": 1", 1)),
        ("a JSON array", "[1]".to_string()),
        ("fingerprint, no prefix", env.replace(&fp, &fp[7..])),
        ("fingerprint, padded", env.replace(&fp, &format!("{fp}="))),
        ("fingerprint, short", env.replace(&fp, &fp[..49])),
        (
            "fingerprint, md5",
            env.replace(&fp, &fp.replace("SHA256:", "MD5:")),
        ),
        (
            "keys reordered",
            env.replacen(&format!("\"fingerprint\":\"{fp}\","), "", 1)
                .replacen("\"m\":", &format!("\"fingerprint\":\"{fp}\",\"m\":"), 1),
        ),
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
    assert!(open(&format!("{env}\n"), &p).is_ok(), "a trailing newline");
}

/// Sealing needs twelve characters, counted after NFC; opening applies no
/// floor, so an envelope made elsewhere with a short passphrase still opens.
#[test]
fn the_passphrase_floor_is_for_sealing_only() {
    let s = seed();
    assert_eq!(
        seal(&s, "elevenchars", FAST),
        Err(BackupError::PassphraseWeak)
    );
    assert!(seal(&s, "twelve chars", FAST).is_ok());
    // Twelve code points as typed (NFD), six characters after NFC.
    let nfd6 = "e\u{301}".repeat(6);
    assert_eq!(nfd6.chars().count(), 12);
    assert_eq!(seal(&s, &nfd6, FAST), Err(BackupError::PassphraseWeak));
    let long = "a".repeat(MAX_PASSPHRASE_BYTES + 1);
    assert_eq!(seal(&s, "", FAST), Err(BackupError::Passphrase));
    assert_eq!(seal(&s, &long, FAST), Err(BackupError::Passphrase));
    let short = by_hand(&s, &fp_of(&s), b"short", &[3u8; 16], &[4u8; 24]);
    assert_eq!(*open(&short, "short").unwrap(), s, "no floor on opening");
    assert_eq!(open(&short, ""), Err(BackupError::Passphrase));
    assert_eq!(open(&short, &long), Err(BackupError::Passphrase));
}

/// NFC on both sides: sealed with one form, opened with the other; and an
/// envelope built by hand over the NFC bytes opens with the NFD form. The
/// key is derived from the NFC bytes, never the typed ones.
#[test]
fn passphrases_are_nfc_normalised_on_both_sides() {
    let nfd = "cafe\u{301} contrasen\u{303}a";
    let nfc = "caf\u{e9} contrase\u{f1}a";
    assert_ne!(nfd.as_bytes(), nfc.as_bytes());
    let s = seed();
    assert_eq!(*open(&seal(&s, nfd, FAST).unwrap(), nfc).unwrap(), s);
    assert_eq!(*open(&seal(&s, nfc, FAST).unwrap(), nfd).unwrap(), s);
    let built = by_hand(&s, &fp_of(&s), nfc.as_bytes(), &[5u8; 16], &[6u8; 24]);
    assert_eq!(*open(&built, nfd).unwrap(), s);
    let raw_nfd = by_hand(&s, &fp_of(&s), nfd.as_bytes(), &[5u8; 16], &[6u8; 24]);
    assert_eq!(
        open(&raw_nfd, nfd),
        Err(BackupError::Open),
        "the NFD bytes are not the key"
    );
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

/// L7: no refusal quotes its input, whatever is malformed.
#[test]
fn refusals_never_echo_the_input() {
    let marker = "ECHOMARKER9c1d";
    let p = passphrase();
    let env = seal(&seed(), &p, FAST).unwrap();
    let fp = field(&env, "fingerprint");
    let salt = field(&env, "salt");
    for t in [
        format!("not json {marker}"),
        format!("{{\"v\":\"{marker}\"}}"),
        env.replacen('{', &format!("{{\"{marker}\":1,"), 1),
        env.replace(&fp, marker),
        env.replace(&salt, marker),
        env.replace("\"argon2id\"", &format!("\"{marker}\"")),
        env.replace("\"m\":19456", &format!("\"m\":\"{marker}\"")),
    ] {
        let e = open(&t, &p).unwrap_err();
        assert!(!e.to_string().contains(marker), "{e}");
        assert!(!format!("{e:?}").contains(marker), "{e:?}");
    }
    let e = open(&env, &format!("{marker}{marker}")).unwrap_err();
    assert!(!format!("{e} {e:?}").contains(marker));
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
        BackupError::PassphraseWeak,
        BackupError::Open,
        BackupError::FingerprintMismatch,
        BackupError::RandomUnavailable,
    ];
    let codes: std::collections::BTreeSet<&str> = all.iter().map(|e| e.code()).collect();
    assert_eq!(codes.len(), all.len());
}
