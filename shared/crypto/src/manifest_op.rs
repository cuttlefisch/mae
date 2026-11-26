//! A signed change to a KB's node list (ADR-036 Amendment 2, G1): adding or
//! removing a node of the collection document, signed by the author's member
//! key like a content op, under its own domain tag so neither signature can
//! stand for the other.
//!
//! @ai-caution: [security] `canonical_bytes` is a wire format: every signed
//! manifest op ever written verifies against it. A change is a new tag
//! (`maemanifest/v2`), never an edit of this one.
//! `tests::the_canonical_bytes_and_signature_are_pinned` holds the vector.

use crate::member::{fingerprint, MemberKey};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// The domain tag that opens [`ManifestOp::canonical_bytes`], distinct from
/// `maecontent/v1` and `mae-identity-binding-v1`.
pub const MANIFEST_OP_DOMAIN: &str = "maemanifest/v1";

/// What the op does to the node list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestAction {
    Add,
    Remove,
}

impl ManifestAction {
    pub fn as_str(self) -> &'static str {
        match self {
            ManifestAction::Add => "add",
            ManifestAction::Remove => "remove",
        }
    }
}

/// A signed node-list change's header. The daemon builds the collection update
/// itself from these fields; the signature binds who asked for which change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestOp {
    pub kb_id: String,
    pub node_id: String,
    pub action: ManifestAction,
    /// The node's title in the list; empty for a removal.
    pub title: String,
    /// The fingerprint of the signing key.
    pub author: String,
    /// The author's ADR-023 epoch in the KB.
    pub epoch: u64,
    /// Unix seconds: audit, not a security boundary.
    pub issued_at: u64,
}

/// Why a header cannot be encoded unambiguously.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestHeaderError {
    EmptyField(&'static str),
    NulInField(&'static str),
    /// A removal names a title.
    TitleOnRemove,
}

impl std::fmt::Display for ManifestHeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestHeaderError::EmptyField(k) => write!(f, "{k} is empty"),
            ManifestHeaderError::NulInField(k) => write!(f, "{k} contains a NUL byte"),
            ManifestHeaderError::TitleOnRemove => f.write_str("a removal carries no title"),
        }
    }
}

impl std::error::Error for ManifestHeaderError {}

