//! The per-document editor record (ADR-113 D3): who applied updates to a
//! document, and when.
//!
//! ADR-113's author rule needs "every principal who edited this node since time
//! T". The CRDT cannot answer that — its client ids are per-device and unsigned,
//! and its op log is compacted away — so the daemon records the answer itself, as
//! it applies each update. Like ADR-106's version history it is a **local audit
//! trail outside the CRDT**: it is not synced, and a mesh peer keeps its own.
//!
//! One coalesced row per `(document, kind, editor)`, not a row per update: the
//! author rule only ever asks "did X edit after T", which `last_at > T` answers,
//! and a per-update log would grow without bound on a busy node.
//!
//! Every write names its [`Attribution`]; there is deliberately no default and no
//! "unknown". A write path that cannot say who it is acting for does not compile.

/// Who is applying an update to a document.
///
/// Only [`Attribution::Principal`] names a person (or an agent acting as one —
/// ADR-098 marks those). Every other variant is recorded under its own
/// [`EditorKind`], so a consumer can never mistake a relay, the daemon, or an
/// unauthenticated socket for a principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribution<'a> {
    /// A collab session's authenticated principal: the key fingerprint, the
    /// `psk:<keyid>`, or an OAuth principal — whatever `kb_access` authorizes.
    Principal(&'a str),
    /// A collab session with no authenticated principal (auth mode `none`, the
    /// un-authed local socket). Named by its session id, which is NOT stable
    /// across restarts: the edit is visible as unattributable rather than missing.
    Unauthenticated { session_id: u64 },
    /// A mesh peer delivering updates to this daemon (the dialer). Names the
    /// remote daemon's key fingerprint, not the person who wrote the edit — a
    /// relay is not an author, and is recorded as a relay.
    MeshPeer(&'a str),
    /// The daemon itself, acting on no session's behalf.
    Internal(InternalWriter),
}

impl<'a> Attribution<'a> {
    /// A collab session's attribution: its authenticated principal, or — on a
    /// session that has none — the session itself, marked unauthenticated.
    pub fn session(principal: Option<&'a str>, session_id: u64) -> Self {
        match principal {
            Some(p) if !p.is_empty() => Self::Principal(p),
            _ => Self::Unauthenticated { session_id },
        }
    }

    /// The `(kind, editor)` key this attribution is recorded under.
    pub(crate) fn key(&self) -> (EditorKind, String) {
        match *self {
            Self::Principal(p) => (EditorKind::Principal, p.to_string()),
            Self::Unauthenticated { session_id } => {
                (EditorKind::Unauthenticated, format!("session:{session_id}"))
            }
            Self::MeshPeer(fp) => (EditorKind::MeshPeer, fp.to_string()),
            Self::Internal(w) => (EditorKind::Internal, w.as_str().to_string()),
        }
    }
}

/// A daemon-internal writer. Recorded (as [`EditorKind::Internal`]) rather than
/// skipped: a migration or restore that rewrote a node since its last approval is
/// something ADR-113's working-copy history (D7) must show, and recording it under
/// its own kind keeps it out of any principal-based rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalWriter {
    /// `load_collection`'s legacy v1 → v2 collection migration (ADR-018 #73).
    CollectionMigration,
    /// The ADR-105 node-document address migration (`kb_doc_migration`).
    DocAddressMigration,
    /// A checkpoint restore (`checkpoint::import_kb`), run by the operator.
    CheckpointRestore,
    /// The local control socket (`handler`), e.g. seeding a P2P share from the
    /// daemon's own KB store. Authenticated by socket file permissions only.
    ControlSocket,
    /// The maintenance scheduler (ADR-061 enrichment lease claims).
    Scheduler,
}

impl InternalWriter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CollectionMigration => "collection-migration",
            Self::DocAddressMigration => "doc-address-migration",
            Self::CheckpointRestore => "checkpoint-restore",
            Self::ControlSocket => "control-socket",
            Self::Scheduler => "scheduler",
        }
    }
}

/// The class of an editor record. Stored as text; see [`EditorKind::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKind {
    Principal,
    Unauthenticated,
    MeshPeer,
    Internal,
}

impl EditorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Principal => "principal",
            Self::Unauthenticated => "unauthenticated",
            Self::MeshPeer => "mesh-peer",
            Self::Internal => "internal",
        }
    }

    /// The inverse of [`Self::as_str`]. `None` for a value this build does not
    /// know, which the reader reports as an error rather than guessing a kind.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "principal" => Some(Self::Principal),
            "unauthenticated" => Some(Self::Unauthenticated),
            "mesh-peer" => Some(Self::MeshPeer),
            "internal" => Some(Self::Internal),
            _ => None,
        }
    }
}

/// What a write records: the editor, and the daemon's clock when it applied the
/// update (Unix milliseconds). Written in the same transaction as the WAL entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditStamp {
    pub kind: EditorKind,
    pub editor: String,
    pub at_ms: u64,
}

impl EditStamp {
    pub fn new(by: Attribution<'_>, at_ms: u64) -> Self {
        let (kind, editor) = by.key();
        EditStamp {
            kind,
            editor,
            at_ms,
        }
    }
}

/// One editor of one document, coalesced over all its updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorRecord {
    pub kind: EditorKind,
    pub editor: String,
    /// Daemon clock (Unix ms) of the first update this editor applied.
    pub first_at_ms: u64,
    /// Daemon clock (Unix ms) of the latest. Never moves backwards, even if the
    /// wall clock does: the row keeps the maximum it has seen.
    pub last_at_ms: u64,
    /// WAL id of the latest update. WAL ids are AUTOINCREMENT, so this is a
    /// clock-independent order across every document in the database — an
    /// alternative to `last_at_ms` for a consumer that must not trust the clock.
    pub last_wal_id: u64,
    /// How many updates this editor applied.
    pub updates: u64,
}

/// The daemon clock the editor record is stamped with: wall-clock Unix
/// milliseconds. Injectable via `DocStore::with_clock` so tests order edits
/// without sleeping.
pub type EditClock = std::sync::Arc<dyn Fn() -> u64 + Send + Sync>;

/// The production clock.
pub fn system_clock() -> EditClock {
    std::sync::Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_and_unknown_is_refused() {
        for k in [
            EditorKind::Principal,
            EditorKind::Unauthenticated,
            EditorKind::MeshPeer,
            EditorKind::Internal,
        ] {
            assert_eq!(EditorKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(EditorKind::parse(""), None);
        assert_eq!(EditorKind::parse("Principal"), None);
    }

    /// A session with no principal — or an empty one — is never recorded as a
    /// principal: an empty string would otherwise be a shared "anyone" editor.
    #[test]
    fn session_without_a_principal_is_unauthenticated() {
        assert_eq!(
            Attribution::session(Some("fp-a"), 7),
            Attribution::Principal("fp-a")
        );
        for p in [None, Some("")] {
            assert_eq!(
                Attribution::session(p, 7),
                Attribution::Unauthenticated { session_id: 7 }
            );
        }
    }

    /// The same string under different kinds is different editors: a mesh peer or
    /// an internal writer can never be read back as a principal.
    #[test]
    fn kind_is_part_of_the_key() {
        let a = Attribution::Principal("x").key();
        let b = Attribution::MeshPeer("x").key();
        assert_eq!(a.1, b.1);
        assert_ne!(a.0, b.0);
        assert_eq!(
            Attribution::Internal(InternalWriter::Scheduler).key(),
            (EditorKind::Internal, "scheduler".to_string())
        );
    }
}

#[cfg(test)]
#[path = "doc_editors_tests.rs"]
mod store_tests;
