//! Canonical encodings: every field length-prefixed (`u64` little-endian), so
//! no two different values encode alike, and a decoder that accepts only the
//! one encoding a value has, so stored content cannot carry a second,
//! differently-hashed spelling of the same node.

use std::collections::HashMap;

use super::super::{MaterializedNode, OrgHeader};
use super::{HistoryEntry, GENESIS};

/// Why stored content is not a node's canonical encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentError {
    /// It ended inside a field.
    Truncated,
    /// A field is malformed: a bad tag, width, UTF-8, or key order.
    Malformed(&'static str),
    /// Bytes follow the last field.
    Trailing,
}

impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => f.write_str("content ends inside a field"),
            Self::Malformed(what) => write!(f, "content is malformed: {what}"),
            Self::Trailing => f.write_str("content has bytes after its last field"),
        }
    }
}

const CONTENT_MAGIC: &[u8] = b"maenode/v1";
const ENTRY_MAGIC: &[u8] = b"maehistory/v1";

#[derive(Default)]
pub(super) struct Writer(pub(super) Vec<u8>);

impl Writer {
    pub(super) fn bytes(&mut self, b: &[u8]) {
        self.0.extend((b.len() as u64).to_le_bytes());
        self.0.extend(b);
    }
    pub(super) fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.bytes(&v.to_le_bytes());
    }
    fn flag(&mut self, v: bool) {
        self.bytes(&[u8::from(v)]);
    }
    fn opt_str(&mut self, v: Option<&str>) {
        self.flag(v.is_some());
        if let Some(s) = v {
            self.bytes(s.as_bytes());
        }
    }
    fn opt_u64(&mut self, v: Option<u64>) {
        self.flag(v.is_some());
        if let Some(n) = v {
            self.u64(n);
        }
    }
    fn strs(&mut self, items: &[String]) {
        self.u64(items.len() as u64);
        for s in items {
            self.bytes(s.as_bytes());
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn bytes(&mut self) -> Result<&'a [u8], ContentError> {
        if self.0.len() < 8 {
            return Err(ContentError::Truncated);
        }
        let (len, rest) = self.0.split_at(8);
        let len = u64::from_le_bytes(len.try_into().expect("8 bytes"));
        let len = usize::try_from(len).map_err(|_| ContentError::Truncated)?;
        if rest.len() < len {
            return Err(ContentError::Truncated);
        }
        let (field, rest) = rest.split_at(len);
        self.0 = rest;
        Ok(field)
    }
    fn fixed<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], ContentError> {
        self.bytes()?
            .try_into()
            .map_err(|_| ContentError::Malformed(what))
    }
    fn u64(&mut self) -> Result<u64, ContentError> {
        Ok(u64::from_le_bytes(self.fixed("integer width")?))
    }
    fn flag(&mut self) -> Result<bool, ContentError> {
        match self.fixed::<1>("flag width")? {
            [0] => Ok(false),
            [1] => Ok(true),
            _ => Err(ContentError::Malformed("flag value")),
        }
    }
    fn string(&mut self) -> Result<String, ContentError> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| ContentError::Malformed("UTF-8"))
    }
    fn opt_string(&mut self) -> Result<Option<String>, ContentError> {
        self.flag()?.then(|| self.string()).transpose()
    }
    fn strings(&mut self) -> Result<Vec<String>, ContentError> {
        let n = self.u64()?;
        // Each item takes at least its 8-byte length: a count the remaining
        // bytes cannot hold is refused before anything is allocated for it.
        if n > (self.0.len() / 8) as u64 {
            return Err(ContentError::Truncated);
        }
        (0..n).map(|_| self.string()).collect()
    }
}

/// The node's canonical encoding: every [`MaterializedNode`] field in order,
/// properties sorted by key.
pub fn encode_content(node: &MaterializedNode) -> Vec<u8> {
    // No `..`: a field added to MaterializedNode must be stored too.
    let MaterializedNode {
        id,
        title,
        body,
        tags,
        links,
        kind,
        todo_state,
        priority,
        aliases,
        properties,
        source_version,
        source,
        created_at,
        org_header,
    } = node;
    let mut w = Writer::default();
    w.bytes(CONTENT_MAGIC);
    for s in [id, title, body] {
        w.bytes(s.as_bytes());
    }
    w.strs(tags);
    w.strs(links);
    for s in [kind, todo_state, priority] {
        w.opt_str(s.as_deref());
    }
    w.strs(aliases);
    let mut props: Vec<(&String, &String)> = properties.iter().collect();
    props.sort();
    w.u64(props.len() as u64);
    for (k, v) in props {
        w.bytes(k.as_bytes());
        w.bytes(v.as_bytes());
    }
    w.opt_u64(source_version.map(u64::from));
    w.opt_str(source.as_deref());
    w.flag(created_at.is_some());
    if let Some(t) = created_at {
        w.i64(*t);
    }
    w.flag(org_header.is_some());
    if let Some(h) = org_header {
        w.strs(&h.property_order);
        w.flag(h.has_title_line);
        w.flag(h.has_filetags_line);
    }
    w.0
}

