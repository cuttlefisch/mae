//! SQL for a revision's attachments (ADR-113 D6, B1–B4): `rendered_pdf` and
//! `signed_pdf`, each a blob of the KB's attachment store (ADR-115) bound to
//! the revision by hash. A revision holds at most one current attachment of
//! each kind; a wrong one is annulled, never replaced, and the kind then takes
//! one new attach.
//!
//! @ai-caution: [append-only] The database enforces the record, not only the
//! methods: a partial unique index allows one `current` row per revision and
//! kind, so two concurrent attaches of different bytes store exactly one; a
//! trigger refuses every delete; and the only update it allows is a current
//! row becoming annulled, every other column unchanged. B5's deletion
//! (PR-7) must widen that trigger deliberately, never drop it.

use crate::storage::StorageError;

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS kb_review_attachments (
     kb_id TEXT NOT NULL,
     revision_id TEXT NOT NULL,
     kind TEXT NOT NULL CHECK (kind IN ('rendered_pdf', 'signed_pdf')),
     seq INTEGER NOT NULL,
     sha256 TEXT NOT NULL,
     size INTEGER NOT NULL,
     content_type TEXT NOT NULL,
     declared_source TEXT,
     uploader TEXT NOT NULL,
     uploader_kind TEXT NOT NULL,
     iss TEXT,
     sub TEXT,
     at INTEGER NOT NULL,
     event_seq INTEGER NOT NULL,
     state TEXT NOT NULL CHECK (state IN ('current', 'annulled', 'deleted')),
     annulled_at INTEGER,
     annulled_by TEXT,
     annul_reason TEXT,
     annul_event_seq INTEGER,
     PRIMARY KEY (revision_id, kind, seq)
 );
 CREATE UNIQUE INDEX IF NOT EXISTS idx_review_attach_current
     ON kb_review_attachments(revision_id, kind) WHERE state = 'current';
 CREATE INDEX IF NOT EXISTS idx_review_attach_kb ON kb_review_attachments(kb_id);
 CREATE TRIGGER IF NOT EXISTS kb_review_attachments_no_delete
 BEFORE DELETE ON kb_review_attachments
 BEGIN SELECT RAISE(ABORT, 'a revision attachment is never deleted'); END;
 CREATE TRIGGER IF NOT EXISTS kb_review_attachments_only_annul
 BEFORE UPDATE ON kb_review_attachments
 WHEN NOT (OLD.state = 'current' AND NEW.state = 'annulled' AND OLD.annulled_at IS NULL
     AND NEW.annulled_at IS NOT NULL AND NEW.annulled_by IS NOT NULL
     AND NEW.annul_reason IS NOT NULL
     AND NEW.kb_id = OLD.kb_id AND NEW.revision_id = OLD.revision_id
     AND NEW.kind = OLD.kind AND NEW.seq = OLD.seq AND NEW.sha256 = OLD.sha256
     AND NEW.size = OLD.size AND NEW.content_type = OLD.content_type
     AND NEW.declared_source IS OLD.declared_source AND NEW.uploader = OLD.uploader
     AND NEW.uploader_kind = OLD.uploader_kind AND NEW.iss IS OLD.iss
     AND NEW.sub IS OLD.sub AND NEW.at = OLD.at AND NEW.event_seq = OLD.event_seq)
 BEGIN SELECT RAISE(ABORT, 'a revision attachment changes only by annulment'); END;";

/// The two kinds (B2). The list is closed: another kind is another amendment.
pub(crate) const KINDS: &[&str] = &["rendered_pdf", "signed_pdf"];

/// Who annulled an attachment, when and why (B1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Annulment {
    pub at: i64,
    pub by: String,
    pub reason: String,
}

/// One attachment row (B4: what is recorded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Attachment {
    pub kb_id: String,
    pub revision_id: String,
    pub kind: String,
    pub seq: i64,
    pub sha256: String,
    pub size: u64,
    pub content_type: String,
    /// `rendered_pdf`: the source the uploader declared it rendered (D6).
    pub declared_source: Option<String>,
    pub uploader: String,
    pub uploader_kind: String,
    pub iss: Option<String>,
    pub sub: Option<String>,
    pub at: i64,
    pub event_seq: u64,
    pub state: String,
    pub annulled: Option<Annulment>,
}

const COLUMNS: &str = "kb_id, revision_id, kind, seq, sha256, size, content_type, \
     declared_source, uploader, uploader_kind, iss, sub, at, event_seq, state, annulled_at, \
     annulled_by, annul_reason";

fn bad(what: &str) -> StorageError {
    StorageError::Sqlite(format!("revision attachment row: bad {what}"))
}

fn read(st: &sqlite::Statement<'_>) -> Result<Attachment, StorageError> {
    let s = |c: &str| st.read::<String, _>(c).map_err(StorageError::from);
    let os = |c: &str| st.read::<Option<String>, _>(c).map_err(StorageError::from);
    let int = |c: &str| st.read::<i64, _>(c).map_err(StorageError::from);
    let annulled = match st.read::<Option<i64>, _>("annulled_at")? {
        None => None,
        Some(at) => Some(Annulment {
            at,
            by: os("annulled_by")?.ok_or_else(|| bad("annulled_by"))?,
            reason: os("annul_reason")?.ok_or_else(|| bad("annul_reason"))?,
        }),
    };
    Ok(Attachment {
        kb_id: s("kb_id")?,
        revision_id: s("revision_id")?,
        kind: s("kind")?,
        seq: int("seq")?,
        sha256: s("sha256")?,
        size: u64::try_from(int("size")?).map_err(|_| bad("size"))?,
        content_type: s("content_type")?,
        declared_source: os("declared_source")?,
        uploader: s("uploader")?,
        uploader_kind: s("uploader_kind")?,
        iss: os("iss")?,
        sub: os("sub")?,
        at: int("at")?,
        event_seq: u64::try_from(int("event_seq")?).map_err(|_| bad("event_seq"))?,
        state: s("state")?,
        annulled,
    })
}

