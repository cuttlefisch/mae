//! # Module: pkg/cli.rs — Package manager CLI
//!
//! Implements `mae pkg <subcommand>` for offline module management.
//! These commands run without starting the editor.

use super::manifest::discover_modules;
use std::path::PathBuf;

/// Run the `mae pkg` CLI. Returns exit code.
pub fn run_pkg_cli(args: &[String]) -> i32 {
    let subcmd = args.first().map(|s| s.as_str()).unwrap_or("help");

    match subcmd {
        "list" => cmd_list(),
        "info" => cmd_info(args.get(1).map(|s| s.as_str())),
        "create" => cmd_create(args.get(1).map(|s| s.as_str())),
        "doctor" => cmd_doctor(args.get(1).map(|s| s.as_str())),
        "help" | "--help" | "-h" => {
            print_help();
            0
        }
        other => {
            eprintln!("Unknown subcommand: {}", other);
            eprintln!("Run `mae pkg help` for usage.");
            1
        }
    }
}

fn print_help() {
    println!("mae pkg — Module package manager");
    println!();
    println!("USAGE:");
    println!("  mae pkg <subcommand>");
    println!();
    println!("SUBCOMMANDS:");
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

fn cmd_create(name: Option<&str>) -> i32 {
    let Some(name) = name else {
        eprintln!("Usage: mae pkg create <NAME>");
        return 1;
    };

    // Determine target directory
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
    println!("  3. Run `mae pkg doctor {}` to validate", name);
    0
}

fn cmd_info(name: Option<&str>) -> i32 {
    let Some(name) = name else {
        eprintln!("Usage: mae pkg info <NAME>");
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

    // Show autoloads.scm contents summary (keybindings defined)
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

        // Check mae_version constraint
        if let Err(e) = manifest.check_mae_version(current_version) {
            println!("  WARNING: {}", e);
            errors += 1;
        }

        // Check entry points exist
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

        // Check required fields
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
        // May find modules or not depending on working directory
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
}
