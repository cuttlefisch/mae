//! # Module: pkg/cli.rs — Package manager CLI
//!
//! Implements both `mae pkg <subcommand>` (legacy) and flat top-level
//! subcommands: `mae sync`, `mae upgrade`, `mae purge`, `mae list`,
//! `mae info`, `mae create`, `mae doctor`.

use super::embedded::DiscoveredModule;
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
        "prune-shadows" => cmd_prune_shadows(args),
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
    println!("                    (to upgrade MAE itself, use top-level `mae upgrade`)");
    println!("  purge             Remove packages not declared in init.scm");
    println!("  list              List discovered modules and their status");
    println!("  info <NAME>       Show detailed information about a module");
    println!("  create <NAME>     Scaffold a new module directory");
    println!("  doctor [NAME]     Validate module manifests");
    println!("  prune-shadows     Remove stale on-disk module copies shadowing newer built-ins");
    println!("                    (dry-run by default; pass --force to delete)");
    println!("  help              Print this help");
}

/// Remove stale on-disk module copies that shadow a NEWER built-in (the
/// "upgraded the binary but ~/.local/share/mae/modules is stale" trap — same
/// detection as the `:messages` warning). Dry-run by default; `--force` deletes.
///
/// Scoped to the USER DATA module dirs only (XDG + platform data dir). It never
/// touches a dev repo/cwd module tree, and only REPORTS (never deletes) stale
/// copies under an explicit `MAE_MODULES_PATH` — that's a user-controlled
/// override (often a read-only app bundle), not ours to remove.
fn cmd_prune_shadows(args: &[String]) -> i32 {
    use crate::pkg::embedded::stale_embedded_shadows;

    let force = args
        .iter()
        .any(|a| matches!(a.as_str(), "--force" | "-f" | "--yes" | "-y"));

    // Deletable data dirs (installer/upgrade copies land here).
    let mut data_dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = crate::pkg::paths::data_dir_candidate("mae/modules") {
        data_dirs.push(d);
    }
    if let Some(d) = dirs::data_dir().map(|d| d.join("mae/modules")) {
        if !data_dirs.contains(&d) {
            data_dirs.push(d);
        }
    }

    let mut disk = Vec::new();
    for dir in &data_dirs {
        if dir.exists() {
            disk.extend(discover_modules(dir));
        }
    }
    let stale = stale_embedded_shadows(&disk);

    // Report-only: an explicit MAE_MODULES_PATH (e.g. an app bundle) we won't delete.
    if let Ok(p) = std::env::var("MAE_MODULES_PATH") {
        let override_dir = PathBuf::from(&p);
        if override_dir.exists() {
            let override_disk = discover_modules(&override_dir);
            let override_stale = stale_embedded_shadows(&override_disk);
            if !override_stale.is_empty() {
                println!(
                    "Note: MAE_MODULES_PATH ({p}) also has stale copies — not removed \
                     (user-controlled override). Refresh or unset it manually:"
                );
                for (name, dv, ev) in &override_stale {
                    println!("  {name} (v{dv} < built-in v{ev})");
                }
                println!();
            }
        }
    }

    if stale.is_empty() {
        println!("No stale built-in module copies found in user data module dirs.");
        return 0;
    }

    let stale_names: std::collections::HashSet<&str> =
        stale.iter().map(|(n, _, _)| n.as_str()).collect();
    let mut targets: Vec<(String, PathBuf, String, String)> = Vec::new();
    for d in &disk {
        if stale_names.contains(d.manifest.name()) {
            if let Some(path) = d.source.disk_dir() {
                if let Some((_, dv, ev)) = stale.iter().find(|(n, _, _)| n == d.manifest.name()) {
                    targets.push((
                        d.manifest.name().to_string(),
                        path.to_path_buf(),
                        dv.clone(),
                        ev.clone(),
                    ));
                }
            }
        }
    }

    println!("Stale on-disk module copies shadowing newer built-ins:");
    for (name, path, dv, ev) in &targets {
        println!("  {name} (v{dv} < built-in v{ev})  {}", path.display());
    }
    println!();

    if !force {
        println!(
            "Dry run — re-run `mae prune-shadows --force` to delete the {} director(ies) above.",
            targets.len()
        );
        return 0;
    }

    let mut removed = 0;
    for (name, path, _, _) in &targets {
        print!("  Removing {name} ({})...", path.display());
        match std::fs::remove_dir_all(path) {
            Ok(()) => {
                println!(" done");
                removed += 1;
            }
            Err(e) => eprintln!(" failed: {e}"),
        }
    }
    println!(
        "\n{removed} stale module copy(ies) removed. Restart MAE (or run :reload-modules) \
         to pick up the built-ins."
    );
    0
}

