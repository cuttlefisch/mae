//! # Module: pkg/loader.rs — Module loading and lifecycle
//!
//! Integrates with SchemeRuntime and Editor to load module autoloads,
//! track module state, and support reload/unload.

use super::manifest::ModuleManifest;
use super::resolver::ResolvedModule;
use std::collections::HashMap;
use std::path::PathBuf;

/// Runtime state of a loaded module.
#[derive(Debug, Clone)]
pub struct ModuleState {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub manifest: ModuleManifest,
    pub enabled_flags: Vec<String>,
    pub status: ModuleStatus,
    /// Commands registered by this module's autoloads.
    pub commands: Vec<String>,
    /// Keybindings registered by this module's autoloads.
    pub keybindings: Vec<(String, String, String)>, // (keymap, key, command)
    /// Options registered by this module.
    pub options: Vec<String>,
    /// Hooks registered by this module.
    pub hooks: Vec<(String, String)>, // (hook_name, fn_name)
}

/// Module lifecycle status.
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleStatus {
    /// Discovered but not yet loaded.
    Discovered,
    /// Autoloads evaluated successfully.
    Loaded,
    /// Failed to load (error message stored).
    Failed(String),
    /// Explicitly disabled by user.
    Disabled,
}

/// Registry tracking all known modules and their runtime state.
#[derive(Debug, Clone)]
pub struct ModuleRegistry {
    modules: HashMap<String, ModuleState>,
    /// Load order (topologically sorted).
    load_order: Vec<String>,
}

impl ModuleRegistry {
    pub fn new() -> Self {
        ModuleRegistry {
            modules: HashMap::new(),
            load_order: Vec::new(),
        }
    }

    /// Register resolved modules (pre-load).
    pub fn register_resolved(&mut self, resolved: &[ResolvedModule]) {
        self.load_order = resolved.iter().map(|r| r.name.clone()).collect();
        for r in resolved {
            self.modules.insert(
                r.name.clone(),
                ModuleState {
                    name: r.name.clone(),
                    version: r.manifest.module.version.clone(),
                    path: r.path.clone(),
                    manifest: r.manifest.clone(),
                    enabled_flags: r.enabled_flags.clone(),
                    status: ModuleStatus::Discovered,
                    commands: Vec::new(),
                    keybindings: Vec::new(),
                    options: Vec::new(),
                    hooks: Vec::new(),
                },
            );
        }
    }

    /// Mark a module as loaded.
    pub fn mark_loaded(&mut self, name: &str) {
        if let Some(state) = self.modules.get_mut(name) {
            state.status = ModuleStatus::Loaded;
        }
    }

    /// Mark a module as failed.
    pub fn mark_failed(&mut self, name: &str, error: String) {
        if let Some(state) = self.modules.get_mut(name) {
            state.status = ModuleStatus::Failed(error);
        }
    }

    /// Get module state by name.
    pub fn get(&self, name: &str) -> Option<&ModuleState> {
        self.modules.get(name)
    }

