//! # Module: pkg/manifest.rs — Module manifest parsing
//!
//! Parses `module.toml` manifests into `ModuleManifest` structs.
//! Handles identity, dependencies, flags, and entry points.
//!
//! Does NOT depend on SchemeRuntime — manifest parsing is pre-Scheme,
//! enabling `mae pkg list` without starting the editor.

use super::embedded::{DiscoveredModule, ModuleSource};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// A parsed `module.toml` manifest.
///
/// `deny_unknown_fields` (here and on the nested structs below) is
/// deliberate: without it, a misplaced or misspelled key — e.g. `depends`
/// nested inside `[module]` instead of a sibling `[dependencies]` table, the
/// exact bug this guards against — is silently dropped by serde rather than
/// failing manifest parsing, so the intended dependency ordering (or
/// whatever the field was meant to configure) just silently never applies.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleManifest {
    pub module: ModuleIdentity,
    #[serde(default)]
    pub flags: HashMap<String, FlagDef>,
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
    #[serde(default)]
    pub entry: EntryPoints,
}

/// Module identity section.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleIdentity {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub mae_version: String,
    #[serde(default)]
    pub category: String,
    /// A **required** (core) module: auto-enabled regardless of the `(mae!)` block,
    /// unless explicitly disabled via `(package! "name" :disable #t)`. Doom's `core/`
    /// analog — for cross-cutting features whose buffers/prompts can be raised by
    /// *background* events (so their keybindings must always be present), e.g. the
    /// `notifications` attention bus. Optional, user-initiated features stay opt-in.
    #[serde(default)]
    pub required: bool,
    /// URL for docs or project homepage.
    #[serde(default)]
    pub homepage: String,
    /// Git URL for the source repository.
    #[serde(default)]
    pub repository: String,
    /// Searchable tags for package discovery.
    #[serde(default)]
    pub keywords: Vec<String>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// A flag that can be enabled with `+name` syntax in `mae!` declarations.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlagDef {
    pub doc: String,
}

/// Entry point file paths (relative to module directory).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryPoints {
    #[serde(default = "default_init")]
    pub init: String,
    #[serde(default = "default_autoloads")]
    pub autoloads: String,
}

fn default_init() -> String {
    "init.scm".to_string()
}

fn default_autoloads() -> String {
    "autoloads.scm".to_string()
}

impl Default for EntryPoints {
    fn default() -> Self {
        EntryPoints {
            init: default_init(),
            autoloads: default_autoloads(),
        }
    }
}

impl ModuleManifest {
    /// Parse a module.toml file.
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        Self::from_str(&content, path)
    }

    /// Parse from a TOML string (with path for error context).
    pub fn from_str(content: &str, path: &Path) -> Result<Self, String> {
        toml::from_str(content).map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
    }

    /// The module name.
    pub fn name(&self) -> &str {
        &self.module.name
    }

    /// Check if a version constraint is satisfied by the current MAE version.
    pub fn check_mae_version(&self, current: &str) -> Result<(), String> {
        if self.module.mae_version.is_empty() {
            return Ok(());
        }
        let req = semver::VersionReq::parse(&self.module.mae_version)
            .map_err(|e| format!("Invalid mae_version '{}': {}", self.module.mae_version, e))?;
        let ver = semver::Version::parse(current)
            .map_err(|e| format!("Invalid current version '{}': {}", current, e))?;
        if req.matches(&ver) {
            Ok(())
        } else {
            Err(format!(
                "Module '{}' requires MAE {}, current is {}",
                self.module.name, self.module.mae_version, current
            ))
        }
    }
}

