//! The `attachments` map of a node (ADR-115 D4): each attachment NAME, as the
//! node's text writes it in `[[attachment:NAME]]`, bound to the blob the KB's
//! store holds for it.
//!
//! ```text
//! root "attachments": YMap {
//!     NAME: { sha256: String, size: Number, media_type: String }
//! }
//! ```
//!
//! **A root type of the node document, not a key of the `node` map.** ADR-093
//! D4 forbids creating a nested container lazily (two peers would each insert
//! their own map at the key and one would be dropped), and D3 forbids writing
//! one on read. Every node that exists today has no such container, so a
//! nested map could only be created lazily or by a backfill. A root type has
//! no creating operation at all: every peer's writes land in the same map
//! whether or not the document had one, and a document without it reads as
//! empty. Reading never creates it (`txn.get_map`, not `get_or_insert_map`).
//!
//! Each entry is one plain value, replaced whole: a rename is a remove and an
//! insert, a new image under the same name is a new value, and two writers of
//! one name resolve by the map's own last-writer-wins, identically on every
//! peer. Different names merge.
//!
//! The reader is tolerant: an entry with a bad name or a bad shape is reported
//! and left out, never repaired; an entry with fields this version does not
//! know is read for the fields it does and the rest reported, so a newer
//! client's additive field does not hide an image from an older one.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use yrs::{Any, Map, Out, ReadTxn, Transact};

use super::KbNodeDoc;

/// The root map's name in a node document.
pub const ATTACHMENTS_ROOT: &str = "attachments";
/// Longest NAME, in UTF-8 bytes: `NAME_MAX` on Linux and macOS, because an
/// export writes the attachment as a file of that name (ADR-115 E2).
pub const MAX_ATTACHMENT_NAME_BYTES: usize = 255;
/// Largest `size`: 2^53 - 1, the largest integer every peer holds exactly
/// (a browser peer's numbers are IEEE doubles).
pub const MAX_ATTACHMENT_SIZE: u64 = (1 << 53) - 1;
/// Longest `type` or `subtype` of a media type (RFC 6838 §4.2).
const MAX_MEDIA_NAME: usize = 127;

const SHA256_KEY: &str = "sha256";
const SIZE_KEY: &str = "size";
const MEDIA_TYPE_KEY: &str = "media_type";

/// What a node binds an attachment NAME to: the blob's address in its KB's
/// store and what the store said about it when it was uploaded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AttachmentRef {
    /// Lowercase hex SHA-256 of the stored bytes (64 characters).
    pub sha256: String,
    /// Size of the stored bytes.
    pub size: u64,
    /// The media type the store sniffed, lowercase `type/subtype`.
    pub media_type: String,
}

/// Why a NAME or an entry is not acceptable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentError {
    /// The NAME is not one plain file name (see [`validate_attachment_name`]).
    Name(&'static str),
    /// `sha256` is missing, not a string, or not 64 lowercase hex digits.
    Sha256,
    /// `size` is missing, not an integer, negative, or over [`MAX_ATTACHMENT_SIZE`].
    Size,
    /// `media_type` is missing or not a lowercase `type/subtype`.
    MediaType,
    /// The entry is not a map at all.
    NotAMap,
    /// A rename's source has no valid entry.
    NoSuchAttachment,
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name(why) => write!(f, "attachment name {why}"),
            Self::Sha256 => f.write_str("sha256 is not 64 lowercase hex digits"),
            Self::Size => write!(f, "size is not an integer from 0 to {MAX_ATTACHMENT_SIZE}"),
            Self::MediaType => f.write_str("media_type is not a lowercase type/subtype"),
            Self::NotAMap => f.write_str("entry is not a map"),
            Self::NoSuchAttachment => f.write_str("no such attachment"),
        }
    }
}

impl std::error::Error for AttachmentError {}

/// Whether `name` is one plain file name, as `[[attachment:NAME]]` names a
/// file in the node's attachment directory (ADR-115 E2):
/// - not empty, and at most [`MAX_ATTACHMENT_NAME_BYTES`] bytes of UTF-8;
/// - not `.` or `..`;
/// - no `/`, and no `\` (a separator on Windows, where an export may run);
/// - no control character (Unicode `Cc`: C0, DEL and C1, NUL among them).
///
/// `..` inside a longer name (`v1..2.png`) is allowed: with no separator it
/// cannot leave the directory.
pub fn validate_attachment_name(name: &str) -> Result<(), AttachmentError> {
    if name.is_empty() {
        return Err(AttachmentError::Name("is empty"));
    }
    if name.len() > MAX_ATTACHMENT_NAME_BYTES {
        return Err(AttachmentError::Name("is longer than 255 bytes"));
    }
    if name == "." || name == ".." {
        return Err(AttachmentError::Name("is a directory reference"));
    }
    if name.contains(['/', '\\']) {
        return Err(AttachmentError::Name("contains a path separator"));
    }
    if name.chars().any(char::is_control) {
        return Err(AttachmentError::Name("contains a control character"));
    }
    Ok(())
}

