//! # Module: pkg/cli.rs — Package manager CLI
//!
//! Implements both `mae pkg <subcommand>` (legacy) and flat top-level
//! subcommands: `mae sync`, `mae upgrade`, `mae purge`, `mae list`,
//! `mae info`, `mae create`, `mae doctor`.

use super::git::PackageSource;
use super::lockfile::{sha256_hex, Lockfile};
use super::manifest::discover_modules;
use std::path::PathBuf;

/// Run the `mae pkg` CLI (legacy entry point). Returns exit code.
pub fn run_pkg_cli(args: &[String]) -> i32 {
    let subcmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    dispatch_subcmd(subcmd, &args[1..])
}

/// Dispatch a flat top-level subcommand. Returns exit code.
pub fn dispatch_subcmd(subcmd: &str, args: &[String]) -> i32 {
    match subcmd {
        "list" => cmd_list(),
        "info" => cmd_info(args.first().map(|s| s.as_str())),
        "create" => cmd_create(args.first().map(|s| s.as_str())),
        "doctor" => cmd_doctor(args.first().map(|s| s.as_str())),
        "sync" => cmd_sync(),
        "upgrade" => cmd_upgrade(),
        "purge" => cmd_purge(),
        "help" | "--help" | "-h" => {
            print_help();
            0
        }
        other => {
            eprintln!("Unknown subcommand: {}", other);
            eprintln!("Run `mae help` for usage.");
            1
        }
    }
}

fn print_help() {
    println!("mae — Module and package management");
    println!();
    println!("USAGE:");
    println!("  mae <subcommand> [args]");
    println!();
    println!("SUBCOMMANDS:");
    println!(
        "  sync              Materialize declared state (clone/update packages, write lockfile)"
    );
    println!("  upgrade           Fetch latest for all packages, update lockfile SHAs");
    println!("  purge             Remove packages not declared in init.scm");
    println!("  list              List discovered modules and their status");
    println!("  info <NAME>       Show detailed information about a module");
    println!("  create <NAME>     Scaffold a new module directory");
    println!("  doctor [NAME]     Validate module manifests");
    println!("  help              Print this help");
}

fn module_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("modules")];
    if let Some(user_pkg) = crate::bootstrap::dirs_candidate("mae/packages") {
        dirs.push(user_pkg);
    }
    dirs
}

fn packages_dir() -> PathBuf {
    crate::bootstrap::dirs_candidate("mae/packages").unwrap_or_else(|| PathBuf::from("packages"))
}

/// Parse init.scm to extract declared packages.
/// Uses a minimal SchemeRuntime eval — no editor needed.
fn parse_declared_packages() -> Vec<mae_scheme::DeclaredPackage> {
    let Ok(mut scheme) = mae_scheme::SchemeRuntime::new() else {
        eprintln!("Failed to initialize Scheme runtime");
        return vec![];
    };

    // Find init.scm
    let init_path = crate::bootstrap::dirs_candidate("mae/init.scm")
        .unwrap_or_else(|| PathBuf::from("init.scm"));

    if !init_path.exists() {
        eprintln!("No init.scm found at {}", init_path.display());
        return vec![];
    }

    if let Err(e) = scheme.load_file(&init_path) {
        eprintln!("Error loading init.scm: {}", e);
        return vec![];
    }

    scheme.declared_packages()
}

// ── sync ──────────────────────────────────────────────────────────

