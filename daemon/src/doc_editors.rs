//! The per-document editor record (ADR-113 D3): who applied updates to a
//! document, and after which WAL entry.
//!
//! ADR-113's author rule needs "every principal who edited this node since its
//! last approved revision". The CRDT cannot answer that — its client ids are
//! per-device and unsigned, and its op log is compacted away — so the daemon
//! records the answer itself, as it applies each update. Like ADR-106's version
//! history it is a **local audit trail outside the CRDT**: it is not synced, and
//! a mesh peer keeps its own.
//!
//! One coalesced row per `(document, kind, editor)`, not a row per update: the
//! author rule only asks "did X edit after the freeze point", which
//! `last_wal_id > cutoff` answers, and a per-update log would grow without bound.
//!
//! **Ordering is by WAL id, not by clock.** WAL ids are AUTOINCREMENT, so they
//! order every update in the database regardless of what the wall clock does.
//! The timestamps on a row are for display only; nothing here filters on them.
//!
//! Every write names its [`Attribution`]; there is deliberately no default and
//! no "unknown". A write path that cannot say who it is acting for does not
//! compile.
//!
//! **Who "the author" is — a stated assumption, not a guarantee.** The record
//! names whoever SUBMITTED an update (and its verified ADR-036 signer, when it
//! has one), not the authors of every struct inside it. A yrs update can carry
//! another client's content the daemon never saw being written: a full state
//! re-shared by Bob may include text Alice wrote elsewhere. Until the hub
//! verifies a signature on every content op (#727, ADR-111 D3), a submitter is
//! treated as the attributable author of everything in their update. Consumers
//! must read the author set as "the principals accountable for these writes",
//! which is what the approver-is-not-an-author rule needs; it is not a
//! cryptographic statement of who typed which character.
//!
//! **What over-inclusion costs.** Every rule here errs toward recording someone
//! as an author who maybe was not one, never the reverse: an extra author can
//! only be refused approval of a revision (fail-closed), while a missing one
//! could approve their own work. Two consequences are accepted knowingly:
//! - Replaying a principal's old ADR-036 signed content op makes them an author
//!   again (the signature binds the payload, not the moment). The worst it does
//!   is block that principal from approving the revision — griefing, not
//!   escalation.
//! - An update yrs holds pending (its dependencies have not arrived) changes the
//!   document only when a later update integrates it. Its submitter is
//!   therefore stamped again at that later moment, alongside whoever filled the
//!   gap — and while anything stays pending, at every change to the pending set.

use std::collections::BTreeSet;

use mae_sync::kb::Transport;

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
    /// un-authed local socket). One shared identity per document: such sessions
    /// have no stable identity to tell apart, and a row per session would grow
    /// without bound.
    Unauthenticated,
    /// A mesh peer delivering updates to this daemon — the dialer on a joiner,
    /// or any session on the P2P transport. Names the remote daemon's key
    /// fingerprint: a relay, not the person who wrote the edit.
    MeshPeer(&'a str),
    /// The daemon itself, acting on no session's behalf.
    Internal(InternalWriter),
}

impl<'a> Attribution<'a> {
    /// A collab session's attribution. On the P2P transport the peer is always
    /// another daemon relaying its own users' edits, so it is a [`MeshPeer`]
    /// even though `kb_access` authorizes it as a member; elsewhere it is the
    /// session's principal. A session with no (or an empty) principal is
    /// [`Unauthenticated`].
    ///
    /// [`MeshPeer`]: Attribution::MeshPeer
    /// [`Unauthenticated`]: Attribution::Unauthenticated
    pub fn session(principal: Option<&'a str>, transport: Transport) -> Self {
        match principal {
            Some(p) if !p.is_empty() && transport == Transport::P2p => Self::MeshPeer(p),
            Some(p) if !p.is_empty() => Self::Principal(p),
            _ => Self::Unauthenticated,
        }
    }

    /// The `(kind, editor)` key this attribution is recorded under when no
    /// signer vouches for the content.
    fn key(&self) -> (EditorKind, String) {
        match *self {
            Self::Principal(p) => (EditorKind::Principal, p.to_string()),
            Self::Unauthenticated => (EditorKind::Unauthenticated, "unauthenticated".to_string()),
            Self::MeshPeer(fp) => (EditorKind::MeshPeer, fp.to_string()),
            Self::Internal(w) => (EditorKind::Internal, w.as_str().to_string()),
        }
    }
}

