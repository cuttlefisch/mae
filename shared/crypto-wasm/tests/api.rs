//! The facade's operations, natively: the documented envelope, keys used by
//! handle, every refusal reaching the caller with its code, and no secret in
//! any response but `export_seed`'s. Each test runs on its own thread, so
//! each has its own keystore (as each module instance does).

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use mae_crypto::identity_challenge::challenge_text;
use mae_crypto_wasm::api::{call, Op, API_VERSION};
use serde_json::{json, Value};

fn resp(op: Op, req: &Value) -> Value {
    let raw = call(op as u32, req.to_string().as_bytes());
    serde_json::from_str(&raw).expect("every response is JSON")
}

fn ok(op: Op, req: Value) -> Value {
    let r = resp(op, &req);
    assert_eq!(r["ok"], true, "{op:?} {req}: {r}");
    r["result"].clone()
}

fn code(op: Op, req: Value) -> String {
    let r = resp(op, &req);
    assert_eq!(r["ok"], false, "{op:?} {req}: {r}");
    assert!(r["error"].is_string());
    r["code"].as_str().unwrap().to_string()
}

const ISS: &str = "https://idp.example.com";
const SUB: &str = "alice@idp";
const NOW: u64 = 1_900_000_000;
/// The floor KDF parameters: valid, and the cheapest to test with.
fn fast() -> Value {
    json!({"m": 19456, "t": 2, "p": 1})
}

fn challenge_for(fp: &str, iss: &str, sub: &str, exp: u64) -> String {
    challenge_text(iss, sub, "https://mae.example.com", fp, "bm9uY2U", exp)
}

const AUD: &str = "https://mae.example.com";

fn sign_req(key: &Value, challenge: &str, now: u64) -> Value {
    json!({"key": key, "issuer": ISS, "subject": SUB, "audience": AUD, "challenge": challenge, "now": now})
}

#[test]
fn version_is_the_contract_and_moves_with_mae_crypto() {
    let v = ok(Op::Version, json!({}));
    assert_eq!(v["version"], API_VERSION);
    assert_eq!(
        API_VERSION,
        mae_crypto::VERSION,
        "the two crates move together"
    );
    assert_eq!(v["backup_version"], 1);
    assert_eq!(v["challenge_domain"], "mae-identity-binding-v1");
    assert_eq!(v["content_op_domain"], "maecontent/v1");
}

#[test]
fn malformed_requests_and_unknown_ops_are_error_responses() {
    assert_eq!(code(Op::Version, json!({"x": 1})), "bad-request");
    assert_eq!(code(Op::KeyInfo, json!({})), "bad-request");
    assert_eq!(code(Op::KeyInfo, json!({"key": "1"})), "bad-request");
    assert_eq!(code(Op::ImportSeed, json!({"seed": "AAAA"})), "bad-request");
    assert_eq!(
        code(
            Op::ContentSeal,
            json!({"content_key": STANDARD.encode([1u8; 32]), "plaintext": "", "sealed": ""})
        ),
        "bad-request",
        "a key another operation takes is still unknown here"
    );
    for raw in ["", "{", "[]", "null", "{\"key\":1}{}"] {
        let r: Value = serde_json::from_str(&call(Op::KeyInfo as u32, raw.as_bytes())).unwrap();
        assert_eq!(r["code"], "bad-request", "{raw:?}");
    }
    let invalid_utf8 = [b'{', 0xff, b'}'];
    let r: Value = serde_json::from_str(&call(1, &invalid_utf8)).unwrap();
    assert_eq!(r["code"], "bad-request");
    // 0, the first number past the last op, and the top of the range.
    for op in [0, Op::ALL.len() as u32 + 1, u32::MAX] {
        let r: Value = serde_json::from_str(&call(op, b"{}")).unwrap();
        assert_eq!(r["code"], "unknown-op");
    }
}

