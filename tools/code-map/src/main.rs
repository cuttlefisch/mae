//! MAE Code Map Generator
//!
//! Generates `docs/CODE_MAP.md` (human-readable with Mermaid diagrams)
//! and `docs/CODE_MAP.json` (machine-readable) from the workspace.
//!
//! Usage:
//!   cd tools/code-map && cargo run --release -- --workspace-root ../..
//!   cd tools/code-map && cargo run --release -- --workspace-root ../.. --check

use cargo_metadata::MetadataCommand;
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
struct CodeMap {
    crates: BTreeMap<String, CrateInfo>,
    scheme_primitives: Vec<SchemePrimitive>,
    scheme_globals: Vec<SchemeGlobal>,
    commands: Vec<BuiltinCommand>,
}

#[derive(Debug, Serialize)]
struct CrateInfo {
    path: String,
    dependencies: Vec<String>,
    public_items: Vec<PublicItem>,
}

#[derive(Debug, Serialize)]
struct PublicItem {
    name: String,
    kind: String, // "struct", "enum", "trait", "fn", "type", "const", "mod"
}

#[derive(Debug, Serialize)]
struct SchemePrimitive {
    name: String,
    source: String,
}

#[derive(Debug, Serialize)]
struct SchemeGlobal {
    name: String,
}

#[derive(Debug, Serialize)]
struct BuiltinCommand {
    name: String,
    doc: String,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut workspace_root = PathBuf::from("../..");
    let mut check_mode = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--workspace-root" => {
                i += 1;
                workspace_root = PathBuf::from(&args[i]);
            }
            "--check" => {
                check_mode = true;
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let workspace_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|e| panic!("Cannot resolve workspace root: {}", e));

    let map = generate_code_map(&workspace_root);
    let md = render_markdown(&map);
    let json = serde_json::to_string_pretty(&map).expect("JSON serialization failed");

    let docs_dir = workspace_root.join("docs");
    fs::create_dir_all(&docs_dir).expect("Cannot create docs/");

    let md_path = docs_dir.join("CODE_MAP.md");
    let json_path = docs_dir.join("CODE_MAP.json");

    if check_mode {
        let existing_json = fs::read_to_string(&json_path).unwrap_or_default();
        if existing_json.trim() == json.trim() {
            println!("Code map is up to date.");
            std::process::exit(0);
        } else {
            eprintln!("Code map is stale. Run `make code-map` to update.");
            std::process::exit(1);
        }
    }

    fs::write(&md_path, &md).expect("Cannot write CODE_MAP.md");
    fs::write(&json_path, &json).expect("Cannot write CODE_MAP.json");
    println!(
        "Generated:\n  {}\n  {}",
        md_path.display(),
        json_path.display()
    );
    println!(
        "  {} crates, {} public items, {} Scheme primitives, {} commands",
        map.crates.len(),
        map.crates.values().map(|c| c.public_items.len()).sum::<usize>(),
        map.scheme_primitives.len(),
        map.commands.len(),
    );
}

fn generate_code_map(workspace_root: &Path) -> CodeMap {
    // 1. Run cargo metadata.
    let metadata = MetadataCommand::new()
        .manifest_path(workspace_root.join("Cargo.toml"))
        .exec()
        .expect("cargo metadata failed");

    let workspace_members: Vec<_> = metadata
        .workspace_members
        .iter()
        .map(|id| id.to_string())
        .collect();

    let mut crates = BTreeMap::new();

    for pkg in &metadata.packages {
        if !workspace_members.iter().any(|m| m == &pkg.id.to_string()) {
            continue;
        }

        // Get dependencies that are also workspace members.
        let deps: Vec<String> = pkg
            .dependencies
            .iter()
            .filter(|d| {
                metadata
                    .packages
                    .iter()
                    .any(|p| p.name == d.name && workspace_members.iter().any(|m| m == &p.id.to_string()))
            })
            .map(|d| d.name.clone())
            .collect();

        // Find the crate's source files.
        let manifest_dir = pkg
            .manifest_path
            .parent()
            .map(|p| PathBuf::from(p.as_std_path()))
            .unwrap_or_default();

        let lib_path = manifest_dir.join("src/lib.rs");
        let main_path = manifest_dir.join("src/main.rs");

        let entry = if lib_path.exists() {
            lib_path
        } else if main_path.exists() {
            main_path
        } else {
            continue;
        };

        let public_items = extract_public_items(&entry);

        let rel_path = entry
            .strip_prefix(workspace_root)
            .unwrap_or(&entry)
            .display()
            .to_string();

        crates.insert(
            pkg.name.clone(),
            CrateInfo {
                path: rel_path,
                dependencies: deps,
                public_items,
            },
        );
    }

    // 2. Extract Scheme primitives.
    let runtime_path = workspace_root.join("crates/scheme/src/runtime.rs");
    let (scheme_primitives, scheme_globals) = extract_scheme_api(&runtime_path);

    // 3. Extract built-in commands.
    let commands_path = workspace_root.join("crates/core/src/commands.rs");
    let commands = extract_commands(&commands_path);

    CodeMap {
        crates,
        scheme_primitives,
        scheme_globals,
        commands,
    }
}

