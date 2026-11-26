//! ADR-036 Amendment 1: the hub verifies every content op it accepts (#727).
//!
//! One check for every path that writes a KB node document — `kb/node_update`
//! and `sync/update` from a session, over every transport that reaches them,
//! and the mesh relay (the dialer) — so no path can verify less than another:
//!
//! 1. **Unsigned** (no `sig`): refused `signature_required` when the store's
//!    policy requires signatures on this transport (always on the mesh; on the
//!    hub once `require_signed_content_ops` is on), else accepted and counted
//!    (F6).
//! 2. **Signature** (F1): `author_pubkey` must hash to `author` and the
//!    signature must verify over D1's canonical bytes, re-bound to the KB and
//!    node the daemon will write — before anything else is read.
//! 3. **Authorship** (F4): an op a session originates is authored by that
//!    session's principal (`not_author`). A mesh peer relays others' ops, so
//!    `Transport::P2p` skips this.
//! 4. **Membership** (F2): the author is an Editor+ member at the op's epoch,
//!    read from the source `kb_access` decides on ([`signed_membership_anchor`]):
//!    the signed op-log on a KB whose log is rooted at a genesis its anchor
//!    signed, the local roster otherwise.
//!
//! What this does NOT cover, tracked by the amendment: collection documents
//! (`kbc:`, F3) and the mesh join snapshot (F3, its own amendment).

use super::*;
use mae_sync::membership::ValidMember;
use std::collections::BTreeMap;

/// Who handed the daemon a content op.
#[derive(Clone, Copy, Debug)]
pub struct OpSender<'a> {
    /// The session's authenticated principal; `None` for a `none`-mode session.
    pub principal: Option<&'a str>,
    pub transport: Transport,
}

impl<'a> OpSender<'a> {
    /// An op arriving on a session (a client, or a mesh peer on the P2P listener).
    pub fn session(principal: Option<&'a str>, transport: Transport) -> Self {
        Self {
            principal,
            transport,
        }
    }

    /// A mesh peer relaying ops it did not author (the dialer's inbound path).
    pub fn mesh_relay() -> Self {
        Self {
            principal: None,
            transport: Transport::P2p,
        }
    }

    /// F4: the principal every op this sender ORIGINATES must be authored by.
    /// A mesh peer's ops are legitimately someone else's, so it has none.
    ///
    /// @ai-caution: [security] A hub session with no principal skips F4. Only a
    /// session with no identity at all has none: `auth.mode = none`
    /// (`handle_client`, and `PeerIdentity::synthetic`). Every identified
    /// session has one — a key or mTLS peer its fingerprint, a `/ws` bearer
    /// its token subject, psk the shared `psk:<keyid>` (so a psk session's
    /// signed ops are always `not_author`: psk clients do not sign). Such a
    /// session is already loopback-trusted for every KB, so F4 would bound
    /// nothing; signature and membership still apply. Should an identified
    /// session ever reach `principal() == None`, it would pass here AND every
    /// `kb_access` gate — fix that at `PeerIdentity::principal`, not here.
    fn originator(&self) -> Option<&'a str> {
        match self.transport {
            Transport::P2p => None,
            Transport::Hub => self.principal,
        }
    }
}

/// Verify the content op `header` carries for `update` to `(kb_id, node_id)`.
/// `Ok(Some)` is a verified signed op, `Ok(None)` an unsigned op this policy
/// accepts (and has counted); any refusal is the caller's to surface.
pub async fn verify_node_op(
    doc_store: &DocStore,
    (kb_id, node_id): (&str, &str),
    update: &[u8],
    header: Option<&serde_json::Value>,
    sender: OpSender<'_>,
    coll: Option<&KbCollectionDoc>,
) -> Result<Option<SignedContentOp>, Refusal> {
    let Some(header) = header.filter(|h| h.get("sig").is_some()) else {
        return accept_unsigned(doc_store, (kb_id, node_id), sender.transport);
    };
    let signed = signed_op(header, (kb_id, node_id), update)?;
    if !signed.verify_signed() {
        return Err(Refusal::new(
            RefusalKind::SignatureInvalid,
            "signed content op rejected: the signature does not verify, or its key is not the author's",
        ));
    }
    refuse_foreign_author(&signed, sender)?;
    verify_content_op_with_coll(doc_store, kb_id, &signed, coll).await?;
    Ok(Some(signed))
}