fn cmd_sync() -> i32 {
    println!("mae sync — materializing declared state...");

    let packages = parse_declared_packages();
    let pkg_dir = packages_dir();
    let lockfile_path = Lockfile::default_path();
    let mut lockfile = Lockfile::load(&lockfile_path);
    let mut synced = 0;
    let mut errors = 0;

    for pkg in &packages {
        if pkg.disable {
            continue;
        }
        let Some(ref source_spec) = pkg.source else {
            continue; // Built-in module override, no clone needed
        };

        let source = match PackageSource::parse(source_spec) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  {} — {}", pkg.name, e);
                errors += 1;
                continue;
            }
        };

        let target = pkg_dir.join(&pkg.name);

        if !target.exists() {
            // Clone
            print!("  Cloning {}...", pkg.name);
            if let Err(e) = std::fs::create_dir_all(&pkg_dir) {
                eprintln!(" failed to create packages dir: {}", e);
                errors += 1;
                continue;
            }
            match super::git::shallow_clone(&source.clone_url(), &target) {
                Ok(()) => println!(" done"),
                Err(e) => {
                    eprintln!(" {}", e);
                    errors += 1;
                    continue;
                }
            }
        } else if let Some(ref pin) = pkg.pin {
            // Pinned — checkout specific SHA
            print!("  Pinning {} to {}...", pkg.name, &pin[..pin.len().min(8)]);
            match super::git::checkout_sha(&target, pin) {
                Ok(()) => println!(" done"),
                Err(e) => {
                    eprintln!(" {}", e);
                    errors += 1;
                    continue;
                }
            }
        }

        // Get current SHA and update lockfile
        match super::git::head_sha(&target) {
            Ok(sha) => {
                let manifest_path = target.join("module.toml");
                let integrity = if manifest_path.exists() {
                    std::fs::read(manifest_path)
                        .map(|data| sha256_hex(&data))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                lockfile.pin(&pkg.name, source_spec, &sha, &integrity);
                synced += 1;
            }
            Err(e) => {
                eprintln!("  {} — failed to read SHA: {}", pkg.name, e);
            }
        }
    }

    // Also run doctor validation
    let mut all_modules = Vec::new();
    for dir in module_search_dirs() {
        if dir.exists() {
            all_modules.extend(discover_modules(&dir));
        }
    }

    // Write lockfile
    if let Err(e) = lockfile.save(&lockfile_path) {
        eprintln!("Failed to write lockfile: {}", e);
        errors += 1;
    }

    println!();
    println!(
        "{} modules discovered, {} packages synced",
        all_modules.len(),
        synced
    );
    if errors > 0 {
        println!("{} error(s)", errors);
        1
    } else {
        0
    }
}

// ── upgrade ───────────────────────────────────────────────────────

fn cmd_upgrade() -> i32 {
    println!("mae upgrade — fetching latest for all packages...");

    let packages = parse_declared_packages();
    let pkg_dir = packages_dir();
    let lockfile_path = Lockfile::default_path();
    let mut lockfile = Lockfile::load(&lockfile_path);
    let mut updated = 0;

    for pkg in &packages {
        if pkg.disable || pkg.source.is_none() {
            continue;
        }
        let source_spec = pkg.source.as_ref().unwrap();
        let target = pkg_dir.join(&pkg.name);

        if !target.exists() {
            println!("  {} — not installed (run `mae sync` first)", pkg.name);
            continue;
        }

        let old_sha = super::git::head_sha(&target).unwrap_or_default();

        print!("  Fetching {}...", pkg.name);
        if let Err(e) = super::git::fetch_latest(&target) {
            eprintln!(" {}", e);
            continue;
        }

        // Reset to origin/HEAD (for shallow clones)
        let _ = std::process::Command::new("git")
            .args(["reset", "--hard", "origin/HEAD"])
            .current_dir(&target)
            .output();

        match super::git::head_sha(&target) {
            Ok(new_sha) => {
                if new_sha != old_sha {
                    println!(
                        " {} → {}",
                        &old_sha[..old_sha.len().min(8)],
                        &new_sha[..new_sha.len().min(8)]
                    );
                    let manifest_path = target.join("module.toml");
                    let integrity = if manifest_path.exists() {
                        std::fs::read(manifest_path)
                            .map(|data| sha256_hex(&data))
                            .unwrap_or_default()
                    } else {
                        String::new()
                    };
                    lockfile.pin(&pkg.name, source_spec, &new_sha, &integrity);
                    updated += 1;
                } else {
                    println!(" already up to date");
                }
            }
            Err(e) => eprintln!(" failed to read SHA: {}", e),
        }
    }

    if let Err(e) = lockfile.save(&lockfile_path) {
        eprintln!("Failed to write lockfile: {}", e);
    }

    println!();
    println!("{} package(s) updated", updated);
    0
}

// ── purge ─────────────────────────────────────────────────────────

fn cmd_purge() -> i32 {
    println!("mae purge — removing orphaned packages...");

    let packages = parse_declared_packages();
    let pkg_dir = packages_dir();
    let lockfile_path = Lockfile::default_path();
    let mut lockfile = Lockfile::load(&lockfile_path);

    let declared_names: std::collections::HashSet<&str> =
        packages.iter().map(|p| p.name.as_str()).collect();

    let mut removed = 0;

    if pkg_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&pkg_dir) {
            for entry in entries.flatten() {
                if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if !declared_names.contains(name.as_str()) {
                    print!("  Removing {}...", name);
                    match std::fs::remove_dir_all(entry.path()) {
                        Ok(()) => {
                            println!(" done");
                            lockfile.unpin(&name);
                            removed += 1;
                        }
                        Err(e) => eprintln!(" failed: {}", e),
                    }
                }
            }
        }
    }

    if let Err(e) = lockfile.save(&lockfile_path) {
        eprintln!("Failed to write lockfile: {}", e);
    }

    println!();
    if removed > 0 {
        println!("{} package(s) removed", removed);
    } else {
        println!("No orphaned packages found");
    }
    0
}

