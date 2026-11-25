//! The review event chain's format (ADR-113 D7, Amendment 4 E10): canonical
//! bytes, the entry hash, the daemon's signatures, the export lines, and the
//! offline verifier. No I/O and no storage: the store writes what this module
//! seals, and `mae-daemon kb review verify` checks what this module exports.
//!
//! An event is the JSON object `{v: 1, kb_id, seq, prev_hash, kind, at, actor,
//! payload, member_sig}`. Its canonical bytes are its RFC 8785 (JCS)
//! serialization over a restricted value set (ASCII keys, integers within
//! ±(2^53 − 1), no floats), for which JCS is: object keys sorted by their
//! bytes, no whitespace, strings escaped as ECMAScript's `JSON.stringify`
//! escapes them. `entry_hash = SHA-256("maereview/v1\n" ‖ canonical)`; the
//! daemon signs `"maereview-sig/v1\n" ‖ entry_hash` (the 32 raw bytes) with
//! pure Ed25519, the key ADR-116 D4 signs history heads with.
//!
//! Verifying needs no canonicalizer: it hashes the canonical bytes the export
//! carries. What it proves is only as strong as the key and head the verifier
//! brings from outside the daemon (E10, "What it proves"): the export's own
//! `daemon_key` is a claim and is never used to verify.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use mae_mcp::identity::{Identity, PublicKey};
use mae_sync::encoding::{base64_to_update as b64_decode, update_to_base64 as b64_encode};

/// The `prev_hash` of a KB's first event.
pub const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
pub const EXPORT_FORMAT: &str = "maereview-export/v1";
pub const HEAD_FORMAT: &str = "maereview-head/v1";
/// The event object's own version.
pub const EVENT_V: i64 = 1;

const ENTRY_DOMAIN: &[u8] = b"maereview/v1\n";
const SIG_DOMAIN: &[u8] = b"maereview-sig/v1\n";
const HEAD_SIG_DOMAIN: &[u8] = b"maereview-head-sig/v1\n";
/// The largest integer every JSON reader holds exactly (E10).
const MAX_SAFE_INT: i64 = (1 << 53) - 1;

/// A value outside E10's restricted set, which JCS would not encode one way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonError {
    NonAsciiKey(String),
    Float,
    IntegerOutOfRange,
}

impl std::fmt::Display for CanonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonAsciiKey(k) => write!(f, "object key {k:?} is not ASCII"),
            Self::Float => f.write_str("a number is not an integer"),
            Self::IntegerOutOfRange => f.write_str("an integer is outside ±(2^53 − 1)"),
        }
    }
}

/// The RFC 8785 bytes of `v`, refusing anything outside E10's value set.
pub fn canonical(v: &Value) -> Result<Vec<u8>, CanonError> {
    let mut out = Vec::new();
    write_canonical(v, &mut out)?;
    Ok(out)
}