/// ADR-036 Amendment 2 (G1, G2): the signed header of a node-list change
/// (`kb/collection_node_add` / `_remove`), checked exactly as a content op's:
/// unsigned is refused or counted under the same switch; a signed one must
/// verify, be authored by the session (F4), and come from a member with
/// `Edit` at its own epoch (F2, ADR-023). `Some(author)` when it was signed.
pub(crate) async fn verify_manifest_op(
    doc_store: &DocStore,
    change: (&str, &str, mae_sync::manifest_op::ManifestAction, &str),
    params: &serde_json::Value,
    sender: OpSender<'_>,
    coll: &KbCollectionDoc,
) -> Result<Option<String>, Refusal> {
    let (kb_id, node_id, action, title) = change;
    if params.get("sig").is_none() {
        return accept_unsigned(doc_store, (kb_id, node_id), sender.transport).map(|_| None);
    }
    let malformed = || {
        Refusal::new(
            RefusalKind::SignatureInvalid,
            "malformed signed manifest op header",
        )
    };
    let (author, pubkey, epoch, issued_at, sig) = manifest_header(params).ok_or_else(malformed)?;
    let op = mae_sync::manifest_op::ManifestOp {
        kb_id: kb_id.to_string(),
        node_id: node_id.to_string(),
        action,
        title: title.to_string(),
        author,
        epoch,
        issued_at,
    };
    if !op.verify(&sig, &pubkey) {
        return Err(Refusal::new(
            RefusalKind::SignatureInvalid,
            "signed manifest op rejected: the signature does not verify, or its key is not the author's",
        ));
    }
    if let Some(principal) = sender.originator() {
        if principal != op.author {
            return Err(Refusal::new(
                RefusalKind::NotAuthor,
                format!(
                    "manifest op signed by {} sent by session {principal}: a session may send only \
                     ops it authored (ADR-036 Amendment 1 F4)",
                    op.author
                ),
            ));
        }
    }
    let members = author_membership(doc_store, kb_id, coll, &op.author).await;
    let Some(member) = members.get(&op.author) else {
        return Err(Refusal::new(
            RefusalKind::NotMember,
            "signed manifest op rejected: not a member",
        ));
    };
    if !member.role.includes(SyncRole::Editor) {
        return Err(Refusal::new(
            RefusalKind::RoleForbidden {
                required: SyncRole::Editor,
            },
            "signed manifest op rejected: changing the node list needs the editor role",
        ));
    }
    if member.epoch != op.epoch {
        return Err(Refusal::new(
            RefusalKind::EpochFenced {
                current_epoch: member.epoch,
                caller_epoch: Some(op.epoch),
            },
            "signed manifest op rejected: a stale epoch",
        ));
    }
    Ok(Some(op.author))
}

/// ADR-036 Amendment 2 (G3): once `require_signed_content_ops` is on, a raw
/// write to KB `kb_id`'s collection (an update merged into the current state)
/// may not add, remove or retitle a node. The node list changes only through a
/// signed manifest op. A collection that does not exist yet is a creation,
/// not a change, and is left to its own path.
pub(crate) async fn refuse_raw_node_list_change(
    doc_store: &DocStore,
    kb_id: &str,
    update: &[u8],
    transport: Transport,
) -> Result<(), Refusal> {
    if !doc_store.content_op_policy().require_signed(transport) {
        return Ok(());
    }
    let name = format!("kbc:{kb_id}");
    if !doc_store.has_durable_doc(&name).await {
        return Ok(());
    }
    let unreadable = || {
        Refusal::new(
            RefusalKind::SignatureRequired,
            "the collection could not be checked",
        )
    };
    let state = doc_store
        .encode_state(&name)
        .await
        .map_err(|_| unreadable())?;
    let list =
        |c: &KbCollectionDoc| -> BTreeMap<String, String> { c.list_nodes().into_iter().collect() };
    let before = KbCollectionDoc::from_bytes(&state).map_err(|_| unreadable())?;
    let mut after = KbCollectionDoc::from_bytes(&state).map_err(|_| unreadable())?;
    after.apply_update(update).map_err(|_| unreadable())?;
    if list(&before) == list(&after) {
        return Ok(());
    }
    Err(Refusal::new(
        RefusalKind::SignatureRequired,
        format!(
            "a raw write to KB '{kb_id}''s collection may not change its node list: add or remove a \
             node with kb/collection_node_add or kb/collection_node_remove and a signed manifest op \
             (ADR-036 Amendment 2)"
        ),
    ))
}