// ── list ──────────────────────────────────────────────────────────

fn cmd_list() -> i32 {
    let mut all_modules = Vec::new();
    for dir in module_search_dirs() {
        if dir.exists() {
            all_modules.extend(discover_modules(&dir));
        }
    }

    if all_modules.is_empty() {
        println!("No modules found.");
        return 0;
    }

    println!("{:<20} {:<10} {:<12} Path", "Module", "Version", "Category");
    println!("{:<20} {:<10} {:<12} ----", "------", "-------", "--------");

    for (path, manifest) in &all_modules {
        println!(
            "{:<20} {:<10} {:<12} {}",
            manifest.name(),
            manifest.module.version,
            if manifest.module.category.is_empty() {
                "-"
            } else {
                &manifest.module.category
            },
            path.display()
        );
    }
    println!();
    println!("Total: {} modules", all_modules.len());
    0
}

// ── create ────────────────────────────────────────────────────────

fn cmd_create(name: Option<&str>) -> i32 {
    let Some(name) = name else {
        eprintln!("Usage: mae create <NAME>");
        return 1;
    };

    let target = if let Some(user_pkg) = crate::bootstrap::dirs_candidate("mae/packages") {
        user_pkg.join(name)
    } else {
        PathBuf::from("modules").join(name)
    };

    if target.exists() {
        eprintln!("Directory already exists: {}", target.display());
        return 1;
    }

    if let Err(e) = std::fs::create_dir_all(&target) {
        eprintln!("Failed to create directory: {}", e);
        return 1;
    }

    let manifest = format!(
        r#"[module]
name = "{name}"
version = "0.1.0"
description = ""
mae_version = ">={version}"
category = "editor"

[entry]
init = "init.scm"
autoloads = "autoloads.scm"
"#,
        name = name,
        version = env!("CARGO_PKG_VERSION")
    );

    let autoloads = format!(
        r#";; {name}/autoloads.scm — keybindings and autoloads
;; This file runs eagerly at startup (before user config.scm).

;; Example: register a keybinding
;; (define-key "normal" "SPC x x" "my-command")

(provide-feature "{name}-autoloads")
"#,
        name = name
    );

    let init = format!(
        r#";; {name}/init.scm — lazy initialization
;; This file runs when the module's feature is first required.

(provide-feature "{name}")
"#,
        name = name
    );

    let write = |file: &str, content: &str| -> Result<(), String> {
        std::fs::write(target.join(file), content)
            .map_err(|e| format!("Failed to write {}: {}", file, e))
    };

    if let Err(e) = write("module.toml", &manifest) {
        eprintln!("{}", e);
        return 1;
    }
    if let Err(e) = write("autoloads.scm", &autoloads) {
        eprintln!("{}", e);
        return 1;
    }
    if let Err(e) = write("init.scm", &init) {
        eprintln!("{}", e);
        return 1;
    }

    println!("Created module '{}' at {}", name, target.display());
    println!();
    println!("Next steps:");
    println!(
        "  1. Edit {}/module.toml — add description, flags, dependencies",
        name
    );
    println!(
        "  2. Edit {}/autoloads.scm — add keybindings and commands",
        name
    );
    println!("  3. Run `mae doctor {}` to validate", name);
    0
}

// ── info ──────────────────────────────────────────────────────────

