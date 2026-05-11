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
}