fn write_canonical(v: &Value, out: &mut Vec<u8>) -> Result<(), CanonError> {
    match v {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) => {
            let i = match (n.as_i64(), n.as_u64()) {
                (Some(i), _) => i,
                (None, Some(_)) => return Err(CanonError::IntegerOutOfRange),
                (None, None) => return Err(CanonError::Float),
            };
            if !(-MAX_SAFE_INT..=MAX_SAFE_INT).contains(&i) {
                return Err(CanonError::IntegerOutOfRange);
            }
            out.extend_from_slice(i.to_string().as_bytes());
        }
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            // ASCII keys: byte order is JCS's UTF-16 code-unit order.
            let mut keys: Vec<&String> = map.keys().collect();
            if let Some(k) = keys.iter().find(|k| !k.is_ascii()) {
                return Err(CanonError::NonAsciiKey((*k).clone()));
            }
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_string(k, out);
                out.push(b':');
                write_canonical(&map[k], out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

/// A string as ECMAScript's `JSON.stringify` writes it (RFC 8785 §3.2.2.2):
/// `"` and `\` escaped, the five short escapes, every other control character
/// as `\u00xx` in lowercase hex, and everything else as itself in UTF-8.
fn write_string(s: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{08}' => out.extend_from_slice(b"\\b"),
            '\u{0c}' => out.extend_from_slice(b"\\f"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes())
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// `SHA-256("maereview/v1\n" ‖ canonical)`.
pub fn entry_hash(canonical: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(ENTRY_DOMAIN);
    h.update(canonical);
    h.finalize().into()
}

fn sig_message(entry_hash: &[u8; 32]) -> Vec<u8> {
    [SIG_DOMAIN, &entry_hash[..]].concat()
}

/// One event before it is sealed: everything but the hashes and signature.
#[derive(Debug, Clone, PartialEq)]
pub struct EventBody {
    pub kb_id: String,
    pub seq: u64,
    pub prev_hash: String,
    pub kind: String,
    pub at: String,
    pub actor: Value,
    pub payload: Value,
    pub member_sig: Option<String>,
}

impl EventBody {
    /// The event object E10 hashes.
    pub fn to_value(&self) -> Value {
        json!({
            "v": EVENT_V,
            "kb_id": self.kb_id,
            "seq": self.seq,
            "prev_hash": self.prev_hash,
            "kind": self.kind,
            "at": self.at,
            "actor": self.actor,
            "payload": self.payload,
            "member_sig": self.member_sig,
        })
    }
}

/// A sealed event: its canonical bytes, their hash, and the daemon's signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub canonical: Vec<u8>,
    pub entry_hash: [u8; 32],
    pub sig: Vec<u8>,
}

/// Canonicalize, hash and sign `body` with `signer`.
pub fn seal(body: &EventBody, signer: &Identity) -> Result<Sealed, CanonError> {
    let canonical = canonical(&body.to_value())?;
    let entry_hash = entry_hash(&canonical);
    let sig = signer.sign(&sig_message(&entry_hash));
    Ok(Sealed {
        canonical,
        entry_hash,
        sig,
    })
}

/// The export's first line.
pub fn header_line(kb_id: &str, daemon_key: &[u8; 32]) -> Value {
    json!({
        "type": "header",
        "format": EXPORT_FORMAT,
        "kb_id": kb_id,
        "daemon_key": b64_encode(daemon_key),
    })
}

/// One event's export line.
pub fn event_line(seq: u64, prev_hash: &str, s: &Sealed) -> Value {
    json!({
        "type": "event",
        "seq": seq,
        "prev_hash": prev_hash,
        "canonical": b64_encode(&s.canonical),
        "entry_hash": hex::encode(s.entry_hash),
        "sig": b64_encode(&s.sig),
    })
}

/// The signed head naming the chain's last event (`seq` 0 and [`ZERO_HASH`]
/// for a KB with none), as its export line.
pub fn head_line(
    kb_id: &str,
    seq: u64,
    entry_hash: &str,
    signed_at: &str,
    signer: &Identity,
) -> Result<Value, CanonError> {
    let canonical = canonical(&json!({
        "format": HEAD_FORMAT,
        "kb_id": kb_id,
        "seq": seq,
        "entry_hash": entry_hash,
        "signed_at": signed_at,
    }))?;
    let sig = signer.sign(&[HEAD_SIG_DOMAIN, &canonical].concat());
    Ok(json!({
        "type": "head",
        "canonical": b64_encode(&canonical),
        "sig": b64_encode(&sig),
    }))
}

/// What a verified export showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub kb_id: String,
    pub events: u64,
    /// The head's `entry_hash` (the last event's, or [`ZERO_HASH`]).
    pub head_entry_hash: String,
    /// The kept head's `seq`, when `--against` was given and matched.
    pub against_seq: Option<u64>,
}

/// Why an export did not verify: the 1-based line, and what failed there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    pub line: usize,
    pub reason: String,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.reason)
    }
}

fn fail<T>(line: usize, reason: impl Into<String>) -> Result<T, VerifyError> {
    Err(VerifyError {
        line,
        reason: reason.into(),
    })
}

fn field<'a>(v: &'a Value, name: &str, line: usize) -> Result<&'a Value, VerifyError> {
    v.get(name).ok_or_else(|| VerifyError {
        line,
        reason: format!("no '{name}'"),
    })
}

fn str_field<'a>(v: &'a Value, name: &str, line: usize) -> Result<&'a str, VerifyError> {
    field(v, name, line)?.as_str().ok_or_else(|| VerifyError {
        line,
        reason: format!("'{name}' is not a string"),
    })
}

fn u64_field(v: &Value, name: &str, line: usize) -> Result<u64, VerifyError> {
    field(v, name, line)?.as_u64().ok_or_else(|| VerifyError {
        line,
        reason: format!("'{name}' is not a non-negative integer"),
    })
}

fn b64_field(v: &Value, name: &str, line: usize) -> Result<Vec<u8>, VerifyError> {
    b64_decode(str_field(v, name, line)?).map_err(|_| VerifyError {
        line,
        reason: format!("'{name}' is not standard base64"),
    })
}

/// A signed head as it was read: the fields it names.
struct Head {
    kb_id: String,
    seq: u64,
    entry_hash: String,
}

/// Check a head line's signature under `key` and read what it names.
fn read_head(line_v: &Value, key: &PublicKey, line: usize) -> Result<Head, VerifyError> {
    if line_v.get("type").and_then(Value::as_str) != Some("head") {
        return fail(line, "not a head line");
    }
    let canonical = b64_field(line_v, "canonical", line)?;
    let sig = b64_field(line_v, "sig", line)?;
    if !key.verify(&[HEAD_SIG_DOMAIN, &canonical].concat(), &sig) {
        return fail(
            line,
            "the head's signature does not verify under the given key",
        );
    }
    let head: Value = serde_json::from_slice(&canonical).map_err(|_| VerifyError {
        line,
        reason: "the head's canonical bytes are not JSON".into(),
    })?;
    if str_field(&head, "format", line)? != HEAD_FORMAT {
        return fail(line, "the head's format is not maereview-head/v1");
    }
    Ok(Head {
        kb_id: str_field(&head, "kb_id", line)?.to_string(),
        seq: u64_field(&head, "seq", line)?,
        entry_hash: str_field(&head, "entry_hash", line)?.to_string(),
    })
}