fn cmd_info(name: Option<&str>) -> i32 {
    let Some(name) = name else {
        eprintln!("Usage: mae info <NAME>");
        return 1;
    };

    let mut all_modules = Vec::new();
    for dir in module_search_dirs() {
        if dir.exists() {
            all_modules.extend(discover_modules(&dir));
        }
    }

    let Some((path, manifest)) = all_modules.iter().find(|(_, m)| m.name() == name) else {
        eprintln!("Module '{}' not found", name);
        return 1;
    };

    println!("Module: {} v{}", manifest.name(), manifest.module.version);
    if !manifest.module.category.is_empty() {
        println!("Category: {}", manifest.module.category);
    }
    if !manifest.module.description.is_empty() {
        println!("Description: {}", manifest.module.description);
    }
    if !manifest.module.mae_version.is_empty() {
        println!("MAE version: {}", manifest.module.mae_version);
    }
    if !manifest.module.authors.is_empty() {
        println!("Authors: {}", manifest.module.authors.join(", "));
    }
    if !manifest.module.license.is_empty() {
        println!("License: {}", manifest.module.license);
    }
    println!("Path: {}", path.display());
    println!(
        "Entry: {} / {}",
        manifest.entry.autoloads, manifest.entry.init
    );

    if !manifest.flags.is_empty() {
        println!();
        println!("Flags:");
        for (flag, def) in &manifest.flags {
            println!("  +{:<20} {}", flag, def.doc);
        }
    }

    if !manifest.dependencies.is_empty() {
        println!();
        println!("Dependencies:");
        for (dep, version) in &manifest.dependencies {
            println!("  {:<20} {}", dep, version);
        }
    }

    let autoloads_path = path.join(&manifest.entry.autoloads);
    if autoloads_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&autoloads_path) {
            let key_count = content.matches("define-key").count();
            let cmd_count = content.matches("define-command").count();
            let opt_count = content.matches("define-option!").count();
            let hook_count = content.matches("add-hook!").count();
            if key_count + cmd_count + opt_count + hook_count > 0 {
                println!();
                println!("Provides:");
                if key_count > 0 {
                    println!("  {} keybinding(s)", key_count);
                }
                if cmd_count > 0 {
                    println!("  {} command(s)", cmd_count);
                }
                if opt_count > 0 {
                    println!("  {} option(s)", opt_count);
                }
                if hook_count > 0 {
                    println!("  {} hook(s)", hook_count);
                }
            }
        }
    }

    0
}

// ── doctor ────────────────────────────────────────────────────────

fn cmd_doctor(name_filter: Option<&str>) -> i32 {
    let mut all_modules = Vec::new();
    for dir in module_search_dirs() {
        if dir.exists() {
            all_modules.extend(discover_modules(&dir));
        }
    }

    if all_modules.is_empty() {
        println!("No modules found.");
        return 0;
    }

    let current_version = env!("CARGO_PKG_VERSION");
    let mut errors = 0;

    for (path, manifest) in &all_modules {
        let name = manifest.name();
        if let Some(filter) = name_filter {
            if name != filter {
                continue;
            }
        }

        println!("Checking {} ({})...", name, path.display());

        if let Err(e) = manifest.check_mae_version(current_version) {
            println!("  WARNING: {}", e);
            errors += 1;
        }

        let autoloads = path.join(&manifest.entry.autoloads);
        if !autoloads.exists() {
            println!(
                "  WARNING: autoloads file '{}' not found",
                manifest.entry.autoloads
            );
            errors += 1;
        }
        let init = path.join(&manifest.entry.init);
        if !init.exists() {
            println!("  WARNING: init file '{}' not found", manifest.entry.init);
            errors += 1;
        }

        if manifest.module.version.is_empty() {
            println!("  WARNING: no version specified");
            errors += 1;
        }
        if manifest.module.description.is_empty() {
            println!("  WARNING: no description");
        }

        if errors == 0 {
            println!("  OK");
        }
    }

    if let Some(filter) = name_filter {
        if !all_modules.iter().any(|(_, m)| m.name() == filter) {
            eprintln!("Module '{}' not found", filter);
            return 1;
        }
    }

    println!();
    if errors > 0 {
        println!("{} warning(s) found", errors);
        1
    } else {
        println!("All modules OK");
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkg_help_returns_zero() {
        assert_eq!(run_pkg_cli(&["help".to_string()]), 0);
    }

    #[test]
    fn pkg_unknown_returns_one() {
        assert_eq!(run_pkg_cli(&["nonexistent".to_string()]), 1);
    }

    #[test]
    fn pkg_list_returns_zero() {
        let code = run_pkg_cli(&["list".to_string()]);
        assert_eq!(code, 0);
    }

    #[test]
    fn pkg_info_no_name_returns_one() {
        assert_eq!(run_pkg_cli(&["info".to_string()]), 1);
    }

    #[test]
    fn pkg_info_nonexistent_returns_one() {
        assert_eq!(
            run_pkg_cli(&["info".to_string(), "nonexistent".to_string()]),
            1
        );
    }

    #[test]
    fn pkg_create_no_name_returns_one() {
        assert_eq!(run_pkg_cli(&["create".to_string()]), 1);
    }

    #[test]
    fn dispatch_subcmd_help() {
        assert_eq!(dispatch_subcmd("help", &[]), 0);
    }

    #[test]
    fn dispatch_subcmd_unknown() {
        assert_eq!(dispatch_subcmd("nonexistent", &[]), 1);
    }
}
