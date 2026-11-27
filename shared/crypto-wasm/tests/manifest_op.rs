//! `sign_manifest_op` and `manifest_op_canonical_bytes` (ADR-036 Amendment 2)
//! against the verifier the daemon runs (`ManifestOp::verify`): the pinned
//! vector, each changed field failing, and the requests the encoding cannot
//! carry refused with their codes.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mae_crypto_wasm::api::{call, Op};
use mae_sync::manifest_op::{ManifestAction, ManifestOp};
use serde_json::{json, Value};

fn resp(op: Op, req: &Value) -> Value {
    serde_json::from_str(&call(op as u32, req.to_string().as_bytes())).expect("JSON")
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

fn key() -> Value {
    ok(
        Op::ImportSeed,
        json!({ "seed": STANDARD.encode([0x5au8; 32]) }),
    )
}

fn add(key: &Value) -> Value {
    json!({ "key": key["key"], "kb_id": "kb-alpha", "node_id": "concept:buffer", "action": "add",
            "title": "Buffer", "epoch": "7", "issued_at": 1_790_000_000u64 })
}

/// The op the daemon rebuilds from a request's params and the header.
fn as_daemon_reads(h: &Value) -> (ManifestOp, Vec<u8>, [u8; 32]) {
    let op = ManifestOp {
        kb_id: h["kb_id"].as_str().unwrap().into(),
        node_id: h["node_id"].as_str().unwrap().into(),
        action: match h["action"].as_str().unwrap() {
            "add" => ManifestAction::Add,
            _ => ManifestAction::Remove,
        },
        title: h["title"].as_str().unwrap().into(),
        author: h["author"].as_str().unwrap().into(),
        epoch: h["epoch"].as_str().unwrap().parse().unwrap(),
        issued_at: h["issued_at"].as_u64().unwrap(),
    };
    let sig = STANDARD.decode(h["sig"].as_str().unwrap()).unwrap();
    let pk: [u8; 32] = STANDARD
        .decode(h["author_pubkey"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    (op, sig, pk)
}

/// The WASM signature is mae-crypto's pinned vector, and the daemon's
/// verifier accepts it; a removal signs no title and verifies too.
#[test]
fn a_signed_manifest_op_is_the_pinned_vector_and_verifies() {
    let k = key();
    let h = ok(Op::SignManifestOp, add(&k));
    let sig_hex: String = STANDARD
        .decode(h["sig"].as_str().unwrap())
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(sig_hex, "3aae6f6a56a95395ebe4e27dc93075c005115bb0647705d9e158ef9e016693864dbaae39413797cfea7a5bafae905cd06bdda2ea09ee7a96013b586a8c562a02");
    let (op, sig, pk) = as_daemon_reads(&h);
    assert!(op.verify(&sig, &pk));
    let mut rm = add(&k);
    rm["action"] = json!("remove");
    rm.as_object_mut().unwrap().remove("title");
    let h = ok(Op::SignManifestOp, rm);
    let (op, sig, pk) = as_daemon_reads(&h);
    assert_eq!(op.title, "");
    assert!(op.verify(&sig, &pk));
}

/// Each field is bound: a header verifies only for the exact op signed.
#[test]
fn every_field_is_bound() {
    let h = ok(Op::SignManifestOp, add(&key()));
    let (op, sig, pk) = as_daemon_reads(&h);
    for changed in [
        ManifestOp {
            title: "Renamed".into(),
            ..op.clone()
        },
        ManifestOp {
            node_id: "concept:other".into(),
            ..op.clone()
        },
        ManifestOp {
            kb_id: "kb-beta".into(),
            ..op.clone()
        },
        ManifestOp {
            epoch: 8,
            ..op.clone()
        },
        ManifestOp {
            issued_at: 1,
            ..op.clone()
        },
    ] {
        assert!(!changed.verify(&sig, &pk), "{changed:?}");
    }
}

/// The canonical bytes for an outside key are exactly what the daemon
/// verifies, and the header matches `sign_manifest_op`'s.
#[test]
fn canonical_bytes_match_what_the_daemon_verifies() {
    let k = key();
    let mut req = add(&k);
    req.as_object_mut().unwrap().remove("key");
    req["author_pubkey"] = k["public_key"].clone();
    let u = ok(Op::ManifestOpCanonicalBytes, req);
    let signed = ok(Op::SignManifestOp, add(&k));
    let (op, _, _) = as_daemon_reads(&signed);
    assert_eq!(
        STANDARD.decode(u["canonical"].as_str().unwrap()).unwrap(),
        op.canonical_bytes()
    );
    assert_eq!(u["author"], signed["author"]);
}

/// What the encoding cannot carry, and malformed requests, are refused with
/// their codes, before anything is signed.
#[test]
fn unencodable_and_malformed_requests_are_refused() {
    let k = key();
    let with = |f: &dyn Fn(&mut Value)| {
        let mut r = add(&k);
        f(&mut r);
        r
    };
    let m = Op::SignManifestOp;
    assert_eq!(
        code(m, with(&|r| r["title"] = json!("a\u{0}b"))),
        "nul-in-field"
    );
    assert_eq!(
        code(m, with(&|r| r["node_id"] = json!("x\u{0}y"))),
        "nul-in-field"
    );
    assert_eq!(code(m, with(&|r| r["node_id"] = json!(""))), "empty-field");
    assert_eq!(
        code(m, with(&|r| r["action"] = json!("remove"))),
        "title-on-remove"
    );
    assert_eq!(
        code(m, with(&|r| r["action"] = json!("rename"))),
        "bad-request"
    );
    assert_eq!(code(m, with(&|r| r["epoch"] = json!("07"))), "bad-request");
    assert_eq!(code(m, with(&|r| r["key"] = json!(99_999))), "no-key");
    assert_eq!(code(m, with(&|r| r["extra"] = json!(1))), "bad-request");
}
