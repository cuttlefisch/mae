//! ADR-105 D4/H3: KB identity — a collab id is not a name, and the primary KB is
//! not one of `instances`.
//!
//! Split out of `kb_identity.rs` to keep both under the structural ceiling
//! (`make audit-metrics-check`); `#[path]`-included so it stays an inner module
//! with access to this crate's private items.

use super::*;
use crate::federation::{generate_uuid, AiResidency, KbInstance, KbInstanceKind, KbRegistry};
use std::path::PathBuf;

/// A registered, not-yet-shared instance. Spelled out rather than defaulted
/// because `KbInstance` has no `Default` — and the two fields that matter here
/// (`collab_id: None`, `shared: false`) are exactly the ones a `..default()`
/// would hide.
fn instance(reg: &mut KbRegistry, name: &str) -> String {
    let uuid = generate_uuid();
    reg.instances.push(KbInstance {
        uuid: uuid.clone(),
        name: name.to_string(),
        org_dir: PathBuf::from("/tmp/kb-identity-test"),
        db_path: PathBuf::from("/tmp/kb-identity-test.db"),
        primary: false,
        enabled: true,
        last_import: None,
        collab_id: None,
        shared: false,
        remote_peers: Vec::new(),
        last_sync: None,
        ai_residency: AiResidency::default(),
        project_root: None,
        kind: KbInstanceKind::UserRegistered,
        priority: 0,
        remote_hub: None,
    });
    uuid
}

/// The property finding A makes non-negotiable: a KB's collab id is
/// write-once. `kb_id` is signed into every `MembershipOp`, so re-minting one
/// destroys the KB's membership and silently downgrades an E2E KB to
/// plaintext. Asserted for BOTH target shapes — the primary is the one that
/// already had a value baked in (`"default"`), so it is the one most likely to
/// be "corrected" to a uuid by a later well-meaning change.
#[test]
fn an_existing_collab_id_is_never_reminted() {
    let mut reg = KbRegistry {
        primary_collab_id: Some("default".to_string()),
        ..Default::default()
    };
    assert_eq!(reg.collab_id_for_share(&KbTarget::Primary), "default");
    assert_eq!(
        reg.primary_collab_id.as_deref(),
        Some("default"),
        "a KB shared before D4 keeps its name-id; re-minting evaporates its \
         signed membership and reads an E2E KB as plaintext"
    );

    let uuid = instance(&mut reg, "notes");
    reg.find_mut("notes").unwrap().collab_id = Some("notes".to_string());
    let t = KbTarget::Instance(uuid);
    assert_eq!(reg.collab_id_for_share(&t), "notes");
    assert_eq!(reg.collab_id_of_target(&t).as_deref(), Some("notes"));
}

/// Finding F, stated as a property rather than a scenario: two editors' primary
/// KBs must not claim the same collab id. Both are named `"default"` — that is
/// the whole point — so a name-derived id collides by construction and the
/// second tenant's share is accepted and then denied on every operation.
#[test]
fn two_editors_primaries_mint_distinct_ids() {
    let mut alice = KbRegistry::default();
    let mut bob = KbRegistry::default();

    let a = alice.collab_id_for_share(&KbTarget::Primary);
    let b = bob.collab_id_for_share(&KbTarget::Primary);

    assert_ne!(
        a, b,
        "two editors' primaries collided on one collab id — finding F, the \
         bug that made a shared daemon single-tenant in practice"
    );
    for id in [&a, &b] {
        assert!(
            !PRIMARY_NAME_ALIASES.contains(&id.as_str()),
            "a minted id must not be the display name it replaces: {id}"
        );
        assert!(
            mae_sync::kb_id_is_addressable(id),
            "a minted id becomes part of every node's address (D3): {id}"
        );
    }
}

