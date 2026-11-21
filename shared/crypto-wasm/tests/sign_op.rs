//! `sign_op` (ADR-036 F5) against the verifier the daemon runs.
//!
//! A header the WASM API returns is framed the two ways a browser sends it:
//! the top-level params of `kb/node_update`, and `params.content_header` of
//! `sync/update` (with the KB and node taken from the document name, as the
//! daemon does). Each must pass `mae_sync::content_ops::SignedContentOp`, the
//! daemon's own check. Each attack must fail it: another KB or node, a changed
//! update, another key, a claimed author. A request the canonical encoding
//! cannot carry is refused with its code.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mae_crypto_wasm::api::{call, Op};
use mae_sync::content_ops::SignedContentOp;
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
    r["code"].as_str().unwrap().to_string()
}

/// A held key from a fixed seed: the pinned vector's (0x5a x 32).
fn key() -> Value {
    ok(
        Op::ImportSeed,
        json!({ "seed": STANDARD.encode([0x5au8; 32]) }),
    )
}

/// A random epoch beyond 2^53, which a JavaScript number would round.
const EPOCH: &str = "18446744073709551557";

fn request(key: &Value, update: &[u8]) -> Value {
    json!({
        "key": key["key"],
        "kb_id": "kb-alpha",
        "node_id": "concept:buffer",
        "base_sv": STANDARD.encode([1u8, 2, 0, 255]),
        "epoch": EPOCH,
        "issued_at": 1_790_000_000u64,
        "update": STANDARD.encode(update),
    })
}

/// `kb/node_update`'s params: the request's own ids and update, plus the
/// header at the top level.
fn node_update_params(header: &Value, kb: &str, node: &str, update: &[u8]) -> Value {
    let mut p = header.clone();
    p["kb_id"] = json!(kb);
    p["node_id"] = json!(node);
    p["update"] = json!(STANDARD.encode(update));
    p
}

/// What the daemon verifies for `sync/update`: `content_header`, re-bound to
/// the KB and node parsed from the document name (content_verify::signed_op).
fn sync_update_verifies(header: &Value, doc: &str, update: &[u8]) -> bool {
    let rest = doc.strip_prefix("kbn:").unwrap();
    let (kb, node) = rest.split_once(':').unwrap();
    let mut p = header.clone();
    p["kb_id"] = json!(kb);
    p["node_id"] = json!(node);
    SignedContentOp::from_params(&p, update.to_vec()).is_some_and(|s| s.verify_signed())
}

fn node_update_verifies(params: &Value, update: &[u8]) -> bool {
    SignedContentOp::from_params(params, update.to_vec()).is_some_and(|s| s.verify_signed())
}

#[test]
fn a_signed_header_passes_the_daemons_check_on_both_write_paths() {
    let k = key();
    let update = b"\x01\x00yrs update\x00bytes".to_vec();
    let header = ok(Op::SignOp, request(&k, &update));

    // The header's author is the key's own fingerprint, and the epoch comes
    // back as the exact text it went in as.
    assert_eq!(header["author"], k["fingerprint"]);
    assert_eq!(header["author_pubkey"], k["public_key"]);
    assert_eq!(header["epoch"], EPOCH);

    let p = node_update_params(&header, "kb-alpha", "concept:buffer", &update);
    assert!(node_update_verifies(&p, &update), "kb/node_update");
    assert!(
        sync_update_verifies(&header, "kbn:kb-alpha:concept:buffer", &update),
        "sync/update"
    );
    // The epoch the verifier reads is the full u64, not a rounded one.
    let s = SignedContentOp::from_params(&p, update.clone()).unwrap();
    assert_eq!(s.op.epoch, EPOCH.parse::<u64>().unwrap());
}

