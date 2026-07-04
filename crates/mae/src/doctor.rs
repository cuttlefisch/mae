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
        ("ruby-lsp", "Ruby", "gem install ruby-lsp"),
        (
            "yaml-language-server",
            "Yaml",
            "npm install -g yaml-language-server",
        ),
        (
            "vscode-json-language-server",
            "Json",
            "npm install -g vscode-langservers-extracted",
        ),
        ("taplo", "Toml", "cargo install taplo-cli --locked"),
        (
            "bash-language-server",
            "Bash",
            "npm install -g bash-language-server",
        ),
        // Label "Cpp" so the env-var check resolves to MAE_LSP_CPP (a "C/C++"
        // label would produce the invalid key MAE_LSP_C/C++). clangd serves both.
        (
            "clangd",
            "Cpp",
            "sudo dnf install clang-tools-extra (Fedora) / sudo apt install clangd (Debian) / brew install llvm (macOS)",
        ),
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
        ("codelldb", "C/C++/Rust (alt)", "install the CodeLLDB extension's codelldb binary, or set MAE_DAP_CODELLDB"),
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

    if check_binary("mae-daemon").is_some() {
        println!("  {} daemon binary: found", GREEN_CHECK);
    } else {
        println!("  {} daemon binary: not found", YELLOW_WARN);
        warnings += 1;
    }

    // Read collab options from the parsed config (uses load_config which is
    // already called at startup; here we re-parse for doctor's standalone context).
    let (doctor_cfg, _) = config::load_config();
    let collab_addr = doctor_cfg
        .collaboration
        .server_address
        .unwrap_or_else(|| mae_core::DEFAULT_COLLAB_ADDRESS.to_string());
    let collab_auto = doctor_cfg.collaboration.auto_connect.unwrap_or(false);
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

    // Service status: platform-specific detection
    #[cfg(target_os = "macos")]
    {
        // Check Homebrew service first
        let brew_status = Command::new("brew")
            .args(["services", "info", "mae", "--json"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok());
        if let Some(ref info) = brew_status {
            if info.contains("\"running\"") {
                println!("  {} daemon: running (brew services)", GREEN_CHECK);
            } else {
                println!(
                    "  {} daemon: not running — brew services start mae",
                    YELLOW_WARN
                );
            }
        } else {
            // Fall back to launchctl check
            let launchd = Command::new("launchctl")
                .args(["list", "com.cuttlefisch.mae-daemon"])
                .output()
                .ok()
                .map(|o| o.status.success());
            match launchd {
                Some(true) => println!("  {} daemon: running (launchd)", GREEN_CHECK),
                _ => println!(
                    "  {} daemon: not running — launchctl load ~/Library/LaunchAgents/com.cuttlefisch.mae-daemon.plist",
                    YELLOW_WARN
                ),
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let systemd_active = Command::new("systemctl")
            .args(["--user", "is-active", "mae-daemon"])
            .output()
            .ok()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if systemd_active {
            println!("  {} systemd user service: active", GREEN_CHECK);
        } else {
            println!(
                "  {} systemd user service: inactive — systemctl --user enable --now mae-daemon",
                YELLOW_WARN
            );
        }
    }

    // TCP reachability
    let tcp_reachable = std::net::TcpStream::connect_timeout(
        &collab_addr.parse().unwrap_or_else(|_| {
            std::net::SocketAddr::from(([127, 0, 0, 1], mae_core::DEFAULT_COLLAB_PORT))
        }),
        std::time::Duration::from_secs(2),
    )
    .is_ok();
    if tcp_reachable {
        println!("  {} TCP reachable: {}", GREEN_CHECK, collab_addr);
    } else {
        println!(
            "  {} TCP unreachable: {} — is mae-daemon listening?",
            RED_CROSS, collab_addr
        );
        errors += 1;
        println!("    Try: ss -tlnp | grep 9473");
    }

    // Firewall check (when bound to non-loopback)
    let is_loopback = collab_addr.starts_with("127.") || collab_addr.starts_with("localhost");
    if !is_loopback {
        if check_binary("firewall-cmd").is_some() {
            let port_open = Command::new("firewall-cmd")
                .args(["--query-port=9473/tcp"])
                .output()
                .ok()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if port_open {
                println!("  {} firewalld: port 9473/tcp open", GREEN_CHECK);
            } else {
                println!(
                    "  {} firewalld: port 9473/tcp not open — sudo firewall-cmd --add-port=9473/tcp --permanent && sudo firewall-cmd --reload",
                    YELLOW_WARN
                );
                warnings += 1;
            }
        } else if check_binary("ufw").is_some() {
            let ufw_open = Command::new("ufw")
                .args(["status"])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.contains("9473"))
                .unwrap_or(false);
            if ufw_open {
                println!("  {} ufw: port 9473 open", GREEN_CHECK);
            } else {
                println!(
                    "  {} ufw: port 9473 not open — sudo ufw allow 9473/tcp",
                    YELLOW_WARN
                );
                warnings += 1;
            }
        }

        // macOS Application Firewall check
        #[cfg(target_os = "macos")]
        {
            let fw = Command::new("/usr/libexec/ApplicationFirewall/socketfilterfw")
                .arg("--getglobalstate")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok());
            match fw {
                Some(ref s) if s.contains("enabled") => {
                    println!(
                        "  {} macOS firewall enabled — ensure mae-daemon is allowed in System Settings > Network > Firewall",
                        YELLOW_WARN
                    );
                    warnings += 1;
                }
                Some(_) => println!("  {} macOS firewall: disabled", GREEN_CHECK),
                None => {} // Can't check, skip
            }
        }

        let has_psk = doctor_cfg.collaboration.psk.is_some()
            || doctor_cfg.collaboration.psk_command.is_some();
        if has_psk {
            println!("  {} PSK authentication configured", GREEN_CHECK);
        } else {
            println!(
                "  {} No PSK configured — collab connections will be unauthenticated",
                YELLOW_WARN
            );
            warnings += 1;
        }
    }

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