/// Who applied an update, and — when it carried an ADR-036 signed content op
/// the daemon VERIFIED — who signed it. The signer is the content's author: the
/// signature binds them to this exact `(kb_id, node_id, payload)`, so it holds
/// however many relays the op crossed.
///
/// Every `Attribution` converts into an unsigned `Writer`, so only the paths
/// that verify a signature need to say anything more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Writer<'a> {
    pub by: Attribution<'a>,
    /// A verified signer. Never set from an unverified header.
    pub signer: Option<&'a str>,
}

impl<'a> From<Attribution<'a>> for Writer<'a> {
    fn from(by: Attribution<'a>) -> Self {
        Writer { by, signer: None }
    }
}

impl<'a> Writer<'a> {
    pub fn signed(by: Attribution<'a>, signer: Option<&'a str>) -> Self {
        Writer {
            by,
            signer: signer.filter(|s| !s.is_empty()),
        }
    }

    /// Every `(kind, editor)` this write is recorded under. A relay whose
    /// update was signed is recorded as a [`EditorKind::SignedRelay`] (the
    /// author is known: the signer) rather than an unsigned [`EditorKind::MeshPeer`]
    /// (the author is not).
    pub(crate) fn keys(&self) -> Vec<(EditorKind, String)> {
        let first = match (self.by, self.signer) {
            (Attribution::MeshPeer(fp), Some(_)) => (EditorKind::SignedRelay, fp.to_string()),
            (by, _) => by.key(),
        };
        let mut keys = vec![first];
        if let Some(s) = self.signer {
            let signer = (EditorKind::Principal, s.to_string());
            if !keys.contains(&signer) {
                keys.push(signer);
            }
        }
        keys
    }
}

/// The author named by an ADR-036 content-op header — the `author` field.
/// Call this ONLY on a header the daemon has verified (`verify_content_op` /
/// `verify_relayed_content_op` returned it): an unverified header is a claim,
/// and recording it would let a relay pick its own author.
pub fn verified_signer(header: Option<&serde_json::Value>) -> Option<&str> {
    header?.get("author")?.as_str()
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EditorKind {
    Principal,
    Unauthenticated,
    /// A relay whose update carried NO verified signer: the author is unknown.
    MeshPeer,
    /// A relay whose update was signed; the signer has its own `Principal` row.
    SignedRelay,
    Internal,
}

impl EditorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Principal => "principal",
            Self::Unauthenticated => "unauthenticated",
            Self::MeshPeer => "mesh-peer",
            Self::SignedRelay => "signed-relay",
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
            "signed-relay" => Some(Self::SignedRelay),
            "internal" => Some(Self::Internal),
            _ => None,
        }
    }

    /// Whether a row of this kind leaves the authors of a change unknown.
    /// Only a principal (a known author) and a signed relay (whose signer is
    /// recorded as a principal) do not.
    fn hides_an_author(self) -> bool {
        !matches!(self, Self::Principal | Self::SignedRelay)
    }
}

/// How an apply changed the updates yrs holds pending (see the module doc).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PendingChange {
    /// This update's content may be among the pending structs: remember its
    /// submitter until the pending set drains.
    pub leaves_pending: bool,
    /// The pending set existed and changed, so content held back from earlier
    /// submitters may have integrated now: stamp every remembered submitter.
    pub integrated: bool,
    /// Nothing is pending any more: forget the remembered submitters.
    pub cleared: bool,
}

impl PendingChange {
    /// A share or restore replaces the whole document. A full state carries
    /// the structs the old document held pending, so content held back from
    /// earlier submitters may be installed by it — stamp every remembered
    /// submitter (marks exist only while something is pending, so this needs
    /// no check of the old document). `new_pending`: the new state still
    /// holds structs back, so its sharer is remembered in turn.
    pub fn replacing(new_pending: bool) -> Self {
        PendingChange {
            leaves_pending: new_pending,
            integrated: true,
            cleared: !new_pending,
        }
    }

    /// From the pending digests before and after an apply
    /// (`TextSync::pending_digest`).
    pub fn between(before: &Option<Vec<u8>>, after: &Option<Vec<u8>>) -> Self {
        let changed = before != after;
        PendingChange {
            leaves_pending: after.is_some() && changed,
            integrated: before.is_some() && changed,
            cleared: before.is_some() && after.is_none(),
        }
    }
}