/// A manifest op header's fields: author, key, epoch (a number, or canonical
/// decimal text as a browser sends it), issue time, signature.
/// [`refuse_raw_node_list_change`] for a write named by document: only a KB
/// collection document is checked (`sync/update` of any document).
pub(crate) async fn refuse_raw_collection_list_change(
    doc_store: &DocStore,
    doc_name: &str,
    update: &[u8],
    transport: Transport,
) -> Result<(), Refusal> {
    match mae_sync::DocAddress::parse(doc_name) {
        Some(mae_sync::DocAddress::KbCollection { kb_id }) => {
            refuse_raw_node_list_change(doc_store, &kb_id, update, transport).await
        }
        _ => Ok(()),
    }
}

/// A manifest op header: author, key, epoch, issue time, signature.
type ManifestHeader = (String, [u8; 32], u64, u64, Vec<u8>);

fn manifest_header(p: &serde_json::Value) -> Option<ManifestHeader> {
    let b64 = |k: &str| mae_sync::encoding::base64_to_update(p.get(k)?.as_str()?).ok();
    let epoch = match p.get("epoch")? {
        serde_json::Value::String(t) => {
            let canonical = !t.is_empty()
                && t.bytes().all(|b| b.is_ascii_digit())
                && (t == "0" || !t.starts_with('0'));
            canonical.then(|| t.parse::<u64>().ok()).flatten()?
        }
        v => v.as_u64()?,
    };
    Some((
        p.get("author")?.as_str()?.to_string(),
        b64("author_pubkey")?.try_into().ok()?,
        epoch,
        p.get("issued_at")?.as_u64()?,
        b64("sig")?,
    ))
}

/// Step 1: an unsigned op, refused or counted.
fn accept_unsigned(
    doc_store: &DocStore,
    (kb_id, node_id): (&str, &str),
    transport: Transport,
) -> Result<Option<SignedContentOp>, Refusal> {
    let policy = doc_store.content_op_policy();
    if policy.require_signed(transport) {
        return Err(Refusal::new(
            RefusalKind::SignatureRequired,
            format!(
                "unsigned content op for KB '{kb_id}' node '{node_id}' rejected: this daemon \
                 requires signed content ops (ADR-036 require-signed)"
            ),
        ));
    }
    // Counted, not merely permitted: the number that decides when the switch
    // can go on (see `content_op_policy`).
    policy.note_unsigned_accepted();
    Ok(None)
}

/// The signed op, its `kb_id`/`node_id` taken from trusted local context rather
/// than the header, so a header signed for another node fails to verify. A
/// header that has a `sig` but does not parse is invalid, never "unsigned".
fn signed_op(
    header: &serde_json::Value,
    (kb_id, node_id): (&str, &str),
    update: &[u8],
) -> Result<SignedContentOp, Refusal> {
    let mut params = header.clone();
    params["kb_id"] = serde_json::json!(kb_id);
    params["node_id"] = serde_json::json!(node_id);
    SignedContentOp::from_params(&params, update.to_vec()).ok_or_else(|| {
        Refusal::new(
            RefusalKind::SignatureInvalid,
            "malformed signed content op header",
        )
    })
}

/// Step 3 (F4): a session cannot author for another member.
fn refuse_foreign_author(signed: &SignedContentOp, sender: OpSender<'_>) -> Result<(), Refusal> {
    match sender.originator() {
        Some(principal) if principal != signed.op.author => Err(Refusal::new(
            RefusalKind::NotAuthor,
            format!(
                "content op signed by {} sent by session {principal}: a session may send only \
                 ops it authored (ADR-036 Amendment 1 F4)",
                signed.op.author
            ),
        )),
        _ => Ok(()),
    }
}

/// Step 4 (F2): `author`'s entry in the membership `kb_access` reads — at most
/// one entry, since admission looks up only the author.
pub(crate) async fn author_membership(
    doc_store: &DocStore,
    kb_id: &str,
    coll: &KbCollectionDoc,
    author: &str,
) -> BTreeMap<String, ValidMember> {
    let entry = match signed_membership_anchor(doc_store, kb_id, coll).await {
        Some(anchor) => doc_store
            .derived_membership(kb_id, coll, &anchor, now_unix())
            .await
            .members
            .get(author)
            .cloned(),
        None => coll.role_of(author).map(|role| ValidMember {
            principal: author.to_string(),
            role,
            can_invite: false,
            invited_by: coll.owner(),
            epoch: coll.epoch_of(author),
            replication: ReplicationPolicy::Full,
        }),
    };
    entry.map(|m| (author.to_string(), m)).into_iter().collect()
}