/// The node `bytes` encode, if they are its canonical encoding: anything
/// [`encode_content`] would not produce (a reordered property, a flag of 2,
/// a trailing byte) is refused.
pub fn decode_content(bytes: &[u8]) -> Result<MaterializedNode, ContentError> {
    let mut r = Reader(bytes);
    if r.bytes()? != CONTENT_MAGIC {
        return Err(ContentError::Malformed("not a node encoding"));
    }
    let (id, title, body) = (r.string()?, r.string()?, r.string()?);
    let (tags, links) = (r.strings()?, r.strings()?);
    let (kind, todo_state, priority) = (r.opt_string()?, r.opt_string()?, r.opt_string()?);
    let aliases = r.strings()?;
    let properties = read_properties(&mut r)?;
    let source_version = r
        .flag()?
        .then(|| r.u64())
        .transpose()?
        .map(|v| u32::try_from(v).map_err(|_| ContentError::Malformed("source_version")))
        .transpose()?;
    let source = r.opt_string()?;
    let created_at = r
        .flag()?
        .then(|| r.fixed("integer width").map(i64::from_le_bytes))
        .transpose()?;
    let org_header = r
        .flag()?
        .then(|| -> Result<OrgHeader, ContentError> {
            Ok(OrgHeader {
                property_order: r.strings()?,
                has_title_line: r.flag()?,
                has_filetags_line: r.flag()?,
            })
        })
        .transpose()?;
    if !r.0.is_empty() {
        return Err(ContentError::Trailing);
    }
    Ok(MaterializedNode {
        id,
        title,
        body,
        tags,
        links,
        kind,
        todo_state,
        priority,
        aliases,
        properties,
        source_version,
        source,
        created_at,
        org_header,
    })
}

/// Properties, whose keys must be strictly increasing: the one order
/// [`encode_content`] writes, so a key cannot appear twice either.
fn read_properties(r: &mut Reader<'_>) -> Result<HashMap<String, String>, ContentError> {
    let n = r.u64()?;
    if n > (r.0.len() / 16) as u64 {
        return Err(ContentError::Truncated);
    }
    let mut out = HashMap::new();
    let mut last: Option<String> = None;
    for _ in 0..n {
        let (k, v) = (r.string()?, r.string()?);
        if last.as_ref().is_some_and(|l| *l >= k) {
            return Err(ContentError::Malformed("property order"));
        }
        last = Some(k.clone());
        out.insert(k, v);
    }
    Ok(out)
}

/// The bytes `entry_hash` is the SHA-256 of: the domain, the predecessor's
/// hash, and every field but `content` (which `content_hash` stands for) and
/// `entry_hash` itself.
pub(super) fn entry_preimage(e: &HistoryEntry) -> Vec<u8> {
    let mut w = Writer::default();
    w.bytes(ENTRY_MAGIC);
    w.bytes(&e.prev_hash);
    w.bytes(e.kb_id.as_bytes());
    w.bytes(e.node_id.as_bytes());
    w.u64(e.seq);
    w.bytes(e.kind.as_str().as_bytes());
    w.i64(e.created_at);
    w.u64(e.editors.len() as u64);
    for ed in &e.editors {
        w.bytes(ed.kind.as_str().as_bytes());
        w.bytes(ed.principal.as_bytes());
    }
    w.u64(e.wal_from);
    w.u64(e.wal_to);
    w.opt_str(e.summary.as_deref());
    w.opt_str(e.name.as_deref());
    w.opt_u64(e.restored_from);
    w.bytes(&e.content_hash);
    w.0
}

/// Whether `h` is the hash of no content.
pub(super) fn is_genesis(h: &[u8; 32]) -> bool {
    *h == GENESIS
}
