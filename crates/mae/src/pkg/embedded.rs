//! # Module: pkg/embedded.rs — built-in modules compiled into the binary
//!
//! The built-in modules under `/modules` are embedded into the `mae` binary at
//! compile time (via `include_dir!`). This makes the default keymap flavor
//! (`keymap-doom`) and all other built-ins **always present**, regardless of
//! install layout or OS path conventions — eliminating the discovery-fragility
//! class of bugs (a brew macOS install where every filesystem search path
//! missed → 0 modules → no `SPC` leader tree).
//!
//! Discovery becomes: embedded modules are the always-present baseline, and an
//! on-disk module with the same name **overrides** the embedded one (see
//! [`merge_modules`]). That preserves the dev loop (edit `modules/<name>/
//! autoloads.scm` + `:reload-modules` without recompiling) and lets users ship
//! their own flavors via `~/.local/share/mae/modules` / `MAE_MODULES_PATH`.
//!
//! A module's files are read uniformly through [`ModuleSource`] — embedded
//! bytes or on-disk paths — so the resolver/loader pipeline is unchanged apart
//! from carrying a `ModuleSource` instead of a `PathBuf`.

use super::manifest::ModuleManifest;
use include_dir::{include_dir, Dir};
use std::path::{Path, PathBuf};

/// The `/modules` tree, embedded at compile time. `$CARGO_MANIFEST_DIR` is
/// `crates/mae`, so `../../modules` resolves to the repo's `modules/` dir.
static EMBEDDED_MODULES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../modules");

/// Where a module's files come from.
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleSource {
    /// Compiled into the binary; `dir_name` is the module's directory name
    /// (e.g. `"keymap-doom"`), a key into [`EMBEDDED_MODULES`].
    Embedded { dir_name: String },
    /// An on-disk module directory.
    Disk(PathBuf),
}

impl ModuleSource {
    /// Read a file relative to the module root (e.g. `"autoloads.scm"`).
    /// Returns `None` if the file does not exist in this source.
    pub fn read_relative(&self, rel: &str) -> Option<String> {
        match self {
            ModuleSource::Disk(dir) => std::fs::read_to_string(dir.join(rel)).ok(),
            ModuleSource::Embedded { dir_name } => EMBEDDED_MODULES
                .get_file(format!("{dir_name}/{rel}"))
                .and_then(|f| f.contents_utf8())
                .map(|s| s.to_string()),
        }
    }

    /// Does a relative file exist in this source?
    pub fn has_relative(&self, rel: &str) -> bool {
        match self {
            ModuleSource::Disk(dir) => dir.join(rel).exists(),
            ModuleSource::Embedded { dir_name } => EMBEDDED_MODULES
                .get_file(format!("{dir_name}/{rel}"))
                .is_some(),
        }
    }

    /// A display/virtual label for logs & error messages. `rel` may be empty
    /// to label the module root.
    pub fn label(&self, rel: &str) -> String {
        match self {
            ModuleSource::Disk(dir) if rel.is_empty() => dir.display().to_string(),
            ModuleSource::Disk(dir) => dir.join(rel).display().to_string(),
            ModuleSource::Embedded { dir_name } if rel.is_empty() => {
                format!("embedded:{dir_name}")
            }
            ModuleSource::Embedded { dir_name } => format!("embedded:{dir_name}/{rel}"),
        }
    }

    /// Best-effort on-disk module dir, for Scheme relative-path resolution
    /// (`set_module_dir`). `None` for embedded modules — verified that none of
    /// the embedded modules reference on-disk relative assets.
    pub fn disk_dir(&self) -> Option<&Path> {
        match self {
            ModuleSource::Disk(dir) => Some(dir.as_path()),
            ModuleSource::Embedded { .. } => None,
        }
    }
}

/// A discovered module: where it lives plus its parsed manifest.
#[derive(Debug, Clone)]
pub struct DiscoveredModule {
    pub source: ModuleSource,
    pub manifest: ModuleManifest,
}

/// Enumerate modules compiled into the binary.
pub fn discover_embedded_modules() -> Vec<DiscoveredModule> {
    let mut out = Vec::new();
    for sub in EMBEDDED_MODULES.dirs() {
        // Top-level subdir path is just the module dir name (e.g. "keymap-doom").
        let dir_name = sub.path().to_string_lossy().to_string();
        let Some(file) = EMBEDDED_MODULES.get_file(format!("{dir_name}/module.toml")) else {
            continue; // not a module dir
        };
        let Some(content) = file.contents_utf8() else {
            continue;
        };
        let label = format!("embedded:{dir_name}/module.toml");
        match ModuleManifest::from_str(content, Path::new(&label)) {
            Ok(manifest) => out.push(DiscoveredModule {
                source: ModuleSource::Embedded { dir_name },
                manifest,
            }),
            Err(e) => eprintln!("[warn] skipping embedded module {dir_name}: {e}"),
        }
    }
    out
}

/// Merge the embedded baseline with on-disk discoveries: on-disk modules
/// override embedded ones **by name** (so the dev loop and user customization
/// win), while embedded-only modules remain present. `disk` is the already
/// collected on-disk discoveries in priority order.
pub fn merge_modules(disk: Vec<DiscoveredModule>) -> Vec<DiscoveredModule> {
    use std::collections::BTreeMap;
    let mut by_name: BTreeMap<String, DiscoveredModule> = BTreeMap::new();
    for d in discover_embedded_modules() {
        by_name.insert(d.manifest.name().to_string(), d);
    }
    for d in disk {
        by_name.insert(d.manifest.name().to_string(), d); // disk overrides embedded
    }
    by_name.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_modules_include_keymap_doom() {
        let mods = discover_embedded_modules();
        assert!(
            mods.len() >= 20,
            "expected the full built-in module set embedded, got {}",
            mods.len()
        );
        let doom = mods
            .iter()
            .find(|d| d.manifest.name() == "keymap-doom")
            .expect("keymap-doom must be embedded (it is the default flavor)");
        assert!(matches!(doom.source, ModuleSource::Embedded { .. }));
        let autoloads = doom
            .source
            .read_relative("autoloads.scm")
            .expect("keymap-doom autoloads.scm must be embedded");
        assert!(
            autoloads.contains("collab-start"),
            "embedded keymap-doom should define the collab leader bindings"
        );
        assert!(doom.source.has_relative("module.toml"));
        assert!(doom.source.disk_dir().is_none());
    }

    #[test]
    fn disk_overrides_embedded_by_name() {
        let fake = DiscoveredModule {
            source: ModuleSource::Disk(PathBuf::from("/tmp/dev/keymap-doom")),
            manifest: ModuleManifest::from_str(
                "[module]\nname = \"keymap-doom\"",
                Path::new("test"),
            )
            .unwrap(),
        };
        let merged = merge_modules(vec![fake]);
        let doom = merged
            .iter()
            .find(|d| d.manifest.name() == "keymap-doom")
            .unwrap();
        assert_eq!(
            doom.source,
            ModuleSource::Disk(PathBuf::from("/tmp/dev/keymap-doom")),
            "on-disk keymap-doom must override the embedded copy"
        );
        // Embedded-only modules are still present after the overlay.
        assert!(merged.iter().any(|d| d.manifest.name() == "surround"));
    }
}