/// F1: anchor `kb_id` to its owner's key, once. Called where an owner shows
/// itself: at `kb/share` (the first share, and an owner re-sharing a KB shared
/// before the amendment) and before a `kb/node_update`/`sync/update` write. A
/// no-op unless the session has a key, the KB has no anchor yet, and the key's
/// fingerprint is the one the collection names as owner — so no other key can
/// anchor a KB. Best-effort: a KB that cannot be loaded or persisted here is
/// anchored at the owner's next try.
///
/// Returns, when this call recorded the anchor and the KB's signed log now
/// governs, the member-list entries that log does not admit — warned about
/// here, so no member is lost silently (the owner's "log wins, made visible").
pub(crate) async fn adopt_owner_anchor(
    doc_store: &DocStore,
    kb_id: &str,
    coll: Option<&KbCollectionDoc>,
    pubkey: Option<&[u8; 32]>,
) -> Vec<String> {
    let Some(pubkey) = pubkey else {
        return Vec::new();
    };
    if doc_store.kb_anchor(kb_id).await.is_some() {
        return Vec::new();
    }
    let loaded;
    let coll = match coll {
        Some(c) => c,
        None => match load_collection(doc_store, kb_id).await {
            Ok(c) => {
                loaded = c;
                &loaded
            }
            Err(_) => return Vec::new(),
        },
    };
    let owner = coll.owner();
    if owner.is_empty() || fingerprint_of(pubkey) != owner {
        return Vec::new();
    }
    match doc_store.record_kb_anchor_if_absent(kb_id, *pubkey).await {
        Ok(true) => {
            info!(kb_id, owner = %owner, "KB content anchor recorded (ADR-036 Amendment 1 F1)");
            let roster_only = membership_source(doc_store, kb_id, coll).await.roster_only;
            warn_roster_only(kb_id, &roster_only);
            roster_only
        }
        Ok(false) => Vec::new(),
        Err(e) => {
            warn!(kb_id, error = %e, "KB content anchor not recorded; retried at the owner's next write");
            Vec::new()
        }
    }
}

/// What `mae-daemon kb anchor` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorAnchor {
    /// Recorded. `roster_only`: member-list entries the KB's signed log, which
    /// now governs, does not admit (empty when the member list still governs).
    Recorded {
        roster_only: Vec<String>,
    },
    AlreadyAnchored,
}

/// `mae-daemon kb anchor` (F1): the operator anchors a KB shared before the
/// amendment whose owner will not reconnect. The same rule as an owner's
/// connection: only the key whose fingerprint the collection names as owner,
/// and only once — a different key than the recorded one is refused.
///
/// @ai-caution: [security] Operator-only, offline, under the store lock, like
/// `kb member`: an anchor decides where a KB's membership is read from. Never
/// an MCP tool or an RPC an agent principal can reach (CLAUDE.md principle #16).
pub async fn operator_anchor_kb(
    doc_store: &DocStore,
    kb_id: &str,
    pubkey: [u8; 32],
) -> Result<OperatorAnchor, String> {
    let coll = load_collection(doc_store, kb_id)
        .await
        .map_err(|e| format!("KB '{kb_id}': {}", McpError::from(e).message))?;
    let (owner, key_fp) = (coll.owner(), fingerprint_of(&pubkey));
    if owner.is_empty() || key_fp != owner {
        return Err(format!(
            "the key's fingerprint {key_fp} is not KB '{kb_id}''s owner ({owner}); only the \
             owner's key anchors a KB"
        ));
    }
    match doc_store.kb_anchor(kb_id).await {
        Some(existing) if existing == pubkey => Ok(OperatorAnchor::AlreadyAnchored),
        Some(_) => Err(format!(
            "KB '{kb_id}' is already anchored to another key; an anchor is recorded once"
        )),
        None => match doc_store.record_kb_anchor_if_absent(kb_id, pubkey).await {
            Ok(true) => {
                let roster_only = membership_source(doc_store, kb_id, &coll).await.roster_only;
                warn_roster_only(kb_id, &roster_only);
                Ok(OperatorAnchor::Recorded { roster_only })
            }
            Ok(false) => Err(format!(
                "KB '{kb_id}' was anchored concurrently; re-run to check"
            )),
            Err(e) => Err(format!("anchor not recorded: {e}")),
        },
    }
}