impl ManifestOp {
    /// The exact bytes signed and verified: the domain tag and every field,
    /// each NUL-terminated. NUL is refused in every text field, so the
    /// separation is unambiguous.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        for s in [
            MANIFEST_OP_DOMAIN,
            &self.kb_id,
            &self.node_id,
            self.action.as_str(),
            &self.title,
            &self.author,
            &self.epoch.to_string(),
            &self.issued_at.to_string(),
        ] {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        b
    }

    /// Whether every field can be encoded unambiguously; [`verify`](Self::verify)
    /// refuses a header that fails it, whatever its signature.
    pub fn check(&self) -> Result<(), ManifestHeaderError> {
        for (name, v) in [("kb_id", &self.kb_id), ("node_id", &self.node_id)] {
            if v.is_empty() {
                return Err(ManifestHeaderError::EmptyField(name));
            }
        }
        for (name, v) in [
            ("kb_id", &self.kb_id),
            ("node_id", &self.node_id),
            ("title", &self.title),
            ("author", &self.author),
        ] {
            if v.as_bytes().contains(&0) {
                return Err(ManifestHeaderError::NulInField(name));
            }
        }
        if self.action == ManifestAction::Remove && !self.title.is_empty() {
            return Err(ManifestHeaderError::TitleOnRemove);
        }
        Ok(())
    }

    /// A header authored by `key`: the author is the key's own fingerprint.
    pub fn for_key(
        key: &MemberKey,
        kb_id: &str,
        node_id: &str,
        action: ManifestAction,
        title: &str,
        epoch: u64,
        issued_at: u64,
    ) -> Self {
        ManifestOp {
            kb_id: kb_id.to_string(),
            node_id: node_id.to_string(),
            action,
            title: title.to_string(),
            author: key.fingerprint(),
            epoch,
            issued_at,
        }
    }

    /// Sign with a held member key.
    pub fn sign_with(&self, key: &MemberKey) -> [u8; 64] {
        key.sign(&self.canonical_bytes())
    }

    /// Sign with the author's Ed25519 seed (the editor path).
    pub fn sign(&self, secret: &[u8; 32]) -> Vec<u8> {
        SigningKey::from_bytes(secret)
            .sign(&self.canonical_bytes())
            .to_bytes()
            .to_vec()
    }

    /// Whether `sig` is `author_pubkey`'s signature over this header, and that
    /// key's fingerprint is the author. An unencodable header never verifies.
    pub fn verify(&self, sig: &[u8], author_pubkey: &[u8; 32]) -> bool {
        if self.check().is_err() || fingerprint(author_pubkey) != self.author {
            return false;
        }
        let Ok(vk) = VerifyingKey::from_bytes(author_pubkey) else {
            return false;
        };
        let Ok(arr) = <[u8; 64]>::try_from(sig) else {
            return false;
        };
        vk.verify(&self.canonical_bytes(), &Signature::from_bytes(&arr))
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_op::ContentOp;

    fn key() -> MemberKey {
        MemberKey::from_seed(&[0x5a; 32])
    }

    fn op(k: &MemberKey) -> ManifestOp {
        ManifestOp::for_key(
            k,
            "kb-alpha",
            "concept:buffer",
            ManifestAction::Add,
            "Buffer",
            7,
            1_790_000_000,
        )
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// The vector every signer and the daemon share.
    #[test]
    fn the_canonical_bytes_and_signature_are_pinned() {
        let k = key();
        let o = op(&k);
        let mut want = b"maemanifest/v1\0kb-alpha\0concept:buffer\0add\0Buffer\0".to_vec();
        want.extend_from_slice(k.fingerprint().as_bytes());
        want.extend_from_slice(b"\x007\x001790000000\0");
        assert_eq!(o.canonical_bytes(), want);
        let sig = o.sign_with(&k);
        assert_eq!(sig.to_vec(), o.sign(&[0x5a; 32]));
        assert_eq!(hex(&sig), PINNED_SIG, "the pinned signature moved");
    }

    const PINNED_SIG: &str = "3aae6f6a56a95395ebe4e27dc93075c005115bb0647705d9e158ef9e016693864dbaae39413797cfea7a5bafae905cd06bdda2ea09ee7a96013b586a8c562a02";

    /// Any change to a field, another key, or a claimed author fails.
    #[test]
    fn a_signature_verifies_only_for_its_exact_op_and_key() {
        let k = key();
        let other = MemberKey::from_seed(&[0x11; 32]);
        let o = op(&k);
        let sig = o.sign_with(&k);
        assert!(o.verify(&sig, &k.public_key()));
        let changed = [
            ManifestOp {
                node_id: "concept:other".into(),
                ..o.clone()
            },
            ManifestOp {
                kb_id: "kb-beta".into(),
                ..o.clone()
            },
            ManifestOp {
                title: "Renamed".into(),
                ..o.clone()
            },
            ManifestOp {
                epoch: 8,
                ..o.clone()
            },
            ManifestOp {
                action: ManifestAction::Remove,
                title: String::new(),
                ..o.clone()
            },
        ];
        for c in changed {
            assert!(!c.verify(&sig, &k.public_key()), "{c:?}");
        }
        assert!(!o.verify(&sig, &other.public_key()), "another key");
        let claimed = ManifestOp {
            author: other.fingerprint(),
            ..o.clone()
        };
        assert!(!claimed.verify(&other.sign(&claimed.canonical_bytes()), &k.public_key()));
    }

    /// A content-op signature over the same key never verifies as a manifest
    /// op, and the reverse: the domain tags differ.
    #[test]
    fn content_and_manifest_signatures_never_stand_for_each_other() {
        let k = key();
        let m = op(&k);
        let c = ContentOp::for_key(&k, "kb-alpha", "concept:buffer", &[], 7, 1_790_000_000);
        let content_sig = c.sign_with(&k, b"");
        assert!(!m.verify(&content_sig, &k.public_key()));
        let manifest_sig = m.sign_with(&k);
        assert!(!c.verify(&manifest_sig, &k.public_key(), b""));
    }

    /// Ids the encoding cannot carry, and a titled removal, never verify.
    #[test]
    fn unencodable_headers_are_refused() {
        let k = key();
        for (o, want) in [
            (
                ManifestOp {
                    kb_id: String::new(),
                    ..op(&k)
                },
                ManifestHeaderError::EmptyField("kb_id"),
            ),
            (
                ManifestOp {
                    node_id: "a\0b".into(),
                    ..op(&k)
                },
                ManifestHeaderError::NulInField("node_id"),
            ),
            (
                ManifestOp {
                    title: "x\0y".into(),
                    ..op(&k)
                },
                ManifestHeaderError::NulInField("title"),
            ),
            (
                ManifestOp {
                    action: ManifestAction::Remove,
                    ..op(&k)
                },
                ManifestHeaderError::TitleOnRemove,
            ),
        ] {
            assert_eq!(o.check(), Err(want));
            let sig = o.sign_with(&k);
            assert!(!o.verify(&sig, &k.public_key()));
        }
    }
}
