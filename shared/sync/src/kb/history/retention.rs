//! Retention (D5): which versions a policy lets a pruning pass drop the
//! content of. The pass itself, its bound per tick and its audit entry are
//! the store's; this decides only which.

use std::collections::BTreeSet;

use super::{HistoryEntry, VersionKind};

/// Content younger than this many days is never pruned, whatever the policy.
pub const MIN_DAYS_FLOOR: u32 = 30;
/// A `max_versions` rule keeps at least this many versions per node.
pub const MAX_VERSIONS_FLOOR: u32 = 100;

const DAY: i64 = 86_400;

/// Which versions a node keeps beyond the floors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// Prune nothing (the default: nothing goes until a policy is set).
    All,
    /// Keep the newest `max_versions` per node. With `max_days`, a version
    /// older than that is prunable even among the newest `max_versions`.
    Versions {
        max_versions: u32,
        max_days: Option<u32>,
    },
}

/// A KB's retention policy. Set by its owner or the operator, never an agent
/// principal: that check is the caller's, since only it knows who is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub min_days: u32,
    pub keep: Keep,
}

impl Default for Policy {
    /// `keep: all`, `min_days: 30` (decided 2026-11-11).
    fn default() -> Self {
        Policy {
            min_days: MIN_DAYS_FLOOR,
            keep: Keep::All,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    MinDaysBelowFloor(u32),
    MaxVersionsBelowFloor(u32),
    /// `max_days` under `min_days` would promise a pruning `min_days` forbids.
    MaxDaysBelowMinDays {
        max_days: u32,
        min_days: u32,
    },
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MinDaysBelowFloor(d) => {
                write!(f, "min_days {d} is below the floor of {MIN_DAYS_FLOOR}")
            }
            Self::MaxVersionsBelowFloor(n) => {
                write!(
                    f,
                    "max_versions {n} is below the floor of {MAX_VERSIONS_FLOOR}"
                )
            }
            Self::MaxDaysBelowMinDays { max_days, min_days } => {
                write!(f, "max_days {max_days} is below min_days {min_days}")
            }
        }
    }
}

impl Policy {
    /// Refuses a policy under the floors, or one that contradicts itself.
    pub fn validate(&self) -> Result<(), PolicyError> {
        if self.min_days < MIN_DAYS_FLOOR {
            return Err(PolicyError::MinDaysBelowFloor(self.min_days));
        }
        if let Keep::Versions {
            max_versions,
            max_days,
        } = self.keep
        {
            if max_versions < MAX_VERSIONS_FLOOR {
                return Err(PolicyError::MaxVersionsBelowFloor(max_versions));
            }
            if let Some(d) = max_days.filter(|&d| d < self.min_days) {
                return Err(PolicyError::MaxDaysBelowMinDays {
                    max_days: d,
                    min_days: self.min_days,
                });
            }
        }
        Ok(())
    }
}

/// What a policy may not prune in one node, beyond what its entries say
/// (named versions and the latest version are read from the entries).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exemptions {
    /// A hold on the KB or on this node.
    pub held: bool,
    /// The `seq`s an ADR-113 revision references.
    pub revision_refs: BTreeSet<u64>,
}

/// The `seq`s of `entries` (one node's chain, in order) whose content
/// `policy` lets a pass prune at `now` (unix seconds), ascending. Never: a
/// version under a hold, a named version, one a revision references, the
/// node's latest version, one younger than `min_days`, or one already pruned.
/// An invalid policy prunes nothing: it is refused.
pub fn prune_candidates(
    policy: &Policy,
    entries: &[HistoryEntry],
    exempt: &Exemptions,
    now: i64,
) -> Result<Vec<u64>, PolicyError> {
    policy.validate()?;
    let Keep::Versions {
        max_versions,
        max_days,
    } = policy.keep
    else {
        return Ok(Vec::new());
    };
    if exempt.held {
        return Ok(Vec::new());
    }
    let versions: Vec<&HistoryEntry> = entries
        .iter()
        .filter(|e| e.kind != VersionKind::Pruned)
        .collect();
    let older_than =
        |e: &HistoryEntry, days: u32| now.saturating_sub(e.created_at) > i64::from(days) * DAY;
    let excess = versions.len().saturating_sub(max_versions as usize);
    let latest = versions.len().saturating_sub(1);
    Ok(versions
        .iter()
        .enumerate()
        .filter(|&(i, e)| {
            i != latest
                && !e.is_pruned()
                && e.kind != VersionKind::Named
                && !exempt.revision_refs.contains(&e.seq)
                && older_than(e, policy.min_days)
                && (i < excess || max_days.is_some_and(|d| older_than(e, d)))
        })
        .map(|(_, e)| e.seq)
        .collect())
}
