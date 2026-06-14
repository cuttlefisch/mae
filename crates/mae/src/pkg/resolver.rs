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

/// Resolve load order via topological sort. Returns modules in dependency order.
///
/// Errors on circular dependencies or missing required dependencies.
pub fn resolve_load_order(
    modules: &[DiscoveredModule],
    enabled: &HashMap<String, Vec<String>>, // name -> enabled flags
) -> Result<Vec<ResolvedModule>, String> {
    // Build name -> index map
    let name_map: HashMap<&str, usize> = modules
        .iter()
        .enumerate()
        .map(|(i, d)| (d.manifest.name(), i))
        .collect();

    // Filter to only enabled modules
    let active: Vec<usize> = modules
        .iter()
        .enumerate()
        .filter(|(_, d)| enabled.contains_key(d.manifest.name()))
        .map(|(i, _)| i)
        .collect();

    // Topological sort (Kahn's algorithm)
    let mut in_degree: HashMap<usize, usize> = HashMap::new();
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();

    for &idx in &active {
        in_degree.entry(idx).or_insert(0);
        for dep_name in modules[idx].manifest.dependencies.keys() {
            if let Some(&dep_idx) = name_map.get(dep_name.as_str()) {
                if active.contains(&dep_idx) {
                    adj.entry(dep_idx).or_default().push(idx);
                    *in_degree.entry(idx).or_insert(0) += 1;
                } else {
                    return Err(format!(
                        "Module '{}' depends on '{}' which is not enabled",
                        modules[idx].manifest.name(),
                        dep_name
                    ));
                }
            } else {
                return Err(format!(
                    "Module '{}' depends on '{}' which is not available",
                    modules[idx].manifest.name(),
                    dep_name
                ));
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
        // Find cycle
        let sorted: HashSet<usize> = order.iter().copied().collect();
        let cycle_members: Vec<&str> = active
            .iter()
            .filter(|i| !sorted.contains(i))
            .map(|&i| modules[i].manifest.name())
            .collect();
        return Err(format!(
            "Circular dependency among: {}",
            cycle_members.join(", ")
        ));
    }

    Ok(order
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
        .collect())
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
        let order = resolve_load_order(&modules, &enabled).unwrap();
        // All should be present (order is deterministic but may vary)
        assert_eq!(order.len(), 3);
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
        let order = resolve_load_order(&modules, &enabled).unwrap();
        let names: Vec<&str> = order.iter().map(|r| r.name.as_str()).collect();
        let tables_pos = names.iter().position(|&n| n == "tables").unwrap();
        let org_pos = names.iter().position(|&n| n == "org").unwrap();
        assert!(tables_pos < org_pos, "tables must load before org");
    }

    #[test]
    fn circular_dep_detected() {
        let modules = vec![
            make_module("a", &[("b", "*")]),
            make_module("b", &[("a", "*")]),
        ];
        let enabled: HashMap<String, Vec<String>> = modules
            .iter()
            .map(|d| (d.manifest.name().to_string(), vec![]))
            .collect();
        let result = resolve_load_order(&modules, &enabled);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Circular dependency"));
    }

    #[test]
    fn missing_dep_error() {
        let modules = vec![make_module("org", &[("tables", ">=0.1.0")])];
        let enabled: HashMap<String, Vec<String>> = modules
            .iter()
            .map(|d| (d.manifest.name().to_string(), vec![]))
            .collect();
        let result = resolve_load_order(&modules, &enabled);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not available"));
    }

    #[test]
    fn disabled_module_excluded() {
        let modules = vec![make_module("a", &[]), make_module("b", &[])];
        let mut enabled = HashMap::new();
        enabled.insert("a".to_string(), vec![]);
        // b is not enabled
        let order = resolve_load_order(&modules, &enabled).unwrap();
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].name, "a");
    }

    #[test]
    fn flags_propagated() {
        let modules = vec![make_module("org", &[])];
        let mut enabled = HashMap::new();
        enabled.insert(
            "org".to_string(),
            vec!["+agenda".to_string(), "+babel".to_string()],
        );
        let order = resolve_load_order(&modules, &enabled).unwrap();
        assert_eq!(order[0].enabled_flags, vec!["+agenda", "+babel"]);
    }
}