#[test]
fn keys_are_held_by_handle_and_forgotten_for_good() {
    let a = ok(Op::Generate, json!({}));
    let b = ok(Op::Generate, json!({}));
    assert_ne!(a["key"], b["key"]);
    assert_ne!(a["public_key"], b["public_key"]);
    assert_eq!(ok(Op::KeyInfo, json!({"key": a["key"]})), a);
    let fp = ok(Op::Fingerprint, json!({"public_key": a["public_key"]}));
    assert_eq!(fp["fingerprint"], a["fingerprint"]);
    assert_eq!(ok(Op::Forget, json!({"key": a["key"]}))["forgotten"], true);
    assert_eq!(ok(Op::Forget, json!({"key": a["key"]}))["forgotten"], false);
    assert_eq!(code(Op::KeyInfo, json!({"key": a["key"]})), "no-key");
    assert_eq!(code(Op::ExportSeed, json!({"key": a["key"]})), "no-key");
    // A new key never takes a forgotten key's handle, nor a held one's.
    let c = ok(Op::Generate, json!({}));
    assert_ne!(c["key"], a["key"]);
    assert_ne!(c["key"], b["key"]);
    assert_eq!(
        ok(Op::KeyInfo, json!({"key": b["key"]})),
        b,
        "b is untouched"
    );
    assert_eq!(ok(Op::KeyInfo, json!({"key": c["key"]})), c);
}

#[test]
fn the_keystore_is_bounded() {
    let mut held = vec![];
    loop {
        let r = resp(Op::Generate, &json!({}));
        if r["ok"] == false {
            assert_eq!(r["code"], "keystore-full");
            break;
        }
        held.push(r["result"]["key"].clone());
        assert!(held.len() <= 16);
    }
    assert_eq!(held.len(), 16);
    ok(Op::Forget, json!({"key": held[0]}));
    ok(Op::Generate, json!({}));
}

#[test]
fn a_seed_leaves_only_when_asked_and_comes_back_as_the_same_key() {
    let a = ok(Op::Generate, json!({}));
    let seed = ok(Op::ExportSeed, json!({"key": a["key"]}))["seed"].clone();
    let raw = STANDARD.decode(seed.as_str().unwrap()).unwrap();
    assert_eq!(raw.len(), 32);
    let b = ok(Op::ImportSeed, json!({"seed": seed}));
    assert_ne!(b["key"], a["key"]);
    for f in ["public_key", "fingerprint", "wrap_public"] {
        assert_eq!(a[f], b[f], "{f}");
    }
    // No other response carries the seed, in any encoding.
    let forms = [
        STANDARD.encode(&raw),
        STANDARD_NO_PAD.encode(&raw),
        raw.iter().map(|x| format!("{x:02x}")).collect::<String>(),
    ];
    let fp = a["fingerprint"].as_str().unwrap().to_string();
    let text = challenge_for(&fp, ISS, SUB, NOW + 60);
    let responses = [
        call(
            Op::KeyInfo as u32,
            json!({"key": a["key"]}).to_string().as_bytes(),
        ),
        call(
            Op::SignChallenge as u32,
            sign_req(&a["key"], &text, NOW).to_string().as_bytes(),
        ),
        call(
            Op::BackupSeal as u32,
            json!({"key": a["key"], "passphrase": "twelve chars", "kdf": fast()})
                .to_string()
                .as_bytes(),
        ),
    ];
    for r in &responses {
        assert!(r.contains("\"ok\":true"), "{r}");
        for form in &forms {
            assert!(
                !r.contains(form.as_str()),
                "a response carries the seed: {r}"
            );
        }
    }
}

