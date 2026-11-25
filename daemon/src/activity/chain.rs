//! The chain's format (ADR-120 D7): the body enters the chain only through
//! its hash, so an export can omit an event's content and still verify.
//!
//! ```text
//! body_hash  = SHA-256("maeactivity-body/v1\n" ‖ JCS(body))
//! link       = {v: 1, chain, seq, gseq, prev_hash, body_hash}
//! entry_hash = SHA-256("maeactivity/v1\n" ‖ JCS(link))
//! ```
//!
//! JCS is ADR-113 E10's restricted encoding (`review::chain::canonical`),
//! reused rather than copied (principle #8). The body's 16-byte `salt` makes
//! `body_hash` hiding: a stub's hash confirms no guess at a low-entropy body.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::event::{Chain, Columns, Event};
use crate::review::chain::{canonical, CanonError};

/// The first event's `prev_hash` in each chain.
pub const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const BODY_DOMAIN: &[u8] = b"maeactivity-body/v1\n";
const ENTRY_DOMAIN: &[u8] = b"maeactivity/v1\n";

fn domain_hash(domain: &[u8], bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(domain);
    h.update(bytes);
    hex::encode(h.finalize())
}

/// `body_hash` of a body's canonical bytes.
pub fn body_hash(body_canonical: &[u8]) -> String {
    domain_hash(BODY_DOMAIN, body_canonical)
}

/// `entry_hash` of a link's canonical bytes.
pub fn entry_hash(link_canonical: &[u8]) -> String {
    domain_hash(ENTRY_DOMAIN, link_canonical)
}

/// A link's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub chain: Chain,
    pub seq: u64,
    pub gseq: u64,
    pub prev_hash: String,
    pub body_hash: String,
}

impl Link {
    pub fn to_value(&self) -> Value {
        json!({
            "v": 1,
            "chain": self.chain.as_str(),
            "seq": self.seq,
            "gseq": self.gseq,
            "prev_hash": self.prev_hash,
            "body_hash": self.body_hash,
        })
    }

    /// The link `bytes` encode, only if they are exactly its canonical bytes
    /// and hold exactly its six fields: so a flipped byte anywhere in a
    /// stored link either fails here or changes a value something checks.
    pub fn parse(bytes: &[u8]) -> Result<Link, String> {
        let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("not JSON: {e}"))?;
        if canonical(&v).map_err(|e| e.to_string())? != bytes {
            return Err("not in canonical form".into());
        }
        let obj = v.as_object().ok_or("not an object")?;
        if obj.len() != 6 || v["v"] != json!(1) {
            return Err("not a v1 link".into());
        }
        let hex64 = |k: &str| -> Result<String, String> {
            let s = v[k].as_str().ok_or(format!("{k} is not a string"))?;
            let ok = s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
            ok.then(|| s.to_string())
                .ok_or(format!("{k} is not 64 lowercase hex digits"))
        };
        let num = |k: &str| v[k].as_u64().filter(|n| *n >= 1).ok_or(format!("bad {k}"));
        Ok(Link {
            chain: v["chain"]
                .as_str()
                .and_then(Chain::parse)
                .ok_or("bad chain")?,
            seq: num("seq")?,
            gseq: num("gseq")?,
            prev_hash: hex64("prev_hash")?,
            body_hash: hex64("body_hash")?,
        })
    }
}

/// An event sealed into its chain position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub link: Link,
    pub link_bytes: Vec<u8>,
    pub body_bytes: Vec<u8>,
    pub entry_hash: String,
    pub columns: Columns,
}

/// Encode `event` with `salt`, and link it after `prev_hash` at `(seq, gseq)`.
pub fn seal(
    event: &Event,
    salt: &[u8; 16],
    seq: u64,
    gseq: u64,
    prev_hash: &str,
) -> Result<Sealed, CanonError> {
    let body = event.body(salt);
    let body_bytes = canonical(&body)?;
    let link = Link {
        chain: event.chain(),
        seq,
        gseq,
        prev_hash: prev_hash.to_string(),
        body_hash: body_hash(&body_bytes),
    };
    let link_bytes = canonical(&link.to_value())?;
    let columns = Columns::of(&body).expect("a body this module built has every column");
    Ok(Sealed {
        entry_hash: entry_hash(&link_bytes),
        link,
        link_bytes,
        body_bytes,
        columns,
    })
}

/// 16 random bytes for a body's salt.
pub fn fresh_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut salt);
    salt
}
