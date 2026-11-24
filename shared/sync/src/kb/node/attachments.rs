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
//! one name resolve by the map's own last-writer-wins. "Last" is yrs's
//! deterministic order of the two writes by client id, not wall time, so every
//! peer picks the same winner. Different names merge.
//!
//! A NAME is one portable file name ([`validate_attachment_name`]), and no two
//! NAMEs of one node may be one file on a case- or normalization-insensitive
//! filesystem: a writer refuses the second ([`AttachmentError::NameCollides`]).
//! Two peers can still create such a pair concurrently, each write valid on
//! its own; the reader keeps both, and an export must refuse or disambiguate
//! them (ADR-115 P4).
//!
//! There are two readers, failing in opposite directions:
//! - the display reader ([`KbNodeDoc::read_attachments`]) is tolerant: an
//!   entry with a bad name or a bad shape is reported and left out, never
//!   repaired; an entry with fields this version does not know is read for the
//!   fields it does and the rest reported, so a newer client's additive field
//!   does not hide an image from an older one;
//! - the sweep's reader ([`KbNodeDoc::referenced_hashes`]) fails safe: every
//!   string anywhere in any entry that is a SHA-256 in hex counts, whatever
//!   key holds it and however deep, so a garbage collector never deletes
//!   bytes a peer this version does not understand still binds; a document it
//!   cannot walk in full is reported unreadable, never read as empty.
//!
//! Rebirth copies the root's entries as stored, every key and value, and
//! never through the NAME rule: a copy that dropped what a reader rejects
//! would drop the references the sweep needs.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use yrs::types::text::{Diff, YChange};
use yrs::types::xml::{Xml, XmlFragment};
use yrs::types::{AsPrelim, Attrs};
use yrs::{Any, Array, GetString, Map, Out, ReadTxn, Text, Transact};

use super::KbNodeDoc;
use crate::portable_name::{
    fold_key, is_format_char, is_windows_device_name, nfd_len, WINDOWS_FORBIDDEN,
};

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
    /// Another NAME of the node is the same file once case and Unicode
    /// normalization are ignored ([`fold_key`]).
    NameCollides { existing: String },
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
            Self::NameCollides { existing } => write!(
                f,
                "attachment name is the same file as {existing:?} on a case-insensitive filesystem"
            ),
        }
    }
}

impl std::error::Error for AttachmentError {}

/// Whether `name` is one plain file name, as `[[attachment:NAME]]` names a
/// file in the node's attachment directory (ADR-115 E2), the same file on
/// Linux, macOS and Windows (principle #13), since an export writes it on any
/// of them:
/// - not empty, and at most [`MAX_ATTACHMENT_NAME_BYTES`] bytes of UTF-8,
///   both as written and in NFD, the form APFS stores (`é` is 2 bytes
///   composed and 3 decomposed, so 128 of them fit in NFC and not on APFS);
/// - not `.` or `..`;
/// - no `/`, and no `\` (a separator on Windows);
/// - no control character (Unicode `Cc`: C0, DEL and C1, NUL among them);
/// - no format character (Unicode `Cf`: bidi controls, zero-width
///   characters), which would make two different names look the same;
/// - none of `< > : " | ? *`, which Windows refuses;
/// - no trailing `.` or space, which Windows strips (`a.png.` would be
///   written as `a.png`);
/// - not a Windows device name, case-insensitively, with or without an
///   extension (`CON`, `nul.png`, `Com1.tar.gz`; see
///   [`is_windows_device_name`]).
///
/// `..` inside a longer name (`v1..2.png`) is allowed: with no separator it
/// cannot leave the directory.
///
/// Two NAMEs of one node that fold to one key are refused by the writers, not
/// here: that needs the node's other NAMEs.
pub fn validate_attachment_name(name: &str) -> Result<(), AttachmentError> {
    if name.is_empty() {
        return Err(AttachmentError::Name("is empty"));
    }
    if name.len() > MAX_ATTACHMENT_NAME_BYTES {
        return Err(AttachmentError::Name("is longer than 255 bytes"));
    }
    if nfd_len(name) > MAX_ATTACHMENT_NAME_BYTES {
        return Err(AttachmentError::Name(
            "is longer than 255 bytes decomposed (NFD), as APFS stores it",
        ));
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
    if name.chars().any(is_format_char) {
        return Err(AttachmentError::Name(
            "contains an invisible format character",
        ));
    }
    if name.contains(WINDOWS_FORBIDDEN) {
        return Err(AttachmentError::Name(
            "contains a character Windows refuses (< > : \" | ? *)",
        ));
    }
    if name.ends_with(['.', ' ']) {
        return Err(AttachmentError::Name("ends in a dot or a space"));
    }
    if is_windows_device_name(name) {
        return Err(AttachmentError::Name("is a Windows device name"));
    }
    Ok(())
}

fn valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Deepest nesting the sweep's reader walks below a NAME. Today's entry is a
/// map of three strings, depth 2; this leaves a newer peer's shape fifteen
/// times that, and it bounds the recursion a rebirth's deep copy
/// (`as_prelim`) does on a shared type to a depth no stack notices.
pub const MAX_REFERENCE_DEPTH: usize = 32;
/// Most values the sweep's reader walks in one document. Today's entry is
/// four values (the map and its three fields), so this is 25,000 such
/// entries, or a thousand entries of a hundred values each: no node an
/// honest writer makes. Past it, a hostile document costs the sweep one
/// bounded walk and the KB is not collected, instead of the walk running
/// as long as the document is big.
pub const MAX_REFERENCE_VALUES: usize = 100_000;

/// Why the sweep's reader could not vouch for every reference a document
/// makes, so a sweep must count it unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnwalkableAttachments {
    /// Nested deeper than [`MAX_REFERENCE_DEPTH`].
    TooDeep,
    /// More than [`MAX_REFERENCE_VALUES`] values.
    TooLarge,
    /// A value this reader cannot see into: a subdocument, a weak link, a
    /// shared type of unknown kind, or an XML node whose children do not all
    /// read.
    Opaque,
}

