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
        project_key: None,
        kind: KbInstanceKind::UserRegistered,
        ingest_policy: Default::default(),
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

/// D4 rests on `generate_uuid` being unique, and nothing asserted that before
/// minting became load-bearing for KB identity.
///
/// It now draws 122 bits of OS entropy. It used to derive the id from a
/// nanosecond clock and 16 bits of pid with NO randomness, so uniqueness came
/// entirely from the clock advancing between calls.
///
/// This test's predecessor **failed on macOS in CI** the first time it ran there
/// — `generate_uuid collided within one process` — while 20k mints measured clean
/// on Linux. macOS's `SystemTime::now()` is coarser, so consecutive mints shared a
/// tick. The risk was never the remote cross-machine one it looked like from a
/// Linux box; it was reproducible on a supported platform (CLAUDE.md #13).
///
/// The trailing 48-bit field was dead as well (`ts >> 64` is zero until the year
/// 2554), so every id ended `000000000000`.
#[test]
fn minting_is_unique_and_actually_random() {
    let n = 20_000;
    let ids: std::collections::HashSet<String> = (0..n).map(|_| generate_uuid()).collect();
    assert_eq!(ids.len(), n, "generate_uuid collided within one process");

    // The property that matters is entropy, not merely non-repetition: a counter
    // never repeats either and would still collide across machines. A nearly
    // constant trailing field is exactly the shape the old implementation had,
    // where it was literally always zero.
    let tails: std::collections::HashSet<&str> = ids.iter().map(|s| &s[s.len() - 12..]).collect();
    assert!(
        tails.len() > n / 2,
        "the last field is nearly constant across {n} mints ({} distinct), so the \
         id is not carrying real entropy",
        tails.len()
    );

    for id in ids.iter().take(64) {
        assert_eq!(id.len(), 36, "expected UUID layout: {id}");
        assert_eq!(&id[14..15], "4", "expected version 4: {id}");
        assert!(
            mae_sync::kb_id_is_addressable(id),
            "a minted id becomes part of every node's address (D3): {id}"
        );
    }
}

/// Layer one of three, and the only deterministic one: a mint never hands out an
/// id already used in THIS registry. Probability covers the cross-machine case and
/// the daemon catches what is left, but a local collision needs no network to
/// detect and so should not be left to chance at all.
#[test]
fn minting_never_reuses_an_id_already_in_this_registry() {
    let mut reg = KbRegistry::default();
    let uuid = instance(&mut reg, "notes");
    let first = reg.collab_id_for_share(&KbTarget::Instance(uuid));
    let primary = reg.collab_id_for_share(&KbTarget::Primary);
    assert_ne!(first, primary);
    assert_eq!(reg.target_of_collab_id(&primary), Some(KbTarget::Primary));
}

/// The recovery `remint_unconfirmed_collab_id` exists for: an id the daemon
/// refused is replaced, so the KB stays shareable. Without it,
/// `collab_id_for_share` — correctly — returns the refused id forever and the KB
/// is unshareable until someone hand-edits `kb-registry.toml`.
#[test]
fn an_unconfirmed_id_can_be_reminted() {
    let mut reg = KbRegistry::default();
    let taken = reg.collab_id_for_share(&KbTarget::Primary);
    assert!(!reg.primary_shared, "precondition: nothing confirmed");

    let fresh = reg
        .remint_unconfirmed_collab_id(&KbTarget::Primary)
        .expect("an unconfirmed id is not ours and must be replaceable");
    assert_ne!(fresh, taken);
    assert_eq!(reg.primary_collab_id.as_deref(), Some(fresh.as_str()));
    assert_eq!(
        reg.collab_id_for_share(&KbTarget::Primary),
        fresh,
        "the next share must go out under the new id"
    );
}

/// The control, and the dangerous direction: a CONFIRMED share's id is write-once
/// no matter what the daemon says. Re-minting it would evaporate the KB's signed
/// membership and make an E2E KB read as plaintext (finding A) — worse than any
/// failed share. Asserted for both target shapes, since the primary and an
/// instance store that marker in different places.
#[test]
fn a_confirmed_ids_remint_is_refused_for_both_target_shapes() {
    let mut reg = KbRegistry::default();
    let before = reg.collab_id_for_share(&KbTarget::Primary);
    reg.primary_shared = true;
    assert_eq!(reg.remint_unconfirmed_collab_id(&KbTarget::Primary), None);
    assert_eq!(reg.primary_collab_id.as_deref(), Some(before.as_str()));

    let uuid = instance(&mut reg, "notes");
    let t = KbTarget::Instance(uuid.clone());
    let inst_before = reg.collab_id_for_share(&t);
    reg.find_mut(&uuid).unwrap().shared = true;
    assert_eq!(reg.remint_unconfirmed_collab_id(&t), None);
    assert_eq!(
        reg.collab_id_of_target(&t).as_deref(),
        Some(inst_before.as_str())
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