/// What a write records, in the same transaction as its WAL entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditStamp {
    /// Every `(kind, editor)` the update is recorded under (at least one).
    pub editors: Vec<(EditorKind, String)>,
    /// Daemon clock (Unix ms). Display only.
    pub at_ms: u64,
    pub pending: PendingChange,
    /// The content this write installs has no recorded authorship (a restore,
    /// a migration copy): move the document's record origin up to this write,
    /// so any cutoff before it reads as [`EditorsSince::Incomplete`].
    pub resets_origin: bool,
}

impl EditStamp {
    pub fn new(by: Writer<'_>, at_ms: u64) -> Self {
        EditStamp {
            editors: by.keys(),
            at_ms,
            pending: PendingChange::default(),
            resets_origin: false,
        }
    }
}

/// One editor of one document, coalesced over all its updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorRecord {
    pub kind: EditorKind,
    pub editor: String,
    /// Daemon clock (Unix ms) of the first update. Display only.
    pub first_at_ms: u64,
    /// Daemon clock (Unix ms) of the latest. Display only: a wall clock that
    /// steps backwards can stamp a later edit with an earlier time, so never
    /// filter on this — use `last_wal_id`.
    pub last_at_ms: u64,
    /// WAL id of the latest update this editor's content reached the document
    /// in. The cutoff [`EditorsSince`] filters on.
    pub last_wal_id: u64,
    /// How many updates this editor submitted.
    pub updates: u64,
}

/// The answer to "who edited this document after WAL id N".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorsSince {
    /// Every editor whose latest update is after the cutoff. Empty means
    /// nobody edited it — never "there is no record".
    Known(Vec<EditorRecord>),
    /// The cutoff is past this document's WAL high-water mark, so it was not
    /// taken from this document (WAL ids are global to the database: it came
    /// from another document, or another database). Nothing can be said.
    CutoffAhead {
        /// The largest WAL id this document has been written at.
        high_water: u64,
    },
    /// The cutoff precedes the point from which this document's edits are
    /// recorded (its record origin: the upgrade that introduced the record, or
    /// a restore or migration that installed content of unrecorded
    /// authorship). Who edited it in between cannot be known.
    Incomplete {
        /// Updates after this WAL id are recorded; none before it are.
        recorded_after_wal_id: u64,
    },
}

/// The ADR-113 author verdict for a change since a cutoff. Use this rather
/// than reading [`EditorsSince`] rows directly, so the rule below cannot be
/// forgotten.
///
/// **Assumption (see the module doc):** an author is a principal who submitted
/// or signed an update since the cutoff; content they carried for someone else
/// is attributed to them. Treat `Known` as "these principals are accountable
/// for every change since the cutoff" — a known limit, not proof of who wrote
/// each character.
///
/// **Rule:** any row after the cutoff that is not a principal — an
/// [`EditorKind::Internal`] writer, an unsigned [`EditorKind::MeshPeer`], or an
/// [`EditorKind::Unauthenticated`] session — means some content's author is not
/// known, and so is an incomplete record. Either way the authors are
/// [`Authors::Unknown`] and approval must be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authors {
    /// Every author is a known principal.
    Known(BTreeSet<String>),
    /// At least one change since the cutoff has no known author.
    Unknown(UnknownAuthors),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownAuthors {
    /// The record does not reach back to the cutoff.
    RecordIncomplete { recorded_after_wal_id: u64 },
    /// The cutoff does not belong to this document (see
    /// [`EditorsSince::CutoffAhead`]).
    CutoffAhead { high_water: u64 },
    /// These editors changed the document after the cutoff and name no author.
    NotPrincipals(Vec<EditorRecord>),
}

impl EditorsSince {
    /// Apply the author rule (see [`Authors`]).
    pub fn authors(&self) -> Authors {
        let rows = match self {
            Self::Incomplete {
                recorded_after_wal_id,
            } => {
                return Authors::Unknown(UnknownAuthors::RecordIncomplete {
                    recorded_after_wal_id: *recorded_after_wal_id,
                })
            }
            Self::CutoffAhead { high_water } => {
                return Authors::Unknown(UnknownAuthors::CutoffAhead {
                    high_water: *high_water,
                })
            }
            Self::Known(rows) => rows,
        };
        let hidden: Vec<_> = rows
            .iter()
            .filter(|r| r.kind.hides_an_author())
            .cloned()
            .collect();
        if !hidden.is_empty() {
            return Authors::Unknown(UnknownAuthors::NotPrincipals(hidden));
        }
        Authors::Known(
            rows.iter()
                .filter(|r| r.kind == EditorKind::Principal)
                .map(|r| r.editor.clone())
                .collect(),
        )
    }
}