fn extract_public_items(path: &Path) -> Vec<PublicItem> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(file) = syn::parse_file(&content) else {
        eprintln!("Warning: failed to parse {}", path.display());
        return Vec::new();
    };

    let mut items = Vec::new();

    for item in &file.items {
        match item {
            syn::Item::Struct(s) if matches!(s.vis, syn::Visibility::Public(_)) => {
                items.push(PublicItem {
                    name: s.ident.to_string(),
                    kind: "struct".to_string(),
                });
            }
            syn::Item::Enum(e) if matches!(e.vis, syn::Visibility::Public(_)) => {
                items.push(PublicItem {
                    name: e.ident.to_string(),
                    kind: "enum".to_string(),
                });
            }
            syn::Item::Trait(t) if matches!(t.vis, syn::Visibility::Public(_)) => {
                items.push(PublicItem {
                    name: t.ident.to_string(),
                    kind: "trait".to_string(),
                });
            }
            syn::Item::Fn(f) if matches!(f.vis, syn::Visibility::Public(_)) => {
                items.push(PublicItem {
                    name: f.sig.ident.to_string(),
                    kind: "fn".to_string(),
                });
            }
            syn::Item::Type(t) if matches!(t.vis, syn::Visibility::Public(_)) => {
                items.push(PublicItem {
                    name: t.ident.to_string(),
                    kind: "type".to_string(),
                });
            }
            syn::Item::Const(c) if matches!(c.vis, syn::Visibility::Public(_)) => {
                items.push(PublicItem {
                    name: c.ident.to_string(),
                    kind: "const".to_string(),
                });
            }
            syn::Item::Mod(m) if matches!(m.vis, syn::Visibility::Public(_)) => {
                items.push(PublicItem {
                    name: m.ident.to_string(),
                    kind: "mod".to_string(),
                });
            }
            _ => {}
        }
    }

    items
}

fn extract_scheme_api(path: &Path) -> (Vec<SchemePrimitive>, Vec<SchemeGlobal>) {
    let Ok(content) = fs::read_to_string(path) else {
        return (Vec::new(), Vec::new());
    };

    let source = path
        .strip_prefix(path.ancestors().nth(4).unwrap_or(path))
        .unwrap_or(path)
        .display()
        .to_string();

    // Match register_fn("name", ...) or register_fn( "name", ...)
    let fn_re = Regex::new(r#"register_fn\s*\(\s*"([^"]+)""#).unwrap();
    let primitives: Vec<SchemePrimitive> = fn_re
        .captures_iter(&content)
        .map(|cap| SchemePrimitive {
            name: cap[1].to_string(),
            source: source.clone(),
        })
        .collect();

    // Match define("*name*", ...) for injected globals.
    let global_re = Regex::new(r#"define\s*\(\s*"\*([^"]+)\*""#).unwrap();
    let mut globals: Vec<SchemeGlobal> = global_re
        .captures_iter(&content)
        .map(|cap| SchemeGlobal {
            name: format!("*{}*", &cap[1]),
        })
        .collect();
    globals.sort_by(|a, b| a.name.cmp(&b.name));
    globals.dedup_by(|a, b| a.name == b.name);

    (primitives, globals)
}

fn extract_commands(path: &Path) -> Vec<BuiltinCommand> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };

    let re = Regex::new(r#"register_builtin\s*\(\s*"([^"]+)"\s*,\s*"([^"]+)""#).unwrap();
    re.captures_iter(&content)
        .map(|cap| BuiltinCommand {
            name: cap[1].to_string(),
            doc: cap[2].to_string(),
        })
        .collect()
}

fn render_markdown(map: &CodeMap) -> String {
    let mut out = String::new();

    out.push_str("# MAE Code Map\n\n");
    out.push_str("Auto-generated by `make code-map`. Do not edit manually.\n\n");

    // Mermaid dependency diagram.
    out.push_str("## Crate Dependencies\n\n");
    out.push_str("```mermaid\ngraph TD\n");
    for (name, info) in &map.crates {
        for dep in &info.dependencies {
            out.push_str(&format!("    {} --> {}\n", mermaid_id(name), mermaid_id(dep)));
        }
        if info.dependencies.is_empty() {
            out.push_str(&format!("    {}[{}]\n", mermaid_id(name), name));
        }
    }
    out.push_str("```\n\n");

    // Per-crate public API tables.
    for (name, info) in &map.crates {
        out.push_str(&format!("## {}\n\n", name));
        out.push_str(&format!("Source: `{}`\n\n", info.path));
        if !info.public_items.is_empty() {
            out.push_str("| Item | Kind |\n|------|------|\n");
            for item in &info.public_items {
                out.push_str(&format!("| `{}` | {} |\n", item.name, item.kind));
            }
            out.push('\n');
        }
    }

    // Scheme API.
    if !map.scheme_primitives.is_empty() {
        out.push_str("## Scheme API\n\n");
        out.push_str("### Primitives (Rust -> Scheme)\n\n");
        out.push_str("| Function | Source |\n|----------|--------|\n");
        for p in &map.scheme_primitives {
            out.push_str(&format!("| `{}` | `{}` |\n", p.name, p.source));
        }
        out.push('\n');
    }

    if !map.scheme_globals.is_empty() {
        out.push_str("### Injected Globals\n\n");
        out.push_str("| Variable |\n|----------|\n");
        for g in &map.scheme_globals {
            out.push_str(&format!("| `{}` |\n", g.name));
        }
        out.push('\n');
    }

    // Commands.
    if !map.commands.is_empty() {
        out.push_str(&format!("## Commands ({} built-in)\n\n", map.commands.len()));
        out.push_str("| Command | Documentation |\n|---------|---------------|\n");
        for cmd in &map.commands {
            out.push_str(&format!("| `{}` | {} |\n", cmd.name, cmd.doc));
        }
        out.push('\n');
    }

    out
}

/// Convert a crate name to a valid Mermaid node ID.
fn mermaid_id(name: &str) -> String {
    name.replace('-', "_")
}
