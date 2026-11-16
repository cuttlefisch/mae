//! SQL for the per-KB content trust anchor (ADR-026, ADR-036 Amendment 1 F1).
//!
//! One row per KB: the owner public key its signed membership op-log is
//! verified against, and where that key came from. Local to this daemon and
//! never synced: a joined KB's anchor comes from its join ticket, a hosted KB's
//! from the session that shared it, and neither is something a peer may tell us.

use super::StorageError;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS kb_anchors (
     kb_id TEXT PRIMARY KEY,
     pubkey BLOB NOT NULL,
     source TEXT NOT NULL DEFAULT 'joined',
     recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
 );";

/// Where a KB's anchor came from — which decides what happens when its log has
/// no genesis the anchor signed (ADR-036 Amendment 1 F2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorSource {
    /// This hub recorded it: the owner's session at first share, the owner's
    /// next share or write, or the operator. The hub is the authority for the
    /// KB's member list, so a log without that genesis falls back to it.
    HubRecorded,
    /// Registered from a mesh join (the join ticket's node id). The member list
    /// came from a relay, so a log without that genesis yields no members.
    Joined,
}

impl AnchorSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HubRecorded => "hub_recorded",
            Self::Joined => "joined",
        }
    }

    /// Anything but `hub_recorded` reads as `Joined`: the fail-closed source,
    /// so an unknown value can never grant the fallback.
    pub fn parse(s: &str) -> Self {
        match s {
            "hub_recorded" => Self::HubRecorded,
            _ => Self::Joined,
        }
    }
}

pub(super) fn ensure_schema(conn: &sqlite::Connection) -> Result<(), sqlite::Error> {
    conn.execute(SCHEMA)
}

/// Every recorded anchor. A row whose key is not 32 bytes is an error, never
/// skipped: an anchor that silently vanished would move its KB's membership to
/// another source.
pub(super) fn load(
    conn: &sqlite::Connection,
) -> Result<Vec<(String, [u8; 32], AnchorSource)>, StorageError> {
    let mut stmt = conn.prepare("SELECT kb_id, pubkey, source FROM kb_anchors")?;
    let mut rows = Vec::new();
    while let sqlite::State::Row = stmt.next()? {
        let kb_id = stmt.read::<String, _>("kb_id")?;
        let key = stmt.read::<Vec<u8>, _>("pubkey")?;
        let source = AnchorSource::parse(&stmt.read::<String, _>("source")?);
        let key: [u8; 32] = key.try_into().map_err(|k: Vec<u8>| {
            StorageError::Sqlite(format!(
                "KB '{kb_id}' has a {}-byte content anchor, not 32",
                k.len()
            ))
        })?;
        rows.push((kb_id, key, source));
    }
    Ok(rows)
}

pub(super) fn put(
    conn: &sqlite::Connection,
    kb_id: &str,
    pubkey: &[u8; 32],
    source: AnchorSource,
) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(
        "INSERT INTO kb_anchors (kb_id, pubkey, source) VALUES (?1, ?2, ?3)
         ON CONFLICT(kb_id) DO UPDATE SET pubkey = excluded.pubkey,
                                          source = excluded.source,
                                          recorded_at = datetime('now')",
    )?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, &pubkey[..]))?;
    stmt.bind((3, source.as_str()))?;
    stmt.next()?;
    Ok(())
}
