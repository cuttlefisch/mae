//! ADR-116 P1, retention: the floors are enforced, the default prunes nothing,
//! and no policy ever offers a held, named, revision-referenced, latest,
//! too-young or already-pruned version.

use std::collections::BTreeSet;

use proptest::prelude::*;

use super::*;
use crate::kb::history::*;

/// The test KB's history key.
fn key() -> HistoryKey {
    HistoryKey::from_bytes([5; 32])
}

fn seal_k(
    kb: &str,
    node: &str,
    prev: Option<&HistoryEntry>,
    d: Draft,
) -> Result<HistoryEntry, SealError> {
    seal(kb, node, prev, d, &key())
}

const DAY: i64 = 86_400;
const NOW: i64 = 1_800_000_000;

/// `n` versions of one node, version `seq` made `n - seq` days before `NOW`,
/// of the kinds `kind_of` picks.
fn history(n: u64, kind_of: impl Fn(u64) -> VersionKind) -> Vec<HistoryEntry> {
    let mut out: Vec<HistoryEntry> = Vec::new();
    for seq in 1..=n {
        let kind = match kind_of(seq) {
            VersionKind::Rollback if seq == 1 => VersionKind::Edit,
            k => k,
        };
        let d = Draft {
            kind,
            created_at: NOW - (n - seq) as i64 * DAY,
            editors: Vec::new(),
            wal_from: seq,
            wal_to: seq,
            summary: None,
            name: (kind == VersionKind::Named).then(|| format!("v{seq}")),
            restored_from: (kind == VersionKind::Rollback).then_some(1),
            content: (kind != VersionKind::Pruned)
                .then(|| KbNodeDoc::new("n", &format!("t{seq}"), "", &[]).materialize()),
        };
        let e = seal_k("kb", "n", out.last(), d).unwrap();
        out.push(e);
    }
    out
}

fn versions(max_versions: u32, max_days: Option<u32>) -> Policy {
    Policy {
        min_days: 30,
        keep: Keep::Versions {
            max_versions,
            max_days,
        },
    }
}

#[test]
fn the_floors_are_enforced_and_the_default_keeps_all() {
    assert_eq!(
        Policy::default(),
        Policy {
            min_days: 30,
            keep: Keep::All
        }
    );
    assert!(Policy::default().validate().is_ok());
    assert_eq!(
        Policy {
            min_days: 29,
            keep: Keep::All
        }
        .validate(),
        Err(PolicyError::MinDaysBelowFloor(29))
    );
    assert_eq!(
        versions(99, None).validate(),
        Err(PolicyError::MaxVersionsBelowFloor(99))
    );
    assert!(versions(100, Some(30)).validate().is_ok());
    assert_eq!(
        versions(100, Some(29)).validate(),
        Err(PolicyError::MaxDaysBelowMinDays {
            max_days: 29,
            min_days: 30
        })
    );
    let h = history(400, |_| VersionKind::Edit);
    let none = Exemptions::default();
    assert_eq!(
        prune_candidates(&Policy::default(), &h, &none, NOW),
        Ok(Vec::new())
    );
    assert!(prune_candidates(&versions(5, None), &h, &none, NOW).is_err());
}

/// One worked case, by hand: 150 daily versions, version `seq` aged
/// `150 - seq` days. Keep 100: the oldest 50 go, except the named one (10) and
/// the one a revision references (20). With `max_days: 60` too, everything
/// older than 60 days goes (seq < 90), with the same two exceptions.
#[test]
fn a_worked_case() {
    let h = history(150, |s| {
        if s == 10 {
            VersionKind::Named
        } else {
            VersionKind::Edit
        }
    });
    let ex = Exemptions {
        held: false,
        revision_refs: BTreeSet::from([20]),
    };
    let expect = |upto: u64| -> Vec<u64> { (1..upto).filter(|s| *s != 10 && *s != 20).collect() };
    assert_eq!(
        prune_candidates(&versions(100, None), &h, &ex, NOW).unwrap(),
        expect(51)
    );
    assert_eq!(
        prune_candidates(&versions(100, Some(60)), &h, &ex, NOW).unwrap(),
        expect(90)
    );
    let held = Exemptions { held: true, ..ex };
    assert_eq!(
        prune_candidates(&versions(100, Some(60)), &h, &held, NOW).unwrap(),
        Vec::<u64>::new()
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the policy and history, a candidate is never exempt, and every
    /// version a `max_versions` rule leaves over is a candidate unless exempt.
    #[test]
    fn candidates_are_never_exempt_and_the_excess_goes(
        n in 1u64..260,
        kinds in prop::collection::vec(0usize..8, 260),
        max_versions in 100u32..200,
        max_days in prop::option::of(30u32..400),
        min_days in 30u32..120,
        refs in prop::collection::btree_set(1u64..260, 0..20),
        pruned in prop::collection::btree_set(1u64..260, 0..20),
    ) {
        let max_days = max_days.map(|d| d.max(min_days));
        let mut h = history(n, |s| VersionKind::ALL[kinds[s as usize - 1]]);
        for e in &mut h {
            if pruned.contains(&e.seq) { e.prune(); }
        }
        let policy = Policy { min_days, keep: Keep::Versions { max_versions, max_days } };
        let ex = Exemptions { held: false, revision_refs: refs.clone() };
        let got: BTreeSet<u64> = prune_candidates(&policy, &h, &ex, NOW).unwrap().into_iter().collect();
        let vs: Vec<&HistoryEntry> = h.iter().filter(|e| e.kind != VersionKind::Pruned).collect();
        let age_ok = |e: &HistoryEntry| NOW - e.created_at > i64::from(min_days) * DAY;
        for e in h.iter().filter(|e| got.contains(&e.seq)) {
            prop_assert!(e.kind != VersionKind::Named && e.kind != VersionKind::Pruned, "{:?}", e.kind);
            prop_assert!(!refs.contains(&e.seq) && !e.is_pruned() && age_ok(e));
            prop_assert!(Some(e.seq) != vs.last().map(|l| l.seq), "the latest version");
        }
        let excess = vs.len().saturating_sub(max_versions as usize);
        for (i, e) in vs.iter().enumerate().take(excess) {
            let exempt = e.kind == VersionKind::Named || refs.contains(&e.seq) || e.is_pruned()
                || !age_ok(e) || i + 1 == vs.len();
            prop_assert_eq!(got.contains(&e.seq), !exempt, "seq {}", e.seq);
        }
        let held = Exemptions { held: true, revision_refs: refs };
        prop_assert!(prune_candidates(&policy, &h, &held, NOW).unwrap().is_empty());
    }
}

/// The two exemptions age alone does not give: the latest version is kept
/// even when it is old and over the count, and versions over the count are
/// kept while younger than `min_days`. Kills a pass that prunes the latest
/// version, or one that ignores `min_days`.
#[test]
fn the_latest_and_the_young_are_kept_whatever_the_count() {
    let mut old = history(150, |_| VersionKind::Edit);
    for e in &mut old {
        e.created_at -= 400 * DAY;
    }
    let all_old = prune_candidates(&versions(100, Some(60)), &old, &Exemptions::default(), NOW);
    assert_eq!(all_old.unwrap(), (1..150).collect::<Vec<u64>>());
    let mut young = history(150, |_| VersionKind::Edit);
    for e in &mut young {
        e.created_at = NOW - DAY + e.seq as i64;
    }
    let none = prune_candidates(&versions(100, None), &young, &Exemptions::default(), NOW);
    assert_eq!(none.unwrap(), Vec::<u64>::new());
}