fn module_search_dirs() -> Vec<PathBuf> {
    // Reuse the editor's canonical module search path so `mae list`/`doctor`/…
    // report exactly what the editor would discover and load — previously this
    // checked only `./modules` + the user packages dir, so the CLI was blind to
    // tarball/Homebrew/data-dir installs and reported "No modules found" even
    // when the editor loaded them fine.
    let mut dirs = crate::bootstrap::builtin_module_dirs();
    if let Some(user_pkg) = crate::bootstrap::dirs_candidate("mae/packages") {
        if !dirs.contains(&user_pkg) {
            dirs.push(user_pkg);
        }
    }
    dirs
}

/// Discover every module the editor would see: embedded built-in baseline plus
/// on-disk overrides (by name), deduplicated. Used by `list`/`info`/`doctor` so
/// the CLI reflects exactly what the editor loads — including embedded-only
/// installs (where the on-disk search would otherwise find nothing).
fn discover_all_modules() -> Vec<DiscoveredModule> {
    let mut disk = Vec::new();
    for dir in module_search_dirs() {
        if dir.exists() {
            disk.extend(discover_modules(&dir));
        }
    }
    crate::pkg::embedded::merge_modules(disk)
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

        if source.is_local() {
            // Local source — create symlink instead of cloning.
            let local_path = match &source {
                PackageSource::Local(p) => p.clone(),
                _ => unreachable!(),
            };
            if !target.exists() {
                print!("  Linking {}...", pkg.name);
                if let Err(e) = std::fs::create_dir_all(&pkg_dir) {
                    eprintln!(" failed to create packages dir: {}", e);
                    errors += 1;
                    continue;
                }
                let abs_path = if local_path.is_relative() {
                    std::env::current_dir()
                        .unwrap_or_default()
                        .join(&local_path)
                } else {
                    local_path.clone()
                };
                #[cfg(unix)]
                {
                    match std::os::unix::fs::symlink(&abs_path, &target) {
                        Ok(()) => println!(" done (→ {})", abs_path.display()),
                        Err(e) => {
                            eprintln!(" symlink failed: {}", e);
                            errors += 1;
                            continue;
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    eprintln!(" path: sources require Unix (symlink support)");
                    errors += 1;
                    continue;
                }
            }
            lockfile.pin(&pkg.name, source_spec, "local", "");
            synced += 1;
        } else if !target.exists() {
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

            // Update lockfile
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
    }

    // Also run doctor validation
    let all_modules = discover_all_modules();

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

pub(crate) fn cmd_upgrade() -> i32 {
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
    let all_modules = discover_all_modules();

    if all_modules.is_empty() {
        println!("No modules found.");
        return 0;
    }

    println!("{:<20} {:<10} {:<12} Path", "Module", "Version", "Category");
    println!("{:<20} {:<10} {:<12} ----", "------", "-------", "--------");

    for d in &all_modules {
        let manifest = &d.manifest;
        println!(
            "{:<20} {:<10} {:<12} {}",
            manifest.name(),
            manifest.module.version,
            if manifest.module.category.is_empty() {
                "-"
            } else {
                &manifest.module.category
            },
            d.source.label("")
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

    let all_modules = discover_all_modules();

    let Some(d) = all_modules.iter().find(|d| d.manifest.name() == name) else {
        eprintln!("Module '{}' not found", name);
        return 1;
    };
    let manifest = &d.manifest;

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
    if !manifest.module.homepage.is_empty() {
        println!("Homepage: {}", manifest.module.homepage);
    }
    if !manifest.module.repository.is_empty() {
        println!("Repository: {}", manifest.module.repository);
    }
    if !manifest.module.keywords.is_empty() {
        println!("Keywords: {}", manifest.module.keywords.join(", "));
    }
    println!("Path: {}", d.source.label(""));
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

    if let Some(content) = d.source.read_relative(&manifest.entry.autoloads) {
        {
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
    let all_modules = discover_all_modules();

    if all_modules.is_empty() {
        println!("No modules found.");
        return 0;
    }

    let current_version = env!("CARGO_PKG_VERSION");
    let mut errors = 0;

    for d in &all_modules {
        let manifest = &d.manifest;
        let name = manifest.name();
        if let Some(filter) = name_filter {
            if name != filter {
                continue;
            }
        }

        println!("Checking {} ({})...", name, d.source.label(""));

        if let Err(e) = manifest.check_mae_version(current_version) {
            println!("  WARNING: {}", e);
            errors += 1;
        }

        if !d.source.has_relative(&manifest.entry.autoloads) {
            println!(
                "  WARNING: autoloads file '{}' not found",
                manifest.entry.autoloads
            );
            errors += 1;
        }
        if !d.source.has_relative(&manifest.entry.init) {
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
        if !all_modules.iter().any(|d| d.manifest.name() == filter) {
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
