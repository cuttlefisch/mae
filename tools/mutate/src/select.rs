//! Which mutants a command acts on: `--area`, `--since`, `--only`, `--shard`.

use crate::config::{Mutant, LIST_DIR, RUNNERS_FILE};
use crate::git;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Default)]
pub struct Selection {
    pub areas: Vec<String>,
    pub only: Vec<String>,
    pub since: Option<String>,
    /// (index, count), index in 1..=count.
    pub shard: Option<(u64, u64)>,
}

pub fn parse_shard(s: &str) -> Result<(u64, u64), String> {
    let bad = || format!("--shard {s}: expected i/N with 1 <= i <= N");
    let (i, n) = s.split_once('/').ok_or_else(bad)?;
    let (i, n): (u64, u64) = (i.parse().map_err(|_| bad())?, n.parse().map_err(|_| bad())?);
    if n == 0 || i == 0 || i > n {
        return Err(bad());
    }
    Ok((i, n))
}

/// A mutant's shard depends on its id alone, so adding an entry never moves
/// another one between shards.
pub fn shard_of(id: &str, count: u64) -> u64 {
    git::fnv(&[id]) % count + 1
}

pub fn select<'a>(
    root: &Path,
    all: &'a [Mutant],
    sel: &Selection,
) -> Result<Vec<&'a Mutant>, String> {
    let known: BTreeSet<&str> = all.iter().map(|m| m.id.as_str()).collect();
    if let Some(bad) = sel.only.iter().find(|id| !known.contains(id.as_str())) {
        return Err(format!("--only {bad}: no such mutant"));
    }
    let areas: BTreeSet<&str> = all.iter().map(|m| m.area.as_str()).collect();
    if let Some(bad) = sel.areas.iter().find(|a| !areas.contains(a.as_str())) {
        return Err(format!("--area {bad}: no mutants/{bad}.toml"));
    }
    let changed = match &sel.since {
        Some(r) => Some(git::changed_since(root, r)?),
        None => None,
    };
    let runners_changed = changed
        .as_ref()
        .is_some_and(|c| c.contains(&format!("{LIST_DIR}/{RUNNERS_FILE}")));
    Ok(all
        .iter()
        .filter(|m| sel.areas.is_empty() || sel.areas.contains(&m.area))
        .filter(|m| sel.only.is_empty() || sel.only.contains(&m.id))
        .filter(|m| match &changed {
            None => true,
            Some(c) => {
                runners_changed
                    || c.contains(&format!("{LIST_DIR}/{}.toml", m.area))
                    || m.edits.iter().any(|e| c.contains(&e.file))
            }
        })
        .filter(|m| sel.shard.is_none_or(|(i, n)| shard_of(&m.id, n) == i))
        .collect())
}