/// The resolution H3 demands. The name comparison it replaces answers `false`
/// for a minted-id primary, and does so SILENTLY — the caller falls through to
/// the instance branch, finds nothing, and treats a live shared KB as unknown.
#[test]
fn a_minted_id_still_resolves_back_to_the_primary() {
    let mut reg = KbRegistry::default();
    let minted = reg.collab_id_for_share(&KbTarget::Primary);

    assert_eq!(reg.target_of_collab_id(&minted), Some(KbTarget::Primary));
    assert!(
        !PRIMARY_NAME_ALIASES.contains(&minted.as_str()),
        "precondition: the id under test must NOT be a name, or this test \
         passes through the very comparison it exists to replace"
    );
}

/// A KB shared before D4 resolves through the same path with no special case —
/// which is what makes D4 safe to land without a migration.
#[test]
fn a_legacy_name_id_resolves_with_no_special_case() {
    let mut reg = KbRegistry {
        primary_collab_id: Some("default".to_string()),
        ..Default::default()
    };
    assert_eq!(reg.target_of_collab_id("default"), Some(KbTarget::Primary));

    let uuid = instance(&mut reg, "notes");
    reg.find_mut("notes").unwrap().collab_id = Some("collabtest".to_string());
    assert_eq!(
        reg.target_of_collab_id("collabtest"),
        Some(KbTarget::Instance(uuid))
    );
}

/// An unknown id resolves to nothing rather than defaulting to the primary.
/// The failure this guards is specific: "unknown ⇒ primary" would route
/// another tenant's KB into this editor's own primary.
#[test]
fn an_unknown_collab_id_resolves_to_nothing() {
    let mut reg = KbRegistry {
        primary_collab_id: Some("default".to_string()),
        ..Default::default()
    };
    instance(&mut reg, "notes");

    assert_eq!(reg.target_of_collab_id("someone-elses-kb"), None);
    // A registered instance that is not SHARED has no collab id, so its name
    // must not resolve as one either.
    assert_eq!(reg.target_of_collab_id("notes"), None);
}

/// D4 rests on `generate_uuid` actually being unique, and nothing asserted
/// that before minting became load-bearing for KB identity.
///
/// Its entropy is a nanosecond timestamp plus the low 16 bits of the pid —
/// no randomness at all — so within one process uniqueness comes entirely
/// from the clock advancing between calls. That holds on a nanosecond clock
/// (measured: 20k mints, zero duplicates) and is the case that matters here,
/// since a single editor sharing several KBs mints them from one process with
/// a fixed pid.
///
/// Two DIFFERENT machines minting in the same nanosecond with matching
/// low-16-bit pids would still collide. Left as-is deliberately: making the
/// mint random means a new direct dependency (`uuid`/`rand` reach this crate
/// only transitively, via cozo) and would change every instance uuid too, and
/// D5 already turns a duplicate id into a named refusal rather than the silent
/// merge finding E describes. Recorded so the trade-off is a decision rather
/// than an oversight.
#[test]
fn minting_is_unique_within_a_process() {
    let n = 20_000;
    let ids: std::collections::HashSet<String> = (0..n).map(|_| generate_uuid()).collect();
    assert_eq!(
        ids.len(),
        n,
        "generate_uuid collided within one process; KB identity (D4) and every \
         instance uuid rest on this"
    );
}

/// Names still work where names belong — and only there.
#[test]
fn names_resolve_for_the_human_facing_direction() {
    let mut reg = KbRegistry::default();
    let uuid = instance(&mut reg, "notes");

    for alias in PRIMARY_NAME_ALIASES {
        assert_eq!(reg.target_of_name(alias), Some(KbTarget::Primary));
    }
    assert_eq!(reg.target_of_name("PRIMARY"), Some(KbTarget::Primary));
    assert_eq!(
        reg.target_of_name("notes"),
        Some(KbTarget::Instance(uuid.clone()))
    );
    assert_eq!(reg.target_of_name(&uuid), Some(KbTarget::Instance(uuid)));
    assert_eq!(
        reg.target_of_name("no-such-kb"),
        None,
        "a typo must surface as 'no such KB', never resolve to the primary"
    );
}