/// Check one event line against the previous `entry_hash`: E10's steps 1 to 5.
/// Returns the line's `entry_hash`.
fn check_event(
    v: &Value,
    kb_id: &str,
    expect_seq: u64,
    prev: &str,
    key: &PublicKey,
    line: usize,
) -> Result<String, VerifyError> {
    if v.get("type").and_then(Value::as_str) != Some("event") {
        return fail(line, "expected an event line");
    }
    let canonical = b64_field(v, "canonical", line)?;
    let event: Value = serde_json::from_slice(&canonical).map_err(|_| VerifyError {
        line,
        reason: "the canonical bytes are not JSON".into(),
    })?;
    for (what, seq) in [
        ("line", u64_field(v, "seq", line)?),
        ("event", u64_field(&event, "seq", line)?),
    ] {
        if seq != expect_seq {
            return fail(
                line,
                format!("the {what}'s seq is {seq}, expected {expect_seq}"),
            );
        }
    }
    for (what, p) in [
        ("line", str_field(v, "prev_hash", line)?),
        ("event", str_field(&event, "prev_hash", line)?),
    ] {
        if p != prev {
            return fail(
                line,
                format!("the {what}'s prev_hash is not the previous entry_hash"),
            );
        }
    }
    if str_field(&event, "kb_id", line)? != kb_id {
        return fail(line, "the event names another KB");
    }
    let recomputed = hex::encode(entry_hash(&canonical));
    if str_field(v, "entry_hash", line)? != recomputed {
        return fail(line, "entry_hash does not match the canonical bytes");
    }
    let sig = b64_field(v, "sig", line)?;
    let raw: [u8; 32] = hex::decode(&recomputed)
        .ok()
        .and_then(|b| b.try_into().ok())
        .expect("a hash we just encoded");
    if !key.verify(&sig_message(&raw), &sig) {
        return fail(
            line,
            "the event's signature does not verify under the given key",
        );
    }
    Ok(recomputed)
}

/// Verify a JSON Lines export (E10's steps) under `key`, and, with `against`,
/// that it extends a head kept elsewhere. `key` and `against` come from
/// outside the export; its header's `daemon_key` is never used.
pub fn verify_export(
    text: &str,
    key: &[u8; 32],
    against: Option<&Value>,
) -> Result<Verified, VerifyError> {
    let key = PublicKey::from_bytes(key, None).ok_or(VerifyError {
        line: 0,
        reason: "the given key is not an Ed25519 public key".into(),
    })?;
    let lines: Vec<Value> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|_| VerifyError {
                line: i + 1,
                reason: "not JSON".into(),
            })
        })
        .collect::<Result<_, _>>()?;
    let Some((header, rest)) = lines.split_first() else {
        return fail(1, "empty export");
    };
    if header.get("type").and_then(Value::as_str) != Some("header")
        || header.get("format").and_then(Value::as_str) != Some(EXPORT_FORMAT)
    {
        return fail(1, "the first line is not a maereview-export/v1 header");
    }
    let kb_id = str_field(header, "kb_id", 1)?.to_string();
    // The header's key is a claim, never what verifies; but an export claiming
    // another key than the one the verifier trusts is not this daemon's.
    if parse_key(str_field(header, "daemon_key", 1)?) != Some(key.to_bytes()) {
        return fail(
            1,
            "the export claims a different daemon key than the one given",
        );
    }
    let Some((head_v, events)) = rest.split_last() else {
        return fail(2, "no signed head");
    };
    let mut prev = ZERO_HASH.to_string();
    // The entry_hash at each seq, for `against`.
    let mut hashes = Vec::with_capacity(events.len());
    for (i, ev) in events.iter().enumerate() {
        prev = check_event(ev, &kb_id, i as u64 + 1, &prev, &key, i + 2)?;
        hashes.push(prev.clone());
    }
    let head_line_no = events.len() + 2;
    let head = read_head(head_v, &key, head_line_no)?;
    if head.kb_id != kb_id {
        return fail(head_line_no, "the head names another KB");
    }
    if head.seq != events.len() as u64 || head.entry_hash != prev {
        return fail(head_line_no, "the head does not name the last event");
    }
    let against_seq = match against {
        None => None,
        Some(kept) => {
            let k = read_head(kept, &key, 0)?;
            let matches = k.kb_id == kb_id
                && match k.seq {
                    0 => k.entry_hash == ZERO_HASH,
                    n => hashes.get(n as usize - 1) == Some(&k.entry_hash),
                };
            if !matches {
                return fail(0, "the export does not extend the kept head");
            }
            Some(k.seq)
        }
    };
    Ok(Verified {
        kb_id,
        events: events.len() as u64,
        head_entry_hash: prev,
        against_seq,
    })
}

/// Decode a base64 Ed25519 public key (the `--key` the verifier is given).
pub fn parse_key(b64: &str) -> Option<[u8; 32]> {
    b64_decode(b64.trim()).ok()?.try_into().ok()
}

#[cfg(test)]
#[path = "chain_tests.rs"]
mod tests;