/// The daemon clock the editor record is stamped with: wall-clock Unix
/// milliseconds. Injectable via `DocStore::with_clock` for tests.
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
            EditorKind::SignedRelay,
            EditorKind::Internal,
        ] {
            assert_eq!(EditorKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(EditorKind::parse(""), None);
        assert_eq!(EditorKind::parse("Principal"), None);
    }

    /// A session with no principal — or an empty one — is never recorded as a
    /// principal, and a P2P session is a relay, never a principal.
    #[test]
    fn session_attribution_follows_principal_and_transport() {
        assert_eq!(
            Attribution::session(Some("fp-a"), Transport::Hub),
            Attribution::Principal("fp-a")
        );
        assert_eq!(
            Attribution::session(Some("fp-a"), Transport::P2p),
            Attribution::MeshPeer("fp-a")
        );
        for p in [None, Some("")] {
            for t in [Transport::Hub, Transport::P2p] {
                assert_eq!(Attribution::session(p, t), Attribution::Unauthenticated);
            }
        }
    }

    /// A signed relay records the relay as `signed-relay` AND the signer as a
    /// principal; an unsigned relay is a bare `mesh-peer`. A signer equal to
    /// the session principal is recorded once.
    #[test]
    fn a_signer_is_recorded_as_a_principal_alongside_the_relay() {
        let k = |e: &str| e.to_string();
        assert_eq!(
            Writer::signed(Attribution::MeshPeer("relay"), Some("alice")).keys(),
            vec![
                (EditorKind::SignedRelay, k("relay")),
                (EditorKind::Principal, k("alice"))
            ]
        );
        assert_eq!(
            Writer::signed(Attribution::MeshPeer("relay"), None).keys(),
            vec![(EditorKind::MeshPeer, k("relay"))]
        );
        assert_eq!(
            Writer::signed(Attribution::Principal("alice"), Some("alice")).keys(),
            vec![(EditorKind::Principal, k("alice"))]
        );
        assert_eq!(
            Writer::signed(Attribution::Principal("bob"), Some("")).keys(),
            vec![(EditorKind::Principal, k("bob"))]
        );
    }

    #[test]
    fn pending_change_reads_both_digests() {
        let (none, a, b) = (None, Some(vec![1]), Some(vec![2]));
        let p = PendingChange::between;
        assert_eq!(p(&none, &none), PendingChange::default());
        assert_eq!(p(&a, &a), PendingChange::default(), "unchanged pending");
        let leave = PendingChange {
            leaves_pending: true,
            ..Default::default()
        };
        assert_eq!(p(&none, &a), leave);
        let drain = PendingChange {
            integrated: true,
            cleared: true,
            ..Default::default()
        };
        assert_eq!(p(&a, &none), drain);
        let shift = PendingChange {
            leaves_pending: true,
            integrated: true,
            cleared: false,
        };
        assert_eq!(p(&a, &b), shift);
    }

    fn row(kind: EditorKind, editor: &str) -> EditorRecord {
        EditorRecord {
            kind,
            editor: editor.to_string(),
            first_at_ms: 0,
            last_at_ms: 0,
            last_wal_id: 1,
            updates: 1,
        }
    }

    /// The consumer rule: principals and signed relays leave the authors known;
    /// any other kind, or an incomplete record, makes them unknown.
    #[test]
    fn the_author_verdict_refuses_whenever_an_author_is_hidden() {
        use EditorKind::*;
        let known = EditorsSince::Known(vec![row(Principal, "a"), row(SignedRelay, "r")]);
        assert_eq!(
            known.authors(),
            Authors::Known(["a".to_string()].into_iter().collect())
        );
        assert_eq!(
            EditorsSince::Known(vec![]).authors(),
            Authors::Known(BTreeSet::new())
        );
        for hidden in [MeshPeer, Internal, Unauthenticated] {
            let rows = EditorsSince::Known(vec![row(Principal, "a"), row(hidden, "x")]);
            assert_eq!(
                rows.authors(),
                Authors::Unknown(UnknownAuthors::NotPrincipals(vec![row(hidden, "x")])),
                "{hidden:?}"
            );
        }
        assert_eq!(
            EditorsSince::Incomplete {
                recorded_after_wal_id: 9
            }
            .authors(),
            Authors::Unknown(UnknownAuthors::RecordIncomplete {
                recorded_after_wal_id: 9
            })
        );
    }
}

#[cfg(test)]
#[path = "doc_editors_tests.rs"]
mod store_tests;