fn valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// RFC 6838 `restricted-name`, lowercase: an alphanumeric first character,
/// then alphanumerics and `!#$&-^_.+`, at most 127 in all.
fn valid_media_name(s: &str) -> bool {
    let mut bytes = s.bytes();
    let first_ok = bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    first_ok
        && s.len() <= MAX_MEDIA_NAME
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"!#$&-^_.+".contains(&b))
}

fn valid_media_type(s: &str) -> bool {
    s.split_once('/')
        .is_some_and(|(t, sub)| valid_media_name(t) && valid_media_name(sub))
}

impl AttachmentRef {
    /// An entry, checked.
    pub fn new(
        sha256: impl Into<String>,
        size: u64,
        media_type: impl Into<String>,
    ) -> Result<Self, AttachmentError> {
        let r = Self {
            sha256: sha256.into(),
            size,
            media_type: media_type.into(),
        };
        r.validate()?;
        Ok(r)
    }

    /// Whether every field has the shape the store writes.
    pub fn validate(&self) -> Result<(), AttachmentError> {
        if !valid_sha256(&self.sha256) {
            return Err(AttachmentError::Sha256);
        }
        if self.size > MAX_ATTACHMENT_SIZE {
            return Err(AttachmentError::Size);
        }
        if !valid_media_type(&self.media_type) {
            return Err(AttachmentError::MediaType);
        }
        Ok(())
    }

    fn to_any(&self) -> Any {
        let mut m = HashMap::new();
        m.insert(SHA256_KEY.to_string(), Any::from(self.sha256.as_str()));
        // Exact: `validate` bounds it at 2^53 - 1.
        m.insert(SIZE_KEY.to_string(), Any::Number(self.size as f64));
        m.insert(
            MEDIA_TYPE_KEY.to_string(),
            Any::from(self.media_type.as_str()),
        );
        Any::Map(Arc::new(m))
    }
}

/// A non-negative integer of at most [`MAX_ATTACHMENT_SIZE`], however the
/// writer's runtime encoded it.
fn read_size(v: &Any) -> Option<u64> {
    let n = match v {
        Any::Number(f) if f.is_finite() && f.fract() == 0.0 && *f >= 0.0 => *f,
        Any::BigInt(i) if *i >= 0 => *i as f64,
        _ => return None,
    };
    (n <= MAX_ATTACHMENT_SIZE as f64).then_some(n as u64)
}

/// One stored entry, read: the entry and the names of any fields it carries
/// that this version does not know, or why it is not an entry.
fn read_entry(name: &str, value: &Out) -> Result<(AttachmentRef, Vec<String>), AttachmentError> {
    validate_attachment_name(name)?;
    let Out::Any(Any::Map(m)) = value else {
        return Err(AttachmentError::NotAMap);
    };
    let string = |k: &str| match m.get(k) {
        Some(Any::String(s)) => Some(s.to_string()),
        _ => None,
    };
    let r = AttachmentRef {
        sha256: string(SHA256_KEY).ok_or(AttachmentError::Sha256)?,
        size: m
            .get(SIZE_KEY)
            .and_then(read_size)
            .ok_or(AttachmentError::Size)?,
        media_type: string(MEDIA_TYPE_KEY).ok_or(AttachmentError::MediaType)?,
    };
    r.validate()?;
    let mut unknown: Vec<String> = m
        .keys()
        .filter(|k| ![SHA256_KEY, SIZE_KEY, MEDIA_TYPE_KEY].contains(&k.as_str()))
        .cloned()
        .collect();
    unknown.sort();
    Ok((r, unknown))
}

/// Something the reader noticed about one stored entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentProblem {
    /// The entry was left out: its NAME or its shape is not acceptable.
    Rejected {
        name: String,
        error: AttachmentError,
    },
    /// The entry was read, but carries fields this version does not know.
    UnknownFields { name: String, fields: Vec<String> },
}

impl std::fmt::Display for AttachmentProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected { name, error } => write!(f, "attachment {name:?} ignored: {error}"),
            Self::UnknownFields { name, fields } => {
                write!(f, "attachment {name:?} has unknown fields {fields:?}")
            }
        }
    }
}

/// The map as a reader sees it, with what was left out and why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachmentsRead {
    pub entries: BTreeMap<String, AttachmentRef>,
    pub problems: Vec<AttachmentProblem>,
}