    /// Get mutable module state.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut ModuleState> {
        self.modules.get_mut(name)
    }

    /// Check if a module is loaded.
    pub fn is_loaded(&self, name: &str) -> bool {
        self.modules
            .get(name)
            .is_some_and(|s| s.status == ModuleStatus::Loaded)
    }

    /// List all modules in load order.
    pub fn list(&self) -> Vec<&ModuleState> {
        self.load_order
            .iter()
            .filter_map(|name| self.modules.get(name))
            .collect()
    }

    /// List module names.
    pub fn names(&self) -> &[String] {
        &self.load_order
    }

    /// Number of registered modules.
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Get the flags enabled for a module.
    pub fn flags(&self, name: &str) -> &[String] {
        self.modules
            .get(name)
            .map(|s| s.enabled_flags.as_slice())
            .unwrap_or(&[])
    }

    /// Check if a specific flag is enabled for a module.
    pub fn has_flag(&self, module: &str, flag: &str) -> bool {
        self.flags(module).iter().any(|f| f == flag)
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Load a module's autoloads.scm into the SchemeRuntime.
///
/// This evaluates the module's autoloads file, which registers commands,
/// keybindings, options, and hooks eagerly (before user config.scm).
///
/// Returns the list of errors encountered (non-fatal).
pub fn load_module_autoloads(
    module: &ResolvedModule,
    scheme: &mut mae_scheme::SchemeRuntime,
    editor: &mut mae_core::Editor,
) -> Result<(), String> {
    let autoloads_path = module.path.join(&module.manifest.entry.autoloads);
    if !autoloads_path.exists() {
        // No autoloads file — that's fine, module may only have init.scm
        return Ok(());
    }

    // Inject flag state so (when-flag "+foo" ...) can check it
    // The flags are set as Scheme variables before loading
    for flag in &module.enabled_flags {
        let clean = flag.trim_start_matches('+');
        let expr = format!("(define __mae-flag-{}-{} #t)", module.name, clean);
        if let Err(e) = scheme.eval(&expr) {
            eprintln!("[warn] Failed to set flag {}/{}: {}", module.name, flag, e);
        }
    }

    // Set module dir for relative path resolution in register-splash-art-image! etc.
    scheme.set_module_dir(Some(&module.path));

    // Load the autoloads file
    scheme.inject_editor_state(editor);
    if let Err(e) = scheme.load_file(&autoloads_path) {
        return Err(format!(
            "Failed to load autoloads for '{}': {}",
            module.name, e
        ));
    }

    // Register module in Scheme runtime's active modules
    let reg_expr = format!(
        "(register-module! \"{}\" \"{}\")",
        module.name, module.manifest.module.version
    );
    let _ = scheme.eval(&reg_expr);

    // Apply accumulated state to editor
    scheme.apply_to_editor(editor);

    // Clear module dir after loading
    scheme.set_module_dir(None);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkg::manifest::ModuleManifest;
    use std::path::Path;

    fn make_resolved(name: &str, flags: &[&str]) -> ResolvedModule {
        let toml = format!("[module]\nname = \"{}\"", name);
        let manifest = ModuleManifest::from_str(&toml, Path::new("test")).unwrap();
        ResolvedModule {
            name: name.to_string(),
            path: PathBuf::from(format!("modules/{}", name)),
            manifest,
            enabled_flags: flags.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn registry_lifecycle() {
        let mut reg = ModuleRegistry::new();
        let resolved = vec![
            make_resolved("dashboard", &[]),
            make_resolved("surround", &[]),
        ];
        reg.register_resolved(&resolved);

        assert_eq!(reg.len(), 2);
        assert!(!reg.is_loaded("dashboard"));
        assert_eq!(
            reg.get("dashboard").unwrap().status,
            ModuleStatus::Discovered
        );

        reg.mark_loaded("dashboard");
        assert!(reg.is_loaded("dashboard"));
        assert!(!reg.is_loaded("surround"));
    }

    #[test]
    fn registry_flags() {
        let mut reg = ModuleRegistry::new();
        let resolved = vec![make_resolved("org", &["+agenda", "+babel"])];
        reg.register_resolved(&resolved);

        assert!(reg.has_flag("org", "+agenda"));
        assert!(reg.has_flag("org", "+babel"));
        assert!(!reg.has_flag("org", "+export"));
    }

    #[test]
    fn list_in_load_order() {
        let mut reg = ModuleRegistry::new();
        let resolved = vec![make_resolved("tables", &[]), make_resolved("org", &[])];
        reg.register_resolved(&resolved);

        let list = reg.list();
        assert_eq!(list[0].name, "tables");
        assert_eq!(list[1].name, "org");
    }

    #[test]
    fn mark_failed() {
        let mut reg = ModuleRegistry::new();
        let resolved = vec![make_resolved("bad", &[])];
        reg.register_resolved(&resolved);

        reg.mark_failed("bad", "syntax error".to_string());
        assert_eq!(
            reg.get("bad").unwrap().status,
            ModuleStatus::Failed("syntax error".to_string())
        );
        assert!(!reg.is_loaded("bad"));
    }
}
