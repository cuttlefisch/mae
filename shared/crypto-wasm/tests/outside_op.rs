//! `op_canonical_bytes` and `verify_op` (ADR-098 Amendment 2): a client that
//! holds its member key outside the module (a non-extractable WebCrypto key)
//! gets the exact bytes ADR-036 D1 signs, signs them itself, and sends the
//! header. Signing those bytes with plain Ed25519 must give the very header
//! `sign_op` gives for the same op, and the daemon's own verifier must accept
//! it on both write paths. Each attack must fail `verify_op` and the daemon.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mae_crypto::member::MemberKey;
use mae_crypto_wasm::api::{call, Op};
use mae_sync::content_ops::SignedContentOp;
use serde_json::{json, Value};

const SEED: [u8; 32] = [0x5a; 32];
const EPOCH: &str = "18446744073709551557";

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
    r["code"].as_str().unwrap().to_string()
}

/// The op's fields as an outside signer names them: the key by its public part.
fn fields(key: &MemberKey, update: &[u8]) -> Value {
    json!({
        "kb_id": "kb-alpha",
        "node_id": "concept:buffer",
        "base_sv": STANDARD.encode([1u8, 2, 0, 255]),
        "author_pubkey": key.public_key_b64(),
        "epoch": EPOCH,
        "issued_at": 1_790_000_000u64,
        "update": STANDARD.encode(update),
    })
}

/// What the browser does: sign the canonical bytes with its own key, and send
/// the header the module returned with that signature.
fn signed_outside(key: &MemberKey, update: &[u8]) -> Value {
    let mut h = ok(Op::OpCanonicalBytes, fields(key, update));
    let canonical = STANDARD.decode(h["canonical"].as_str().unwrap()).unwrap();
    let sig = key.sign(&canonical);
    let o = h.as_object_mut().unwrap();
    o.remove("canonical");
    o.insert("sig".into(), json!(STANDARD.encode(sig)));
    h
}

fn verifies(header: &Value, kb: &str, node: &str, update: &[u8]) -> bool {
    let mut p = header.clone();
    p["kb_id"] = json!(kb);
    p["node_id"] = json!(node);
    SignedContentOp::from_params(&p, update.to_vec()).is_some_and(|s| s.verify_signed())
}

fn verify_op(fields_and_sig: Value) -> bool {
    ok(Op::VerifyOp, fields_and_sig)["valid"].as_bool().unwrap()
}

/// The outside path and `sign_op` give the same header for the same op, and
/// the daemon accepts it for `kb/node_update` and `sync/update` alike. Kills
/// any drift between the bytes handed out and the bytes the daemon verifies.
#[test]
fn signing_the_canonical_bytes_outside_gives_sign_ops_header() {
    let key = MemberKey::from_seed(&SEED);
    let update = b"\x01\x00yrs update\x00bytes".to_vec();
    let outside = signed_outside(&key, &update);

    let held = ok(Op::ImportSeed, json!({ "seed": STANDARD.encode(SEED) }));
    let mut req = fields(&key, &update);
    let o = req.as_object_mut().unwrap();
    o.remove("author_pubkey");
    o.insert("key".into(), held["key"].clone());
    let inside = ok(Op::SignOp, req);
    assert_eq!(outside, inside, "one op, one header, whoever signs it");

    assert_eq!(outside["author"], json!(key.fingerprint()));
    assert!(verifies(&outside, "kb-alpha", "concept:buffer", &update));
    let mut v = fields(&key, &update);
    v["sig"] = outside["sig"].clone();
    assert!(verify_op(v));
}

/// `verify_op` answers as the daemon does: another node or KB, a changed
/// update, another key's signature, a claimed author all fail. Kills a
/// verifier that checks less than the daemon.
#[test]
fn verify_op_refuses_what_the_daemon_refuses() {
    let key = MemberKey::from_seed(&SEED);
    let other = MemberKey::from_seed(&[0x11; 32]);
    let update = b"edit".to_vec();
    let header = signed_outside(&key, &update);
    let with = |k: &str, v: Value| {
        let mut f = fields(&key, &update);
        f["sig"] = header["sig"].clone();
        f[k] = v;
        f
    };
    assert!(!verify_op(with("node_id", json!("concept:other"))));
    assert!(!verify_op(with("kb_id", json!("kb-beta"))));
    assert!(!verify_op(with("update", json!(STANDARD.encode(b"edit!")))));
    assert!(!verify_op(with("epoch", json!("7"))));
    assert!(!verify_op(with(
        "author_pubkey",
        json!(other.public_key_b64())
    )));
    assert!(!verify_op(with("author", json!(other.fingerprint()))));
    let forged = STANDARD.encode(other.sign(b"anything"));
    assert!(!verify_op(with("sig", json!(forged))));
    assert!(!verifies(&header, "kb-alpha", "concept:other", &update));
    assert!(verify_op(with("author", json!(key.fingerprint()))));
}

/// A request that names no op is refused with its code, as `sign_op`'s is:
/// encodable ids, a canonical epoch, a 32-byte key, base64; a claimed author
/// that is not the key's; a signature offered to the wrong operation.
#[test]
fn a_request_that_names_no_op_is_refused() {
    let key = MemberKey::from_seed(&SEED);
    let base = fields(&key, b"u");
    let with = |k: &str, v: Value| {
        let mut f = base.clone();
        f[k] = v;
        f
    };
    let c = |r: Value| code(Op::OpCanonicalBytes, r);
    assert_eq!(c(with("kb_id", json!(""))), "empty-field");
    assert_eq!(c(with("node_id", json!("a\u{0}b"))), "nul-in-field");
    for bad in ["007", "-1", "", "18446744073709551616"] {
        assert_eq!(c(with("epoch", json!(bad))), "bad-request", "{bad}");
    }
    assert_eq!(
        c(with("author_pubkey", json!(STANDARD.encode([1u8; 31])))),
        "bad-request"
    );
    assert_eq!(c(with("update", json!("not base64!"))), "bad-request");
    assert_eq!(c(with("author", json!("SHA256:someone"))), "bad-request");
    assert_eq!(
        c(with("sig", json!(STANDARD.encode([0u8; 64])))),
        "bad-request"
    );
    assert_eq!(c(with("extra", json!(1))), "bad-request");
    assert_eq!(
        code(Op::VerifyOp, base.clone()),
        "bad-request",
        "verify_op needs sig"
    );
}