impl KbNodeDoc {
    /// Every acceptable entry, with a report of what was not. Writes nothing.
    pub fn read_attachments(&self) -> AttachmentsRead {
        let txn = self.doc.transact();
        let mut out = AttachmentsRead::default();
        let Some(map) = txn.get_map(ATTACHMENTS_ROOT) else {
            return out;
        };
        for (name, value) in map.iter(&txn) {
            match read_entry(name, &value) {
                Ok((r, unknown)) => {
                    if !unknown.is_empty() {
                        out.problems.push(AttachmentProblem::UnknownFields {
                            name: name.to_string(),
                            fields: unknown,
                        });
                    }
                    out.entries.insert(name.to_string(), r);
                }
                Err(error) => out.problems.push(AttachmentProblem::Rejected {
                    name: name.to_string(),
                    error,
                }),
            }
        }
        out.problems
            .sort_by(|a, b| problem_name(a).cmp(problem_name(b)));
        out
    }

    /// The acceptable entries, by NAME. Empty for a document without the map.
    pub fn attachments(&self) -> BTreeMap<String, AttachmentRef> {
        self.read_attachments().entries
    }

    /// The entry for `name`, if it has an acceptable one.
    pub fn attachment(&self, name: &str) -> Option<AttachmentRef> {
        let txn = self.doc.transact();
        let value = txn.get_map(ATTACHMENTS_ROOT)?.get(&txn, name)?;
        read_entry(name, &value).ok().map(|(r, _)| r)
    }

    /// Bind `name` to `attachment`, replacing what it was bound to. Writes
    /// nothing when a reader already sees exactly that entry.
    #[must_use = "dropping this update silently prevents the attachment from syncing to peers"]
    pub fn set_attachment(
        &mut self,
        name: &str,
        attachment: &AttachmentRef,
    ) -> Result<Vec<u8>, AttachmentError> {
        validate_attachment_name(name)?;
        attachment.validate()?;
        if self.attachment(name).as_ref() == Some(attachment) {
            return Ok(self.doc.transact_mut().encode_update_v1());
        }
        let map = self.doc.get_or_insert_map(ATTACHMENTS_ROOT);
        let mut txn = self.doc.transact_mut();
        map.insert(&mut txn, name, attachment.to_any());
        Ok(txn.encode_update_v1())
    }

    /// Remove whatever `name` holds, acceptable or not (so a malformed entry
    /// can be cleared by name). Writes nothing when it holds nothing.
    #[must_use = "dropping this update silently prevents the removal from syncing to peers"]
    pub fn remove_attachment(&mut self, name: &str) -> Vec<u8> {
        let present = {
            let txn = self.doc.transact();
            txn.get_map(ATTACHMENTS_ROOT)
                .is_some_and(|m| m.contains_key(&txn, name))
        };
        if !present {
            return self.doc.transact_mut().encode_update_v1();
        }
        let map = self.doc.get_or_insert_map(ATTACHMENTS_ROOT);
        let mut txn = self.doc.transact_mut();
        map.remove(&mut txn, name);
        txn.encode_update_v1()
    }

    /// Rename `from` to `to` in one transaction: the same entry, under a new
    /// NAME. Replaces whatever `to` held. Writes nothing when the two are equal.
    #[must_use = "dropping this update silently prevents the rename from syncing to peers"]
    pub fn rename_attachment(&mut self, from: &str, to: &str) -> Result<Vec<u8>, AttachmentError> {
        validate_attachment_name(to)?;
        let entry = self
            .attachment(from)
            .ok_or(AttachmentError::NoSuchAttachment)?;
        if from == to {
            return Ok(self.doc.transact_mut().encode_update_v1());
        }
        let map = self.doc.get_or_insert_map(ATTACHMENTS_ROOT);
        let mut txn = self.doc.transact_mut();
        map.remove(&mut txn, from);
        map.insert(&mut txn, to, entry.to_any());
        Ok(txn.encode_update_v1())
    }

    /// Whether the map holds an entry a writer must not introduce: a bad name
    /// or a bad shape (unknown extra fields are fine, so a newer client's
    /// additive field never trips an older daemon). For
    /// `field_type_violations`.
    pub(super) fn has_rejected_attachment(&self) -> bool {
        self.read_attachments()
            .problems
            .iter()
            .any(|p| matches!(p, AttachmentProblem::Rejected { .. }))
    }
}

fn problem_name(p: &AttachmentProblem) -> &str {
    match p {
        AttachmentProblem::Rejected { name, .. }
        | AttachmentProblem::UnknownFields { name, .. } => name,
    }
}

/// The map as bytes for a digest: an empty map contributes nothing, so a
/// document without attachments hashes exactly as it did before the field
/// existed. Every value is length-prefixed and names are in order.
pub(super) fn digest_bytes(map: &BTreeMap<String, AttachmentRef>) -> Option<Vec<u8>> {
    if map.is_empty() {
        return None;
    }
    let mut out = b"|attachments|".to_vec();
    let mut put = |b: &[u8]| {
        out.extend((b.len() as u64).to_le_bytes());
        out.extend(b);
    };
    put(&(map.len() as u64).to_le_bytes());
    for (name, r) in map {
        put(name.as_bytes());
        put(r.sha256.as_bytes());
        put(&r.size.to_le_bytes());
        put(r.media_type.as_bytes());
    }
    Some(out)
}
