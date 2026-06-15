//! # Module: pkg/resolver.rs — Dependency resolution and topological sort
//!
//! Takes a set of module manifests and produces a load order that respects
//! dependency constraints. Detects circular dependencies and version conflicts.

use super::embedded::{DiscoveredModule, ModuleSource};
use super::manifest::ModuleManifest;
use std::collections::{HashMap, HashSet};

/// A module that's been resolved and is ready to load.
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub name: String,
    /// Where the module's files are read from (embedded or on disk).
    pub source: ModuleSource,
    pub manifest: ModuleManifest,
    /// Flags enabled for this module (from user's mae! declaration).
    pub enabled_flags: Vec<String>,
}

/// A module that could NOT be resolved (and why), so the caller can surface it
/// without letting it take down the rest of the load.
#[derive(Debug, Clone)]
pub struct SkippedModule {
    pub name: String,
    pub reason: String,
}

/// Outcome of resolution: the modules that resolved (in load order) plus any that
/// were skipped because their dependency graph was unsatisfiable.
#[derive(Debug, Clone, Default)]
pub struct ResolveOutcome {
    pub resolved: Vec<ResolvedModule>,
    pub skipped: Vec<SkippedModule>,
}

/// Resolve load order via topological sort, returning modules in dependency order.
///
/// Resolution degrades gracefully: a module with a missing/disabled dependency, or
/// one caught in a circular dependency, is *skipped* (recorded in
/// [`ResolveOutcome::skipped`]) rather than aborting the entire load. This is a
/// deliberate change from the old all-or-nothing behavior — a single broken
/// drop-in module (the documented `~/.local/share/mae/modules` extensibility
/// path) must NOT be able to brick the editor by taking out the embedded
/// `keymap-leader`/flavor and the whole leader/which-key system with it. Skipping
/// a module also skips everything that (transitively) depends on it, so the
/// surviving set is always internally consistent and safe to load.
pub fn resolve_load_order(
    modules: &[DiscoveredModule],
    enabled: &HashMap<String, Vec<String>>, // name -> enabled flags
) -> ResolveOutcome {
    // Build name -> index map
    let name_map: HashMap<&str, usize> = modules
        .iter()
        .enumerate()
        .map(|(i, d)| (d.manifest.name(), i))
        .collect();

    // Start with every enabled+discovered module active, then prune to a
    // resolvable subset.
    let mut active: HashSet<usize> = modules
        .iter()
        .enumerate()
        .filter(|(_, d)| enabled.contains_key(d.manifest.name()))
        .map(|(i, _)| i)
        .collect();

    let mut skipped: Vec<SkippedModule> = Vec::new();

    // Fixpoint prune: drop any active module whose dependency is unavailable
    // (not discovered) or disabled (not active). Dropping a module makes its
    // active dependents unsatisfiable too, so we loop until the set is stable.
    loop {
        let mut to_remove: Vec<(usize, String)> = Vec::new();
        for &idx in &active {
            for dep_name in modules[idx].manifest.dependencies.keys() {
                let reason = match name_map.get(dep_name.as_str()) {
                    None => Some(format!("depends on '{dep_name}' which is not available")),
                    Some(&dep_idx) if !active.contains(&dep_idx) => {
                        Some(format!("depends on '{dep_name}' which is not enabled"))
                    }
                    _ => None,
                };
                if let Some(reason) = reason {
                    to_remove.push((idx, reason));
                    break; // first unsatisfied dep is enough to skip this module
                }
            }
        }
        if to_remove.is_empty() {
            break;
        }
        // Deterministic skip order for stable diagnostics.
        to_remove.sort_by_key(|(idx, _)| *idx);
        for (idx, reason) in to_remove {
            if active.remove(&idx) {
                skipped.push(SkippedModule {
                    name: modules[idx].manifest.name().to_string(),
                    reason,
                });
            }
        }
    }

    // Topological sort the surviving active set (Kahn's algorithm).
    let mut in_degree: HashMap<usize, usize> = HashMap::new();
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for &idx in &active {
        in_degree.entry(idx).or_insert(0);
        for dep_name in modules[idx].manifest.dependencies.keys() {
            if let Some(&dep_idx) = name_map.get(dep_name.as_str()) {
                // dep_idx is guaranteed active here (pruning ensured it).
                adj.entry(dep_idx).or_default().push(idx);
                *in_degree.entry(idx).or_insert(0) += 1;
            }
        }
    }

    let mut queue: Vec<usize> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&idx, _)| idx)
        .collect();
    queue.sort(); // deterministic order

    let mut order = Vec::new();
    while let Some(idx) = queue.pop() {
        order.push(idx);
        if let Some(neighbors) = adj.get(&idx) {
            for &next in neighbors {
                let deg = in_degree.get_mut(&next).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push(next);
                }
            }
        }
    }

    if order.len() != active.len() {
        // Whatever didn't come out of the queue is part of a cycle — skip those
        // members (and report them) instead of failing the whole load.
        let sorted: HashSet<usize> = order.iter().copied().collect();
        let mut cycle: Vec<usize> = active
            .iter()
            .copied()
            .filter(|i| !sorted.contains(i))
            .collect();
        cycle.sort();
        let names: Vec<&str> = cycle.iter().map(|&i| modules[i].manifest.name()).collect();
        let joined = names.join(", ");
        for &i in &cycle {
            skipped.push(SkippedModule {
                name: modules[i].manifest.name().to_string(),
                reason: format!("part of a circular dependency among: {joined}"),
            });
        }
    }

    let resolved = order
        .into_iter()
        .map(|idx| {
            let d = &modules[idx];
            let flags = enabled.get(d.manifest.name()).cloned().unwrap_or_default();
            ResolvedModule {
                name: d.manifest.name().to_string(),
                source: d.source.clone(),
                manifest: d.manifest.clone(),
                enabled_flags: flags,
            }
        })
        .collect();

    ResolveOutcome { resolved, skipped }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkg::manifest::ModuleManifest;
    use std::path::{Path, PathBuf};

    fn make_module(name: &str, deps: &[(&str, &str)]) -> DiscoveredModule {
        let deps_str = deps
            .iter()
            .map(|(k, v)| format!("{} = \"{}\"", k, v))
            .collect::<Vec<_>>()
            .join("\n");
        let toml = format!(
            "[module]\nname = \"{}\"\n\n[dependencies]\n{}",
            name, deps_str
        );
        let manifest = ModuleManifest::from_str(&toml, Path::new("test")).unwrap();
        DiscoveredModule {
            source: ModuleSource::Disk(PathBuf::from(format!("modules/{}", name))),
            manifest,
        }
    }

    #[test]
    fn no_deps_sorts_alphabetically() {
        let modules = vec![
            make_module("c-mod", &[]),
            make_module("a-mod", &[]),
            make_module("b-mod", &[]),
        ];
        let enabled: HashMap<String, Vec<String>> = modules
            .iter()
            .map(|d| (d.manifest.name().to_string(), vec![]))
            .collect();
        let outcome = resolve_load_order(&modules, &enabled);
        // All should be present (order is deterministic but may vary)
        assert_eq!(outcome.resolved.len(), 3);
        assert!(outcome.skipped.is_empty());
    }

    #[test]
    fn deps_respected() {
        let modules = vec![
            make_module("org", &[("tables", ">=0.1.0")]),
            make_module("tables", &[]),
        ];
        let enabled: HashMap<String, Vec<String>> = modules
            .iter()
            .map(|d| (d.manifest.name().to_string(), vec![]))
            .collect();
        let outcome = resolve_load_order(&modules, &enabled);
        let names: Vec<&str> = outcome.resolved.iter().map(|r| r.name.as_str()).collect();
        let tables_pos = names.iter().position(|&n| n == "tables").unwrap();
        let org_pos = names.iter().position(|&n| n == "org").unwrap();
        assert!(tables_pos < org_pos, "tables must load before org");
    }

    #[test]
    fn circular_dep_skips_cycle_members_not_everything() {
        // A cycle between a<->b must not take down an unrelated healthy module.
        let modules = vec![
            make_module("a", &[("b", "*")]),
            make_module("b", &[("a", "*")]),
            make_module("healthy", &[]),
        ];
        let enabled: HashMap<String, Vec<String>> = modules
            .iter()
            .map(|d| (d.manifest.name().to_string(), vec![]))
            .collect();
        let outcome = resolve_load_order(&modules, &enabled);
        let resolved: Vec<&str> = outcome.resolved.iter().map(|r| r.name.as_str()).collect();
        assert!(
            resolved.contains(&"healthy"),
            "healthy module must still load"
        );
        let skipped: Vec<&str> = outcome.skipped.iter().map(|s| s.name.as_str()).collect();
        assert!(skipped.contains(&"a") && skipped.contains(&"b"));
        assert!(outcome
            .skipped
            .iter()
            .all(|s| s.reason.contains("circular")));
    }

    #[test]
    fn missing_dep_skips_only_the_dependent() {
        // org needs the (absent) tables module; healthy is independent.
        let modules = vec![
            make_module("org", &[("tables", ">=0.1.0")]),
            make_module("healthy", &[]),
        ];
        let enabled: HashMap<String, Vec<String>> = modules
            .iter()
            .map(|d| (d.manifest.name().to_string(), vec![]))
            .collect();
        let outcome = resolve_load_order(&modules, &enabled);
        let resolved: Vec<&str> = outcome.resolved.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(resolved, vec!["healthy"]);
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(outcome.skipped[0].name, "org");
        assert!(outcome.skipped[0].reason.contains("not available"));
    }

    #[test]
    fn transitive_dependents_of_a_skipped_module_are_also_skipped() {
        // c -> b -> tables(absent). Both b and c must be skipped, a survives.
        let modules = vec![
            make_module("a", &[]),
            make_module("b", &[("tables", "*")]),
            make_module("c", &[("b", "*")]),
        ];
        let enabled: HashMap<String, Vec<String>> = modules
            .iter()
            .map(|d| (d.manifest.name().to_string(), vec![]))
            .collect();
        let outcome = resolve_load_order(&modules, &enabled);
        let resolved: Vec<&str> = outcome.resolved.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(resolved, vec!["a"]);
        let skipped: Vec<&str> = outcome.skipped.iter().map(|s| s.name.as_str()).collect();
        assert!(skipped.contains(&"b") && skipped.contains(&"c"));
    }

    #[test]
    fn disabled_module_excluded() {
        let modules = vec![make_module("a", &[]), make_module("b", &[])];
        let mut enabled = HashMap::new();
        enabled.insert("a".to_string(), vec![]);
        // b is not enabled
        let outcome = resolve_load_order(&modules, &enabled);
        assert_eq!(outcome.resolved.len(), 1);
        assert_eq!(outcome.resolved[0].name, "a");
    }

    #[test]
    fn flags_propagated() {
        let modules = vec![make_module("org", &[])];
        let mut enabled = HashMap::new();
        enabled.insert(
            "org".to_string(),
            vec!["+agenda".to_string(), "+babel".to_string()],
        );
        let outcome = resolve_load_order(&modules, &enabled);
        assert_eq!(outcome.resolved[0].enabled_flags, vec!["+agenda", "+babel"]);
    }
}
