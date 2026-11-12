//! The KB's signed history head (D4). A head names each node's tip at the
//! moment it was signed: `(node_id, seq, entry_hash)`. Their digest is
//! [`kb_head`], and the daemon signs that digest with its Ed25519 key.
//!
//! A head is a prefix commitment, not a snapshot. Versions cut after it are
//! expected; what it rules out is a tip it names that is gone or different. A
//! removed last version, a rewritten entry and a whole re-sealed chain all
//! show as a tip that is no longer there.
//!
//! The honest limit (D4): whoever holds the daemon's key and its database can
//! sign a new head over a rewritten history. Only a head kept elsewhere,
//! compared with [`verify_against_head`], shows that.

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use super::chain::{kb_head, verify_chain, Broken, SealError, VerifyError};
use super::{Hash, HistoryEntry};

const SIG_DOMAIN: &[u8] = b"maehistoryhead-sig/v1";

/// One node's latest version, as a head names it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tip {
    pub node_id: String,
    pub seq: u64,
    pub entry_hash: Hash,
}

/// A KB's history head, signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedHead {
    pub kb_id: String,
    /// When it was signed, unix seconds.
    pub at: i64,
    /// Sorted by node.
    pub tips: Vec<Tip>,
    /// [`kb_head`] of the tips.
    pub digest: Hash,
    /// The signer's Ed25519 public key.
    pub signer: [u8; 32],
    pub signature: [u8; 64],
}

/// Why a head does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadError {
    /// The signature, or the digest of the tips it covers, does not verify.
    Signature,
    /// The head is another KB's.
    Kb,
    /// A node's chain fails on its own.
    Chain { node_id: String, err: VerifyError },
    /// A tip the head names is not in the node's chain as named: removed,
    /// rewritten, or the node's history gone.
    Tip { node_id: String, seq: u64 },
}

/// The Ed25519 public key of the secret seed `seed`: whose key a head must
/// carry to be this signer's.
pub fn public_key_of(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

impl SignedHead {
    /// Sign the tips of `kb_id` at `at` with the Ed25519 key whose secret
    /// seed is `seed`.
    pub fn sign(
        kb_id: &str,
        at: i64,
        mut tips: Vec<Tip>,
        seed: &[u8; 32],
    ) -> Result<Self, SealError> {
        tips.sort();
        let digest = kb_head(
            kb_id,
            tips.iter()
                .map(|t| (t.node_id.as_str(), t.seq, t.entry_hash)),
        )?;
        let key = SigningKey::from_bytes(seed);
        let mut head = SignedHead {
            kb_id: kb_id.to_string(),
            at,
            tips,
            digest,
            signer: key.verifying_key().to_bytes(),
            signature: [0; 64],
        };
        head.signature = key.sign(&head.message()).to_bytes();
        Ok(head)
    }

    fn message(&self) -> Vec<u8> {
        let mut m = SIG_DOMAIN.to_vec();
        for part in [self.kb_id.as_bytes(), &self.at.to_le_bytes(), &self.digest] {
            m.extend((part.len() as u64).to_le_bytes());
            m.extend(part);
        }
        m
    }

    /// Whether the tips hash to `digest` and the signature over it verifies
    /// under `signer`. Whether `signer` is a key to trust is the caller's
    /// question: a daemon checks its own, an operator the one they recorded.
    pub fn verify_signature(&self) -> bool {
        let tips = self
            .tips
            .iter()
            .map(|t| (t.node_id.as_str(), t.seq, t.entry_hash));
        let Ok(digest) = kb_head(&self.kb_id, tips) else {
            return false;
        };
        let Ok(key) = VerifyingKey::from_bytes(&self.signer) else {
            return false;
        };
        digest == self.digest
            && key
                .verify(&self.message(), &Signature::from_bytes(&self.signature))
                .is_ok()
    }
}

/// Checks `chains` (each node's whole history, in `seq` order) against
/// `head`: the signature, every chain on its own, and every tip the head names
/// still in its chain with the same entry hash.
pub fn verify_against_head(
    head: &SignedHead,
    kb_id: &str,
    chains: &BTreeMap<String, Vec<HistoryEntry>>,
) -> Result<(), HeadError> {
    if head.kb_id != kb_id {
        return Err(HeadError::Kb);
    }
    if !head.verify_signature() {
        return Err(HeadError::Signature);
    }
    for (node_id, chain) in chains {
        verify_chain(kb_id, node_id, chain, None).map_err(|err| HeadError::Chain {
            node_id: node_id.clone(),
            err,
        })?;
    }
    for tip in &head.tips {
        let held = chains
            .get(&tip.node_id)
            .and_then(|c| c.iter().find(|e| e.seq == tip.seq))
            .is_some_and(|e| e.entry_hash == tip.entry_hash);
        if !held {
            return Err(HeadError::Tip {
                node_id: tip.node_id.clone(),
                seq: tip.seq,
            });
        }
    }
    Ok(())
}

impl HeadError {
    /// The chain-check form of the failure, for reports shared with
    /// [`verify_chain`]'s.
    pub fn broken(&self) -> Broken {
        match self {
            Self::Chain { err, .. } => err.broken,
            _ => Broken::Head,
        }
    }
}