fn collect(st: &mut sqlite::Statement<'_>) -> Result<Vec<Attachment>, StorageError> {
    let mut out = Vec::new();
    while st.next()? == sqlite::State::Row {
        out.push(read(st)?);
    }
    Ok(out)
}

fn opt(v: Option<&str>) -> sqlite::Value {
    v.map_or(sqlite::Value::Null, |s| s.into())
}

/// Record a new current attachment. The unique index refuses a second current
/// one of the same kind (B1), whatever raced to it.
pub(crate) fn insert(conn: &sqlite::Connection, a: &Attachment) -> Result<(), StorageError> {
    let mut st = conn.prepare(format!(
        "INSERT INTO kb_review_attachments ({COLUMNS})
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 'current',
                 NULL, NULL, NULL)"
    ))?;
    let size = i64::try_from(a.size).map_err(|_| bad("size"))?;
    let seq = i64::try_from(a.event_seq).map_err(|_| bad("event_seq"))?;
    let values: [sqlite::Value; 14] = [
        a.kb_id.as_str().into(),
        a.revision_id.as_str().into(),
        a.kind.as_str().into(),
        a.seq.into(),
        a.sha256.as_str().into(),
        size.into(),
        a.content_type.as_str().into(),
        opt(a.declared_source.as_deref()),
        a.uploader.as_str().into(),
        a.uploader_kind.as_str().into(),
        opt(a.iss.as_deref()),
        opt(a.sub.as_deref()),
        a.at.into(),
        seq.into(),
    ];
    for (i, v) in values.into_iter().enumerate() {
        st.bind((i + 1, v))?;
    }
    st.next()?;
    Ok(())
}

/// The revision's current attachment of `kind`, if any.
pub(crate) fn current(
    conn: &sqlite::Connection,
    revision_id: &str,
    kind: &str,
) -> Result<Option<Attachment>, StorageError> {
    let mut st = conn.prepare(format!(
        "SELECT {COLUMNS} FROM kb_review_attachments
         WHERE revision_id = ?1 AND kind = ?2 AND state = 'current'"
    ))?;
    st.bind((1, revision_id))?;
    st.bind((2, kind))?;
    Ok(collect(&mut st)?.into_iter().next())
}

/// Every attachment of a revision, annulled ones included, in upload order (D5).
pub(crate) fn of_revision(
    conn: &sqlite::Connection,
    revision_id: &str,
) -> Result<Vec<Attachment>, StorageError> {
    let mut st = conn.prepare(format!(
        "SELECT {COLUMNS} FROM kb_review_attachments WHERE revision_id = ?1 ORDER BY rowid"
    ))?;
    st.bind((1, revision_id))?;
    collect(&mut st)
}

/// The next sequence number within a revision's kind (1 for the first).
pub(crate) fn next_seq(
    conn: &sqlite::Connection,
    revision_id: &str,
    kind: &str,
) -> Result<i64, StorageError> {
    let mut st = conn.prepare(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM kb_review_attachments
         WHERE revision_id = ?1 AND kind = ?2",
    )?;
    st.bind((1, revision_id))?;
    st.bind((2, kind))?;
    st.next()?;
    Ok(st.read::<i64, _>(0)?)
}

/// Annul the revision's current attachment of `kind` (B1). Whether there was
/// one to annul.
pub(crate) fn annul(
    conn: &sqlite::Connection,
    revision_id: &str,
    kind: &str,
    by: &Annulment,
    event_seq: u64,
) -> Result<bool, StorageError> {
    let mut st = conn.prepare(
        "UPDATE kb_review_attachments
         SET state = 'annulled', annulled_at = ?3, annulled_by = ?4, annul_reason = ?5,
             annul_event_seq = ?6
         WHERE revision_id = ?1 AND kind = ?2 AND state = 'current'",
    )?;
    st.bind((1, revision_id))?;
    st.bind((2, kind))?;
    st.bind((3, by.at))?;
    st.bind((4, by.by.as_str()))?;
    st.bind((5, by.reason.as_str()))?;
    st.bind((6, i64::try_from(event_seq).map_err(|_| bad("event_seq"))?))?;
    st.next()?;
    Ok(conn.change_count() == 1)
}

/// Every blob a KB's revisions hold, annulled ones included (B5: an annulled
/// attachment counts as referenced exactly as a current one). For the sweep.
pub(crate) fn hashes(conn: &sqlite::Connection, kb_id: &str) -> Result<Vec<String>, StorageError> {
    let mut st = conn.prepare(
        "SELECT DISTINCT sha256 FROM kb_review_attachments
         WHERE kb_id = ?1 AND state != 'deleted'",
    )?;
    st.bind((1, kb_id))?;
    let mut out = Vec::new();
    while st.next()? == sqlite::State::Row {
        out.push(st.read::<String, _>(0)?);
    }
    Ok(out)
}
