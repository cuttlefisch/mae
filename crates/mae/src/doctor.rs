//! `mae doctor` — unified diagnostic command (inspired by Doom Emacs's `doom doctor`).
//!
//! Checks build/runtime prerequisites, config validity, LSP/DAP availability,
//! and AI provider status. Prints human-readable output with colored checkmarks.

use std::path::PathBuf;
use std::process::Command;

use crate::config;

const GREEN_CHECK: &str = "\x1b[32m✓\x1b[0m";
const RED_CROSS: &str = "\x1b[31m✗\x1b[0m";
const YELLOW_WARN: &str = "\x1b[33m!\x1b[0m";

fn check_binary(name: &str) -> Option<String> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn binary_version(name: &str, flag: &str) -> Option<String> {
    Command::new(name)
        .arg(flag)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn section(title: &str) {
    println!("\n\x1b[1m{}\x1b[0m", title);
}

/// Run the doctor diagnostic and return exit code (0 = ok, 1 = errors found).
pub fn run_doctor() -> i32 {
    println!("mae doctor v{}\n", env!("CARGO_PKG_VERSION"));

    let mut errors = 0;
    let mut warnings = 0;

    // --- Build Prerequisites ---
    section("Build Prerequisites");

    // Rust
    if let Some(ver) = binary_version("rustc", "--version") {
        println!("  {} {}", GREEN_CHECK, ver);
    } else {
        println!(
            "  {} rustc not found — install via https://rustup.rs",
            RED_CROSS
        );
        errors += 1;
    }

    // Cargo
    if check_binary("cargo").is_some() {
        println!("  {} cargo", GREEN_CHECK);
    } else {
        println!("  {} cargo not found", RED_CROSS);
        errors += 1;
    }

    // clang (needed for GUI build / skia)
    if let Some(ver) = binary_version("clang", "--version") {
        let first_line = ver.lines().next().unwrap_or(&ver);
        println!("  {} clang ({})", GREEN_CHECK, first_line);
    } else {
        println!(
            "  {} clang not found — needed for GUI build. Install: sudo dnf install clang (Fedora) / sudo apt install clang (Debian)",
            YELLOW_WARN
        );
        warnings += 1;
    }

    // pkg-config
    if check_binary("pkg-config").is_some() {
        println!("  {} pkg-config", GREEN_CHECK);
    } else {
        println!("  {} pkg-config not found", YELLOW_WARN);
        warnings += 1;
    }

    // fontconfig (check via pkg-config)
    let has_fontconfig = Command::new("pkg-config")
        .args(["--exists", "fontconfig"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if has_fontconfig {
        println!("  {} fontconfig", GREEN_CHECK);
    } else {
        println!(
            "  {} fontconfig not found — needed for GUI. Install: sudo dnf install fontconfig-devel (Fedora) / sudo apt install libfontconfig1-dev (Debian)",
            YELLOW_WARN
        );
        warnings += 1;
    }

    // freetype (check via pkg-config)
    let has_freetype = Command::new("pkg-config")
        .args(["--exists", "freetype2"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if has_freetype {
        println!("  {} freetype", GREEN_CHECK);
    } else {
        println!(
            "  {} freetype not found — needed for GUI. Install: sudo dnf install freetype-devel (Fedora) / sudo apt install libfreetype6-dev (Debian)",
            YELLOW_WARN
        );
        warnings += 1;
    }

    // --- Configuration ---
    section("Configuration");

    let config_path = config::config_path();
    if config_path.exists() {
        println!("  {} config.toml ({})", GREEN_CHECK, config_path.display());
    } else {
        println!(
            "  {} config.toml not found — run `mae --init-config` to create one",
            YELLOW_WARN
        );
        warnings += 1;
    }

    let init_path = config_path
        .parent()
        .unwrap_or(&PathBuf::from("."))
        .join("init.scm");
    if init_path.exists() {
        println!("  {} init.scm ({})", GREEN_CHECK, init_path.display());
    } else {
        println!(
            "  {} init.scm not found — run `mae --init-config` to create one",
            YELLOW_WARN
        );
        warnings += 1;
    }

    // Validate config if it exists
    if config_path.exists() {
        let (app_config, parse_error) = config::load_config();
        if let Some(err) = &parse_error {
            println!("  {} config.toml: {}", RED_CROSS, err);
            errors += 1;
        } else {
            println!("  {} config.toml parses OK", GREEN_CHECK);
        }
        // Check AI provider
        if let Some(ref provider) = app_config.ai.provider {
            if !provider.is_empty() {
                println!("  {} AI provider: {}", GREEN_CHECK, provider);
            }
        }
    }

    // --- AI Provider ---
    section("AI Provider");

    let ai_keys = [
        ("ANTHROPIC_API_KEY", "Claude"),
        ("OPENAI_API_KEY", "OpenAI"),
        ("GEMINI_API_KEY", "Gemini"),
        ("DEEPSEEK_API_KEY", "DeepSeek"),
    ];

    let mut any_key = false;
    for (env_var, name) in &ai_keys {
        if std::env::var(env_var).is_ok() {
            println!("  {} {} API key set ({})", GREEN_CHECK, name, env_var);
            any_key = true;
        }
    }
    if !any_key {
        println!(
            "  {} No AI API key found — set ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY, or DEEPSEEK_API_KEY",
            YELLOW_WARN
        );
        warnings += 1;
    }

    // --- LSP Servers ---
    section("LSP Servers");

    let lsp_servers = [
        (
            "rust-analyzer",
            "Rust",
            "rustup component add rust-analyzer",
        ),
        ("pyright", "Python", "pip install pyright"),
        (
            "typescript-language-server",
            "TypeScript",
            "npm install -g typescript-language-server",
        ),
        ("gopls", "Go", "go install golang.org/x/tools/gopls@latest"),
    ];

    for (binary, lang, install) in &lsp_servers {
        // Check env var override first
        let env_key = format!("MAE_LSP_{}", lang.to_uppercase());
        if let Ok(path) = std::env::var(&env_key) {
            println!("  {} {} ({} = {})", GREEN_CHECK, lang, env_key, path);
        } else if check_binary(binary).is_some() {
            println!("  {} {} ({})", GREEN_CHECK, lang, binary);
        } else {
            println!("  {} {} not found — {}", YELLOW_WARN, lang, install);
        }
    }

    // --- DAP Adapters ---
    section("DAP Adapters");

    let dap_adapters = [
        ("lldb-dap", "C/C++/Rust", "sudo dnf install lldb (Fedora) / sudo apt install lldb (Debian) / brew install llvm (macOS)"),
        ("debugpy", "Python", "pip install debugpy"),
    ];

    for (binary, lang, install) in &dap_adapters {
        if check_binary(binary).is_some() {
            println!("  {} {} ({})", GREEN_CHECK, lang, binary);
        } else {
            println!("  {} {} not found — {}", YELLOW_WARN, lang, install);
        }
    }

    // --- Collaborative Editing ---
    section("Collaborative Editing");

    if check_binary("mae-state-server").is_some() {
        println!("  {} state-server binary: found", GREEN_CHECK);
    } else {
        println!("  {} state-server binary: not found", YELLOW_WARN);
        warnings += 1;
    }

    match std::env::var("MAE_STATE_SERVER") {
        Ok(val) => println!("  {} MAE_STATE_SERVER env: {}", GREEN_CHECK, val),
        Err(_) => println!("  {} MAE_STATE_SERVER env: not set", YELLOW_WARN),
    }

    // Read collab options from config.toml if present.
    // These options live in `[collaboration]` section of config.toml
    // and default via the OptionRegistry.
    let collab_addr = config_path
        .exists()
        .then(|| {
            std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|s| s.parse::<toml::Value>().ok())
                .and_then(|t| {
                    t.get("collaboration")
                        .and_then(|c| c.get("server_address"))
                        .and_then(|v| v.as_str().map(String::from))
                })
        })
        .flatten()
        .unwrap_or_else(|| "127.0.0.1:9473".to_string());
    let collab_auto = config_path
        .exists()
        .then(|| {
            std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|s| s.parse::<toml::Value>().ok())
                .and_then(|t| {
                    t.get("collaboration")
                        .and_then(|c| c.get("auto_connect"))
                        .and_then(|v| v.as_bool())
                })
        })
        .flatten()
        .unwrap_or(false);
    println!("  {} collab_server_address: {}", GREEN_CHECK, collab_addr);
    println!(
        "  {} collab_auto_connect: {}",
        if collab_auto {
            GREEN_CHECK
        } else {
            YELLOW_WARN
        },
        collab_auto
    );

    // --- Summary ---
    println!();
    if errors > 0 {
        println!(
            "\x1b[31m{} error(s)\x1b[0m, {} warning(s)",
            errors, warnings
        );
        1
    } else if warnings > 0 {
        println!(
            "\x1b[32m0 errors\x1b[0m, \x1b[33m{} warning(s)\x1b[0m",
            warnings
        );
        0
    } else {
        println!("\x1b[32mAll checks passed.\x1b[0m");
        0
    }
}