impl std::fmt::Display for UnwalkableAttachments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooDeep => "attachments nest deeper than the sweep reads",
            Self::TooLarge => "attachments hold more values than the sweep reads",
            Self::Opaque => "attachments hold a value the sweep cannot read",
        })
    }
}

impl std::error::Error for UnwalkableAttachments {}

/// One value still to read.
enum Pending {
    Out(Out),
    Any(Any),
}

/// The sweep's walk of a document's `attachments` root: iterative, so no
/// input reaches the stack, and bounded in depth and in values.
#[derive(Default)]
struct HashWalk {
    stack: Vec<(Pending, usize)>,
    values: usize,
    hashes: std::collections::BTreeSet<String>,
}

impl HashWalk {
    fn push(&mut self, item: Pending, depth: usize) -> Result<(), UnwalkableAttachments> {
        if depth > MAX_REFERENCE_DEPTH {
            return Err(UnwalkableAttachments::TooDeep);
        }
        self.values += 1;
        if self.values > MAX_REFERENCE_VALUES {
            return Err(UnwalkableAttachments::TooLarge);
        }
        self.stack.push((item, depth));
        Ok(())
    }

    /// Count `s` if it is a SHA-256 in hex once ASCII whitespace is trimmed,
    /// lowercased (the store's spelling; an uppercase one is counted rather
    /// than trusted to be meaningless).
    fn string(&mut self, s: &str) {
        let t = s.trim_matches(|c: char| c.is_ascii_whitespace());
        if t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
            self.hashes.insert(t.to_ascii_lowercase());
        }
    }

    fn attrs(&mut self, attrs: Option<&Attrs>, depth: usize) -> Result<(), UnwalkableAttachments> {
        for (k, v) in attrs.into_iter().flatten() {
            self.string(k);
            self.push(Pending::Any(v.clone()), depth + 1)?;
        }
        Ok(())
    }

    fn any(&mut self, v: Any, depth: usize) -> Result<(), UnwalkableAttachments> {
        match v {
            Any::String(s) => self.string(&s),
            Any::Buffer(b) => {
                // A digest stored as its 32 raw bytes, or as hex in bytes.
                if b.len() == 32 {
                    self.hashes.insert(hex::encode(&b[..]));
                } else if let Ok(s) = std::str::from_utf8(&b) {
                    self.string(s);
                }
            }
            Any::Array(a) => {
                for v in a.iter() {
                    self.push(Pending::Any(v.clone()), depth + 1)?;
                }
            }
            Any::Map(m) => {
                for (k, v) in m.iter() {
                    self.string(k);
                    self.push(Pending::Any(v.clone()), depth + 1)?;
                }
            }
            Any::Null | Any::Undefined | Any::Bool(_) | Any::Number(_) | Any::BigInt(_) => {}
        }
        Ok(())
    }

    fn text<T: ReadTxn, X: Text + GetString>(
        &mut self,
        txn: &T,
        t: &X,
        depth: usize,
    ) -> Result<(), UnwalkableAttachments> {
        self.string(&t.get_string(txn));
        // Embeds and formatting attributes, which the string leaves out.
        for Diff {
            insert, attributes, ..
        } in t.diff(txn, YChange::identity)
        {
            if !matches!(insert, Out::Any(Any::String(_))) {
                self.push(Pending::Out(insert), depth + 1)?;
            }
            self.attrs(attributes.as_deref(), depth)?;
        }
        Ok(())
    }

    fn xml_attributes<T: ReadTxn, X: Xml>(
        &mut self,
        txn: &T,
        x: &X,
        depth: usize,
    ) -> Result<(), UnwalkableAttachments> {
        for (k, v) in x.attributes(txn) {
            self.string(k);
            self.push(Pending::Out(v), depth + 1)?;
        }
        Ok(())
    }

    fn xml_children<T: ReadTxn, X: XmlFragment>(
        &mut self,
        txn: &T,
        x: &X,
        depth: usize,
    ) -> Result<(), UnwalkableAttachments> {
        // The iterator stops at the first child that is not an XML node,
        // so a short count means children it did not show.
        let mut seen = 0u32;
        for child in x.children(txn) {
            seen += 1;
            self.push(Pending::Out(child.into()), depth + 1)?;
        }
        if seen != x.len(txn) {
            return Err(UnwalkableAttachments::Opaque);
        }
        Ok(())
    }

    fn out<T: ReadTxn>(
        &mut self,
        txn: &T,
        v: Out,
        depth: usize,
    ) -> Result<(), UnwalkableAttachments> {
        match v {
            Out::Any(a) => self.any(a, depth)?,
            Out::YText(t) => self.text(txn, &t, depth)?,
            Out::YArray(a) => {
                for v in a.iter(txn) {
                    self.push(Pending::Out(v), depth + 1)?;
                }
            }
            Out::YMap(m) => {
                for (k, v) in m.iter(txn) {
                    self.string(k);
                    self.push(Pending::Out(v), depth + 1)?;
                }
            }
            Out::YXmlText(t) => {
                self.text(txn, &t, depth)?;
                self.xml_attributes(txn, &t, depth)?;
            }
            Out::YXmlElement(e) => {
                self.string(e.tag());
                self.xml_attributes(txn, &e, depth)?;
                self.xml_children(txn, &e, depth)?;
            }
            Out::YXmlFragment(f) => self.xml_children(txn, &f, depth)?,
            // A subdocument's content is not in this document, and an
            // untyped or weak reference has no shape to read by.
            _ => return Err(UnwalkableAttachments::Opaque),
        }
        Ok(())
    }

    /// Every hash in the root `map`, under any NAME, key or depth.
    fn root<T: ReadTxn>(
        txn: &T,
        map: &yrs::MapRef,
    ) -> Result<std::collections::BTreeSet<String>, UnwalkableAttachments> {
        let mut walk = Self::default();
        for (name, value) in map.iter(txn) {
            walk.string(name);
            walk.push(Pending::Out(value), 1)?;
        }
        while let Some((item, depth)) = walk.stack.pop() {
            match item {
                Pending::Out(v) => walk.out(txn, v, depth)?,
                Pending::Any(a) => walk.any(a, depth)?,
            }
        }
        Ok(walk.hashes)
    }
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

    /// Every blob hash the map binds, for a garbage collector (ADR-115 D5):
    /// the opposite of [`read_attachments`](Self::read_attachments). Every
    /// string in the root, at any depth and under any key or NAME, whatever
    /// its container (a plain value, a nested map or array, a `Y.Text`, an
    /// XML node), that is 64 hex digits once ASCII whitespace is trimmed,
    /// lowercased; and a 32-byte buffer, as its hex. An entry the display
    /// reader rejects (a bad NAME, a size stored as a string, a hash under
    /// `blob.sha256` or `digest`) counts all the same, because leaving it
    /// out would delete bytes a peer still binds. Keeping a blob nobody
    /// needed is the cost.
    ///
    /// `Err` when the walk passes [`MAX_REFERENCE_DEPTH`] or
    /// [`MAX_REFERENCE_VALUES`], or meets a value it cannot see into: the
    /// caller must then treat the document as unreadable.
    ///
    /// @ai-caution: [data-loss] Never filter this through the NAME rule or
    /// [`AttachmentRef::validate`], and never turn an `Err` into an empty
    /// set. A missed hash is a deleted image.
    pub fn referenced_hashes(
        &self,
    ) -> Result<std::collections::BTreeSet<String>, UnwalkableAttachments> {
        let txn = self.doc.transact();
        match txn.get_map(ATTACHMENTS_ROOT) {
            Some(map) => HashWalk::root(&txn, &map),
            None => Ok(Default::default()),
        }
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
        self.check_no_collision(name, name)?;
        if self.attachment(name).as_ref() == Some(attachment) {
            return Ok(self.doc.transact_mut().encode_update_v1());
        }
        let map = self.doc.get_or_insert_map(ATTACHMENTS_ROOT);
        let mut txn = self.doc.transact_mut();
        map.insert(&mut txn, name, attachment.to_any());
        Ok(txn.encode_update_v1())
    }

    /// Write a node's map, as a `Node` projection holds it, into a document
    /// being MINTED (a fresh lineage, which has no stored root to copy from).
    /// Every entry is written as held: no NAME rule, no shape check, and no
    /// collision rule, because the map is a copy of what peers already
    /// converged on, and that can hold a pair two peers bound concurrently
    /// that fold to one key. A copy that refused one would delete an image.
    ///
    /// @ai-caution: [data-loss] Only for copying an existing node map. A NEW
    /// NAME goes through `set_attachment`, which keeps the rules.
    #[must_use = "dropping this update silently prevents the attachments from syncing to peers"]
    pub fn copy_attachments(&mut self, entries: &BTreeMap<String, AttachmentRef>) -> Vec<u8> {
        if entries.is_empty() {
            return self.doc.transact_mut().encode_update_v1();
        }
        let map = self.doc.get_or_insert_map(ATTACHMENTS_ROOT);
        let mut txn = self.doc.transact_mut();
        for (name, attachment) in entries {
            map.insert(&mut txn, name.as_str(), attachment.to_any());
        }
        txn.encode_update_v1()
    }

    /// Copy `src`'s `attachments` root into this document RAW, for rebirth:
    /// every key, each value as stored (a plain value is the same value; a
    /// shared type a deep copy of it). Nothing is checked, so an entry a
    /// reader rejects (a legacy NAME, a malformed shape, a newer peer's
    /// fields) crosses with the reference the sweep needs.
    ///
    /// `Err`, writing nothing, when the sweep's reader cannot walk `src`'s
    /// root ([`referenced_hashes`](Self::referenced_hashes)): what cannot be
    /// read in full cannot be shown to be copied in full, and the bound keeps
    /// the deep copy's recursion shallow.
    ///
    /// @ai-caution: [data-loss] Never route this through `read_attachments`,
    /// `set_attachment` or the NAME rule: each drops what it rejects.
    #[must_use = "dropping this update silently prevents the attachments from syncing to peers"]
    pub fn copy_raw_attachments(
        &mut self,
        src: &KbNodeDoc,
    ) -> Result<Vec<u8>, UnwalkableAttachments> {
        src.referenced_hashes()?;
        let src_txn = src.doc.transact();
        let Some(src_map) = src_txn.get_map(ATTACHMENTS_ROOT) else {
            return Ok(self.doc.transact_mut().encode_update_v1());
        };
        let map = self.doc.get_or_insert_map(ATTACHMENTS_ROOT);
        let mut txn = self.doc.transact_mut();
        for (name, value) in src_map.iter(&src_txn) {
            match value {
                Out::Any(any) => {
                    map.insert(&mut txn, name, any);
                }
                shared => {
                    map.insert(&mut txn, name, shared.as_prelim(&src_txn));
                }
            }
        }
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
        // `from` goes away, so a case-only rename (`a.png` to `A.png`) is fine.
        self.check_no_collision(to, from)?;
        if from == to {
            return Ok(self.doc.transact_mut().encode_update_v1());
        }
        let map = self.doc.get_or_insert_map(ATTACHMENTS_ROOT);
        let mut txn = self.doc.transact_mut();
        map.remove(&mut txn, from);
        map.insert(&mut txn, to, entry.to_any());
        Ok(txn.encode_update_v1())
    }

    /// Refuse to bind `name` when it is not bound yet and another NAME of
    /// the node, other than `leaving`, folds to the same key: the two would be
    /// one file on a case- or normalization-insensitive filesystem. Rebinding
    /// a NAME already present introduces no pair, so it is allowed even while
    /// a pair two peers made concurrently exists. Every stored NAME counts,
    /// rejected entries too.
    fn check_no_collision(&self, name: &str, leaving: &str) -> Result<(), AttachmentError> {
        let txn = self.doc.transact();
        let Some(map) = txn.get_map(ATTACHMENTS_ROOT) else {
            return Ok(());
        };
        if map.contains_key(&txn, name) {
            return Ok(());
        }
        let key = fold_key(name);
        // The least such NAME, so the error is the same whatever order the
        // map (a hash map) iterates in.
        match map
            .keys(&txn)
            .filter(|k| *k != leaving && fold_key(k) == key)
            .min()
        {
            Some(existing) => Err(AttachmentError::NameCollides {
                existing: existing.to_string(),
            }),
            None => Ok(()),
        }
    }

    /// The NAMEs of the entries a writer must not introduce: a bad name or a
    /// bad shape (unknown extra fields are fine, so a newer client's additive
    /// field never trips an older daemon). For the field-type check, which
    /// compares them entry by entry.
    pub(super) fn rejected_attachment_names(&self) -> std::collections::BTreeSet<String> {
        self.read_attachments()
            .problems
            .into_iter()
            .filter_map(|p| match p {
                AttachmentProblem::Rejected { name, .. } => Some(name),
                AttachmentProblem::UnknownFields { .. } => None,
            })
            .collect()
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