#[test]
fn the_signature_is_the_pinned_vector() {
    // The same vector mae-crypto's content_op tests pin, and the editor's
    // seed-based signer produces: one implementation on every surface.
    let k = key();
    let mut req = request(&k, &[0u8, 1, 2, 3, 0, 9]);
    req["epoch"] = json!("7");
    let header = ok(Op::SignOp, req);
    let sig = STANDARD.decode(header["sig"].as_str().unwrap()).unwrap();
    let hex: String = sig.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex,
        "f98071fb0ab87e6107a5e0835b3beadc3aa532bee4de847b6dba6a0b1ba67a6fc8ee7c45935c5d3cb25143207acb1c39ab20f620bf16d135782318d049a87702"
    );
}

#[test]
fn a_header_signed_for_one_node_fails_for_any_other_or_a_changed_update() {
    let k = key();
    let update = b"edit".to_vec();
    let header = ok(Op::SignOp, request(&k, &update));
    // Another node or KB: the daemon re-binds the ids from its own context.
    assert!(!node_update_verifies(
        &node_update_params(&header, "kb-alpha", "concept:other", &update),
        &update
    ));
    assert!(!node_update_verifies(
        &node_update_params(&header, "kb-beta", "concept:buffer", &update),
        &update
    ));
    assert!(!sync_update_verifies(
        &header,
        "kbn:kb-beta:concept:buffer",
        &update
    ));
    // A changed update, epoch or state vector.
    let p = node_update_params(&header, "kb-alpha", "concept:buffer", &update);
    assert!(!node_update_verifies(&p, b"edit!"));
    let mut q = p.clone();
    q["epoch"] = json!("18446744073709551556");
    assert!(!node_update_verifies(&q, &update));
    let mut q = p.clone();
    q["base_sv"] = json!(STANDARD.encode([1u8, 2, 0]));
    assert!(!node_update_verifies(&q, &update));
    // Another member's key presented with it, or a claimed other author.
    let other = ok(
        Op::ImportSeed,
        json!({ "seed": STANDARD.encode([0x11u8; 32]) }),
    );
    let mut q = p.clone();
    q["author_pubkey"] = other["public_key"].clone();
    assert!(!node_update_verifies(&q, &update));
    let mut q = p.clone();
    q["author"] = other["fingerprint"].clone();
    assert!(!node_update_verifies(&q, &update));
}

#[test]
fn a_request_the_encoding_cannot_carry_is_refused_with_its_code() {
    let k = key();
    let base = request(&k, b"u");
    let with = |f: &str, v: Value| {
        let mut r = base.clone();
        r[f] = v;
        r
    };
    assert_eq!(code(Op::SignOp, with("kb_id", json!(""))), "empty-field");
    assert_eq!(
        code(Op::SignOp, with("node_id", json!("a\u{0}b"))),
        "nul-in-field"
    );
    // The epoch is canonical decimal text: one value, one text.
    for bad in ["", "007", "+7", "7.0", "1e3", "-1", "18446744073709551616"] {
        assert_eq!(
            code(Op::SignOp, with("epoch", json!(bad))),
            "bad-request",
            "{bad}"
        );
    }
    // A JSON number epoch is refused rather than silently accepted rounded.
    assert_eq!(code(Op::SignOp, with("epoch", json!(7))), "bad-request");
    // The author is the key's: a request cannot name one.
    assert_eq!(
        code(Op::SignOp, with("author", json!("SHA256:someone"))),
        "bad-request"
    );
    assert_eq!(
        code(Op::SignOp, with("update", json!("not base64!"))),
        "bad-request"
    );
    assert_eq!(code(Op::SignOp, with("key", json!(9999))), "no-key");
}

#[test]
fn the_daemon_accepts_a_decimal_text_epoch_only_in_canonical_form() {
    let k = key();
    let update = b"u".to_vec();
    let header = ok(Op::SignOp, request(&k, &update));
    let p = node_update_params(&header, "kb-alpha", "concept:buffer", &update);
    for bad in ["0018446744073709551557", "+18446744073709551557", "", " 1"] {
        let mut q = p.clone();
        q["epoch"] = json!(bad);
        assert!(
            SignedContentOp::from_params(&q, update.clone()).is_none(),
            "{bad:?} must not parse"
        );
    }
}