#[test]
fn challenges_are_signed_under_the_shared_rules_and_refused_with_their_codes() {
    let a = ok(Op::Generate, json!({}));
    let other = ok(Op::Generate, json!({}));
    let fp = a["fingerprint"].as_str().unwrap();
    let good = challenge_for(fp, ISS, SUB, NOW + 60);
    let s = ok(Op::SignChallenge, sign_req(&a["key"], &good, NOW));
    assert_eq!(s["challenge"]["sub"], SUB);
    assert_eq!(s["body"]["public_key"], a["public_key"]);
    assert_eq!(s["body"]["challenge_id"], "bm9uY2U");
    let sig = STANDARD
        .decode(s["body"]["signature"].as_str().unwrap())
        .unwrap();
    let pk: [u8; 32] = STANDARD
        .decode(a["public_key"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    assert!(mae_crypto::member::verify(&pk, good.as_bytes(), &sig));
    assert_eq!(
        ok(Op::CheckChallenge, sign_req(&a["key"], &good, NOW))["challenge"],
        s["challenge"]
    );
    let cases = [
        ("other-key", sign_req(&other["key"], &good, NOW)),
        ("expired", sign_req(&a["key"], &good, NOW + 60)),
        (
            "other-subject",
            sign_req(
                &a["key"],
                &challenge_for(fp, ISS, "mallory@idp", NOW + 60),
                NOW,
            ),
        ),
        (
            "other-issuer",
            sign_req(
                &a["key"],
                &challenge_for(fp, "https://idp.example.org", SUB, NOW + 60),
                NOW,
            ),
        ),
        (
            "not-a-challenge",
            sign_req(&a["key"], &good.replace("\":", "\": "), NOW),
        ),
        ("not-a-challenge", sign_req(&a["key"], "sign me", NOW)),
        ("other-audience", {
            let mut r = sign_req(&a["key"], &good, NOW);
            r["audience"] = json!("https://other.example.com");
            r
        }),
        (
            "expiry-too-far",
            sign_req(&a["key"], &challenge_for(fp, ISS, SUB, NOW + 3601), NOW),
        ),
        (
            "wrong-domain",
            sign_req(
                &a["key"],
                &good.replace("mae-identity-binding-v1", "mae-x-v1"),
                NOW,
            ),
        ),
        (
            "empty-field",
            sign_req(&a["key"], &challenge_for(fp, ISS, "", NOW + 60), NOW),
        ),
    ];
    for (want, req) in cases {
        assert_eq!(code(Op::SignChallenge, req.clone()), want, "{req}");
        assert_eq!(code(Op::CheckChallenge, req.clone()), want, "{req}");
    }
}

#[test]
fn a_backup_opens_only_with_its_passphrase_and_only_unaltered() {
    let a = ok(Op::Generate, json!({}));
    let env = ok(
        Op::BackupSeal,
        json!({"key": a["key"], "passphrase": "pässwörd lang", "kdf": fast()}),
    )["envelope"]
        .as_str()
        .unwrap()
        .to_string();
    let mut want = fast();
    want["fingerprint"] = a["fingerprint"].clone();
    assert_eq!(ok(Op::BackupParams, json!({"envelope": env})), want);
    assert_eq!(
        code(
            Op::BackupSeal,
            json!({"key": a["key"], "passphrase": "elevenchars", "kdf": fast()})
        ),
        "passphrase-weak"
    );
    let b = ok(
        Op::BackupOpen,
        json!({"envelope": env, "passphrase": "pässwörd lang"}),
    );
    assert_eq!(b["fingerprint"], a["fingerprint"]);
    assert_ne!(b["key"], a["key"], "opening gives a new handle");
    assert_eq!(
        code(
            Op::BackupOpen,
            json!({"envelope": env, "passphrase": "passwort"})
        ),
        "open"
    );
    let tampered = env.replace("\"v\":1", "\"v\":2");
    assert_eq!(
        code(
            Op::BackupOpen,
            json!({"envelope": tampered, "passphrase": "pässwörd lang"})
        ),
        "unknown-version"
    );
    let ct = serde_json::from_str::<Value>(&env).unwrap()["ct"]
        .as_str()
        .unwrap()
        .to_string();
    let mut flipped = ct.clone().into_bytes();
    flipped[5] = if flipped[5] == b'A' { b'B' } else { b'A' };
    let altered = env.replace(&ct, std::str::from_utf8(&flipped).unwrap());
    assert_eq!(
        code(
            Op::BackupOpen,
            json!({"envelope": altered, "passphrase": "pässwörd lang"})
        ),
        "open"
    );
    let weak = json!({"key": a["key"], "passphrase": "p", "kdf": {"m": 1024, "t": 1, "p": 1}});
    assert_eq!(code(Op::BackupSeal, weak), "kdf-params");
    assert_eq!(
        code(Op::BackupSeal, json!({"key": a["key"], "passphrase": ""})),
        "passphrase"
    );
    assert_eq!(
        code(Op::BackupSeal, json!({"key": 999, "passphrase": "p"})),
        "no-key"
    );
}

#[test]
fn content_crypto_round_trips_and_refuses_the_wrong_key_or_member() {
    let ck = ok(Op::ContentKeyGenerate, json!({}))["content_key"].clone();
    let other_ck = ok(Op::ContentKeyGenerate, json!({}))["content_key"].clone();
    assert_ne!(ck, other_ck);
    for n in [0usize, 1, 64, 70_000] {
        let pt = STANDARD.encode((0..n).map(|i| (i * 7 % 251) as u8).collect::<Vec<u8>>());
        let sealed =
            ok(Op::ContentSeal, json!({"content_key": ck, "plaintext": pt}))["sealed"].clone();
        assert_eq!(
            ok(
                Op::ContentOpen,
                json!({"content_key": ck, "sealed": sealed})
            )["plaintext"],
            pt
        );
        assert_eq!(
            code(
                Op::ContentOpen,
                json!({"content_key": other_ck, "sealed": sealed})
            ),
            "decrypt"
        );
    }
    let alice = ok(Op::Generate, json!({}));
    let bob = ok(Op::Generate, json!({}));
    let wrapped = ok(
        Op::ContentWrap,
        json!({"content_key": ck, "wrap_public": alice["wrap_public"]}),
    )["wrapped"]
        .clone();
    assert_eq!(
        ok(
            Op::ContentUnwrap,
            json!({"key": alice["key"], "wrapped": wrapped})
        )["content_key"],
        ck
    );
    assert_eq!(
        code(
            Op::ContentUnwrap,
            json!({"key": bob["key"], "wrapped": wrapped})
        ),
        "decrypt"
    );
    let low_order = STANDARD.encode([0u8; 32]);
    assert_eq!(
        code(
            Op::ContentWrap,
            json!({"content_key": ck, "wrap_public": low_order})
        ),
        "malformed"
    );
    assert_eq!(
        code(
            Op::ContentOpen,
            json!({"content_key": ck, "sealed": "AAAA"})
        ),
        "malformed"
    );
}

/// L7: an error response never contains the bytes of a malformed secret or
/// challenge field, whatever is wrong with it.
#[test]
fn error_responses_never_echo_their_input() {
    let marker = "ECHOMARKER5e2b";
    let a = ok(Op::Generate, json!({}));
    let k = &a["key"];
    let ck = STANDARD.encode([7u8; 32]);
    let fp = a["fingerprint"].as_str().unwrap().to_string();
    let good = challenge_for(&fp, ISS, SUB, NOW + 60);
    let env = ok(
        Op::BackupSeal,
        json!({"key": k, "passphrase": "twelve chars", "kdf": fast()}),
    )["envelope"]
        .as_str()
        .unwrap()
        .to_string();
    let mut as_key = json!({});
    as_key[marker] = json!(1);
    let bad_values = [
        json!(marker),
        json!(format!("{marker}==")),
        as_key,
        json!([marker]),
    ];
    let mut cases: Vec<(Op, Value)> = vec![];
    for v in &bad_values {
        cases.push((Op::ImportSeed, json!({"seed": v})));
        cases.push((Op::Fingerprint, json!({"public_key": v})));
        if !v.is_string() {
            // A string passphrase this long is valid; only wrong types err.
            cases.push((Op::BackupSeal, json!({"key": k, "passphrase": v})));
        }
        cases.push((Op::BackupOpen, json!({"envelope": env, "passphrase": v})));
        cases.push((
            Op::BackupOpen,
            json!({"envelope": v, "passphrase": "twelve chars"}),
        ));
        cases.push((Op::BackupParams, json!({"envelope": v})));
        cases.push((Op::ContentSeal, json!({"content_key": v, "plaintext": ""})));
        cases.push((Op::ContentOpen, json!({"content_key": ck, "sealed": v})));
        cases.push((
            Op::ContentWrap,
            json!({"content_key": v, "wrap_public": ck}),
        ));
        cases.push((Op::ContentUnwrap, json!({"key": k, "wrapped": v})));
        for f in ["issuer", "subject", "audience", "challenge"] {
            let mut r = sign_req(k, &good, NOW);
            r[f] = v.clone();
            cases.push((Op::SignChallenge, r.clone()));
            cases.push((Op::CheckChallenge, r));
        }
    }
    // Well-formed JSON whose values are wrong in content, not in type.
    for t in [
        good.replace("https://mae.example.com", marker),
        good.replacen('{', &format!("{{\"{marker}\":1,"), 1),
        format!("{{\"challenge_id\":\"x\",\"challenge\":\"{marker}\",\"expires_in\":1}}"),
    ] {
        cases.push((Op::SignChallenge, sign_req(k, &t, NOW)));
    }
    cases.push((
        Op::BackupOpen,
        json!({"envelope": env.replace(&fp, marker), "passphrase": "twelve chars"}),
    ));
    let mut extra = json!({"key": k});
    extra[marker] = json!(1);
    cases.push((Op::KeyInfo, extra));
    for (op, req) in cases {
        let r = call(op as u32, req.to_string().as_bytes());
        assert!(r.contains("\"ok\":false"), "{op:?} {req}: {r}");
        assert!(!r.contains(marker), "{op:?} echoed its input: {r}");
    }
}