/// Discover all on-disk modules in a directory (each subdirectory with a
/// `module.toml`). Returns [`DiscoveredModule`]s with `ModuleSource::Disk`;
/// embedded modules are discovered separately (see [`super::embedded`]) and
/// merged via [`super::embedded::merge_modules`].
pub fn discover_modules(dir: &Path) -> Vec<DiscoveredModule> {
    let mut modules = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return modules;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let manifest_path = path.join("module.toml");
            if manifest_path.exists() {
                match ModuleManifest::from_path(&manifest_path) {
                    Ok(manifest) => modules.push(DiscoveredModule {
                        source: ModuleSource::Disk(path),
                        manifest,
                    }),
                    Err(e) => eprintln!("[warn] Skipping {}: {}", manifest_path.display(), e),
                }
            }
        }
    }
    modules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_manifest() {
        let toml = r#"
[module]
name = "dashboard"
"#;
        let m = ModuleManifest::from_str(toml, Path::new("test")).unwrap();
        assert_eq!(m.name(), "dashboard");
        assert_eq!(m.module.version, "0.1.0");
        assert!(m.flags.is_empty());
        assert!(m.dependencies.is_empty());
        assert_eq!(m.entry.init, "init.scm");
        assert_eq!(m.entry.autoloads, "autoloads.scm");
        // `required` defaults false — modules are opt-in unless they opt into core.
        assert!(!m.module.required);
    }

    #[test]
    fn parse_required_core_module() {
        let toml = r#"
[module]
name = "notifications"
category = "tools"
required = true
"#;
        let m = ModuleManifest::from_str(toml, Path::new("test")).unwrap();
        assert!(
            m.module.required,
            "a core module opts into auto-enable via `required = true`"
        );
    }

    #[test]
    fn parse_full_manifest() {
        let toml = r#"
[module]
name = "org"
version = "0.2.0"
description = "Org-mode support"
mae_version = ">=0.9.0"
category = "lang"

[flags]
agenda = { doc = "Task/schedule agenda view" }
babel = { doc = "Code block execution" }

[dependencies]
tables = ">=0.1.0"

[entry]
init = "org-init.scm"
autoloads = "org-autoloads.scm"
"#;
        let m = ModuleManifest::from_str(toml, Path::new("test")).unwrap();
        assert_eq!(m.name(), "org");
        assert_eq!(m.module.version, "0.2.0");
        assert_eq!(m.flags.len(), 2);
        assert!(m.flags.contains_key("agenda"));
        assert_eq!(m.dependencies.len(), 1);
        assert_eq!(m.entry.init, "org-init.scm");
    }

    /// Adversarial regression test for the audit finding that `agenda`'s and
    /// `tables`' `module.toml` used a `depends = [...]` array nested inside
    /// `[module]` — not part of the schema (the real mechanism is a sibling
    /// `[dependencies]` table) — and serde silently dropped it, leaving
    /// dependency ordering unenforced with no error anywhere. Confirms
    /// `deny_unknown_fields` now makes parsing FAIL LOUD on exactly that
    /// malformed shape, instead of silently ignoring it.
    #[test]
    fn misplaced_depends_field_in_module_table_is_rejected() {
        let toml = r#"
[module]
name = "agenda"
depends = ["org"]

[entry]
autoloads = "autoloads.scm"
"#;
        let err = ModuleManifest::from_str(toml, Path::new("test")).unwrap_err();
        assert!(
            err.contains("depends") || err.contains("unknown field"),
            "error should point at the unrecognized field, got: {err}"
        );
    }

    /// The two real modules this bug affected must now parse correctly
    /// through the fixed `[dependencies]` table form.
    #[test]
    fn agenda_and_tables_dependencies_parse_correctly() {
        let agenda = r#"
[module]
name = "agenda"
category = "app"

[dependencies]
org = "*"

[entry]
autoloads = "autoloads.scm"
"#;
        let m = ModuleManifest::from_str(agenda, Path::new("test")).unwrap();
        assert_eq!(m.dependencies.get("org").map(String::as_str), Some("*"));

        let tables = r#"
[module]
name = "tables"
category = "editor"

[dependencies]
org = "*"
markdown = "*"

[entry]
init = "init.scm"
autoloads = "autoloads.scm"
"#;
        let m = ModuleManifest::from_str(tables, Path::new("test")).unwrap();
        assert_eq!(m.dependencies.len(), 2);
        assert!(m.dependencies.contains_key("org"));
        assert!(m.dependencies.contains_key("markdown"));
    }

    #[test]
    fn mae_version_check() {
        let toml = r#"
[module]
name = "test"
mae_version = ">=0.9.0"
"#;
        let m = ModuleManifest::from_str(toml, Path::new("test")).unwrap();
        assert!(m.check_mae_version("0.9.0").is_ok());
        assert!(m.check_mae_version("1.0.0").is_ok());
        assert!(m.check_mae_version("0.8.1").is_err());
    }

    #[test]
    fn empty_mae_version_always_passes() {
        let toml = r#"
[module]
name = "test"
"#;
        let m = ModuleManifest::from_str(toml, Path::new("test")).unwrap();
        assert!(m.check_mae_version("0.1.0").is_ok());
    }

    #[test]
    fn invalid_toml_gives_error() {
        let result = ModuleManifest::from_str("not valid toml {{{", Path::new("bad"));
        assert!(result.is_err());
    }

    #[test]
    fn parse_manifest_with_metadata() {
        let toml = r#"
[module]
name = "splash-themes"
version = "0.1.0"
description = "Community splash screen art"
homepage = "https://github.com/cuttlefisch/mae-splash-themes"
repository = "https://github.com/cuttlefisch/mae-splash-themes.git"
keywords = ["splash", "themes", "art"]
category = "ui"
"#;
        let m = ModuleManifest::from_str(toml, Path::new("test")).unwrap();
        assert_eq!(
            m.module.homepage,
            "https://github.com/cuttlefisch/mae-splash-themes"
        );
        assert_eq!(
            m.module.repository,
            "https://github.com/cuttlefisch/mae-splash-themes.git"
        );
        assert_eq!(m.module.keywords, vec!["splash", "themes", "art"]);
    }

    #[test]
    fn metadata_fields_default_empty() {
        let toml = r#"
[module]
name = "minimal"
"#;
        let m = ModuleManifest::from_str(toml, Path::new("test")).unwrap();
        assert!(m.module.homepage.is_empty());
        assert!(m.module.repository.is_empty());
        assert!(m.module.keywords.is_empty());
    }
}
