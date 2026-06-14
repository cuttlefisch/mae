//! `mae upgrade` — channel-aware self-upgrade for MAE itself.
//!
//! Doom's `doom upgrade` is single-mechanism (git pull + sync) because Doom is
//! always a git checkout. MAE is emacs+doom in one — a full Rust build + manual
//! KB + daemon + modules — distributed across Homebrew, release tarballs
//! (`install.sh`), source checkouts, and cargo. So `mae upgrade` is a *channel
//! orchestrator*: it version-checks against GitHub releases, gates on
//! compatibility, runs preflight state checks, then **delegates to the existing
//! installer for the detected channel** rather than hand-rolling binary
//! self-replacement (which would leave `mae-mcp-shim`, `mae-daemon`, the manual
//! KB, and the `.app` stale).
//!
//! Package upgrades (the old `mae upgrade` behavior) now live under
//! `mae pkg upgrade` / `mae sync`; `mae upgrade --packages` is a shim.

use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;

use crate::config;

const GREEN: &str = "\x1b[32m✓\x1b[0m";
const RED: &str = "\x1b[31m✗\x1b[0m";
const YELLOW: &str = "\x1b[33m!\x1b[0m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

/// GitHub repo slug for the releases API.
const REPO: &str = "cuttlefisch/mae";

// ===========================================================================
// Types
// ===========================================================================

/// Which install channel the running binary came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    /// Homebrew formula (`cask: false`) or cask GUI bundle (`cask: true`).
    Homebrew { cask: bool },
    /// Release tarball installed via the bundled `install.sh`.
    Tarball,
    /// A git source checkout built with `make install`.
    SourceCheckout,
    /// `cargo install` into `CARGO_HOME/bin`.
    Cargo,
    /// Can't classify safely (e.g. running AppImage) — guided, never mutated.
    Unknown,
}

impl Channel {
    fn label(&self) -> &'static str {
        match self {
            Channel::Homebrew { cask: false } => "Homebrew (formula)",
            Channel::Homebrew { cask: true } => "Homebrew (cask)",
            Channel::Tarball => "release tarball (install.sh)",
            Channel::SourceCheckout => "source checkout (make)",
            Channel::Cargo => "cargo install",
            Channel::Unknown => "unknown",
        }
    }
}

/// Whether an upgrade across a version range is allowed automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatGate {
    /// Compatible — safe to auto-upgrade.
    AutoOk,
    /// A breaking change appears in the changelog range — refuse, route to manual.
    RefuseBreaking,
    /// Crosses a major version — refuse, route to manual.
    RefuseMajor,
}

/// Parsed `mae upgrade` flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct UpgradeOpts {
    /// Report availability only; never mutate.
    check: bool,
    /// Skip the confirmation prompt.
    yes: bool,
    /// Delegate to package upgrade (old `mae upgrade` behavior).
    packages: bool,
}

/// Outcome of argument parsing.
#[derive(Debug, PartialEq, Eq)]
enum ArgParse {
    Opts(UpgradeOpts),
    Help,
    Unknown(String),
}

/// A single preflight check result.
struct Check {
    ok: bool,
    /// Error-level checks block the upgrade; non-error are warnings.
    error: bool,
    msg: String,
    fix: Option<String>,
}

/// Snapshot of the install layout used by preflight + execution.
struct InstallState {
    /// Running exe at its symlink location (used to derive the install PREFIX).
    exe_path: PathBuf,
    /// Directory containing the running binary.
    bin_dir: PathBuf,
    /// Whether `bin_dir` is writable by the current user.
    bin_dir_writable: bool,
    /// Sibling binaries (`mae-mcp-shim`, `mae-daemon`) present next to the exe.
    siblings_present: bool,
    /// MAE data dir (manual KB / modules live here).
    data_dir: Option<PathBuf>,
    /// Source-checkout repo root, if this is a source build.
    repo_root: Option<PathBuf>,
}

// ===========================================================================
// Pure helpers (unit-tested; no I/O)
// ===========================================================================

/// Classify the install channel from the canonicalized exe path, the Homebrew
/// prefix (if brew is installed), and whether the exe lives in a source tree.
fn classify_channel_from(exe: &Path, brew_prefix: Option<&Path>, has_source_tree: bool) -> Channel {
    let s = exe.to_string_lossy();
    // cargo install location wins — it's an unmanaged channel.
    if s.contains("/.cargo/bin/") || s.contains("/cargo/bin/") {
        return Channel::Cargo;
    }
    // Homebrew: a cask puts the binary under .../Caskroom/.../MAE.app; a formula
    // under .../Cellar/mae/.../bin. Also accept the configured brew prefix and
    // the well-known prefixes (covers symlinked bins resolved to their target).
    let brew_match = brew_prefix.is_some_and(|p| exe.starts_with(p))
        || s.contains("/Cellar/")
        || s.contains("/Caskroom/")
        || s.contains("/.linuxbrew/")
        || s.contains("/opt/homebrew/");
    if brew_match {
        return Channel::Homebrew {
            cask: s.contains("/Caskroom/") || s.contains(".app/Contents/MacOS/"),
        };
    }
    if has_source_tree {
        return Channel::SourceCheckout;
    }
    // Default: a normal installed binary is an install.sh/tarball install.
    Channel::Tarball
}

/// Parse the `tag_name` of a GitHub release JSON into a semver `Version`
/// (strips a leading `v`).
fn parse_latest_tag(json: &str) -> Result<Version, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid release JSON: {e}"))?;
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "release JSON missing tag_name".to_string())?;
    let stripped = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(stripped).map_err(|e| format!("unparseable version '{tag}': {e}"))
}

/// Decide whether `current -> target` may auto-upgrade.
fn compat_decision(current: &Version, target: &Version, breaking_in_range: bool) -> CompatGate {
    if target.major > current.major {
        return CompatGate::RefuseMajor;
    }
    if breaking_in_range {
        return CompatGate::RefuseBreaking;
    }
    CompatGate::AutoOk
}

/// Scan concatenated release-notes / changelog text for breaking-change markers
/// (conventional-commit `BREAKING CHANGE:` / `!:`, or a "Breaking" heading).
fn scan_notes_for_breaking(notes: &str) -> bool {
    let lower = notes.to_lowercase();
    if lower.contains("breaking change") {
        return true;
    }
    // A markdown heading mentioning "breaking" (git-cliff's breaking group).
    if lower
        .lines()
        .any(|l| l.trim_start().starts_with('#') && l.contains("breaking"))
    {
        return true;
    }
    // Conventional-commit breaking marker: a `type!:` subject (e.g. `feat!:`).
    lower.lines().any(|l| {
        let t = l.trim_start_matches(['-', '*', ' ']);
        // crude but effective: a "word!:" near the start of a bullet/line
        t.split_whitespace()
            .next()
            .is_some_and(|w| w.ends_with("!:"))
    })
}

/// Map (os, arch) to the self-upgrade release asset (the `install.sh`-bundling
/// tarball — never the GUI `.zip`/`.AppImage`). `None` = no prebuilt asset.
fn select_asset(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Some("mae-macos-aarch64.tar.gz"),
        ("linux", "x86_64") => Some("mae-linux-x86_64.tar.gz"),
        _ => None,
    }
}

/// Parse `mae upgrade` arguments.
fn parse_upgrade_args(args: &[String]) -> ArgParse {
    let mut opts = UpgradeOpts::default();
    for a in args {
        match a.as_str() {
            "--check" | "-n" => opts.check = true,
            "--yes" | "-y" => opts.yes = true,
            "--packages" | "-p" => opts.packages = true,
            "--help" | "-h" => return ArgParse::Help,
            other => return ArgParse::Unknown(other.to_string()),
        }
    }
    ArgParse::Opts(opts)
}

// ===========================================================================
// I/O wrappers
// ===========================================================================

/// `curl` a URL and return the body, or an error (offline / non-2xx / no curl).
fn curl_get(url: &str) -> Result<String, String> {
    let mut cmd = Command::new("curl");
    cmd.args([
        "-fsSL",
        "--max-time",
        "15",
        "-H",
        "Accept: application/vnd.github+json",
    ]);
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            cmd.arg("-H").arg(format!("Authorization: Bearer {token}"));
        }
    }
    cmd.arg(url);
    let output = cmd
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "request failed ({}): {}",
            url,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Fetch the latest release: returns (version, full release JSON).
fn fetch_latest_release() -> Result<(Version, serde_json::Value), String> {
    let body = curl_get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))?;
    let version = parse_latest_tag(&body)?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("invalid release JSON: {e}"))?;
    Ok((version, json))
}

/// Fetch release notes for every release with `current < tag <= target` and
/// concatenate their bodies (for breaking-change scanning). Best-effort: on a
/// fetch error returns an empty string (caller treats "no notes" as no marker).
fn fetch_notes_in_range(current: &Version, target: &Version) -> String {
    let body = match curl_get(&format!(
        "https://api.github.com/repos/{REPO}/releases?per_page=100"
    )) {
        Ok(b) => b,
        Err(_) => return String::new(),
    };
    let Ok(releases) = serde_json::from_str::<serde_json::Value>(&body) else {
        return String::new();
    };
    let Some(arr) = releases.as_array() else {
        return String::new();
    };
    let mut out = String::new();
    for rel in arr {
        let Some(tag) = rel.get("tag_name").and_then(|t| t.as_str()) else {
            continue;
        };
        let stripped = tag.strip_prefix('v').unwrap_or(tag);
        if let Ok(v) = Version::parse(stripped) {
            if &v > current && &v <= target {
                if let Some(notes) = rel.get("body").and_then(|b| b.as_str()) {
                    out.push_str(notes);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Compute the SHA-256 of a byte slice as lowercase hex (reliable, via `sha2`).
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Detect a source-checkout: walk up from `exe` looking for a repo root that has
/// both a `.git` dir and a `Makefile` (the `make install-upgrade` target lives
/// there). Returns the repo root if found.
fn find_source_root(exe: &Path) -> Option<PathBuf> {
    let mut dir = exe.parent();
    while let Some(d) = dir {
        if d.join(".git").exists() && d.join("Makefile").exists() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Whether a directory is writable by the current user (best-effort: try to
/// create and remove a temp file).
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(".mae-upgrade-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Detect the install channel and gather install state.
fn detect_channel() -> (Channel, InstallState) {
    // AppImage self-replacement is unsupported (roadmap) — treat as guided.
    let is_appimage = std::env::var_os("APPIMAGE").is_some();

    let exe_raw = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mae"));
    let exe_canon = std::fs::canonicalize(&exe_raw).unwrap_or_else(|_| exe_raw.clone());
    let bin_dir = exe_raw
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let brew_prefix = config::brew_prefix();
    let repo_root = find_source_root(&exe_canon);
    let channel = if is_appimage {
        Channel::Unknown
    } else {
        classify_channel_from(&exe_canon, brew_prefix.as_deref(), repo_root.is_some())
    };

    let siblings_present =
        bin_dir.join("mae-mcp-shim").exists() || bin_dir.join("mae-daemon").exists();
    let data_dir = crate::pkg::paths::data_dir_candidate("mae");

    let state = InstallState {
        exe_path: exe_raw,
        bin_dir: bin_dir.clone(),
        bin_dir_writable: dir_writable(&bin_dir),
        siblings_present,
        data_dir,
        repo_root,
    };
    (channel, state)
}

/// Run preflight checks for the detected channel. Error-level failures block.
fn preflight(state: &InstallState, channel: &Channel) -> Vec<Check> {
    let mut checks = Vec::new();

    // Running binary resolvable.
    checks.push(Check {
        ok: state.exe_path.exists(),
        error: true,
        msg: format!("running binary: {}", state.exe_path.display()),
        fix: Some("could not resolve the running executable".to_string()),
    });

    // Bin dir writable (brew manages its own perms, so warn-only there).
    let writable_is_error = !matches!(channel, Channel::Homebrew { .. });
    checks.push(Check {
        ok: state.bin_dir_writable,
        error: writable_is_error,
        msg: format!(
            "binary directory writable: {} ({})",
            state.bin_dir.display(),
            if state.bin_dir_writable { "yes" } else { "no" }
        ),
        fix: Some(format!(
            "re-run with elevated permissions or reinstall into a writable prefix ({})",
            state.bin_dir.display()
        )),
    });

    // Sibling binaries (warn — TUI-only installs may lack the daemon).
    checks.push(Check {
        ok: state.siblings_present,
        error: false,
        msg: format!(
            "companion binaries (mae-mcp-shim/mae-daemon) present: {}",
            if state.siblings_present { "yes" } else { "no" }
        ),
        fix: None,
    });

    // Data dir + manual KB.
    let manual_ok = state
        .data_dir
        .as_ref()
        .map(|d| crate::manual_kb::locate_and_validate(d, None).is_some())
        .unwrap_or(false);
    checks.push(Check {
        ok: manual_ok,
        error: false,
        msg: format!(
            "manual knowledge base present: {}",
            if manual_ok { "yes" } else { "no" }
        ),
        fix: Some("the upgrade will reinstall it".to_string()),
    });

    // Modules dir.
    let modules_ok = crate::pkg::paths::builtin_module_dirs()
        .iter()
        .any(|d| d.exists());
    checks.push(Check {
        ok: modules_ok,
        error: false,
        msg: format!(
            "modules directory present: {}",
            if modules_ok { "yes" } else { "no" }
        ),
        fix: None,
    });

    // Required tooling per channel.
    let tools: &[&str] = match channel {
        Channel::Homebrew { .. } => &["brew"],
        Channel::Tarball => &["curl", "tar"],
        Channel::SourceCheckout => &["git", "make", "cargo"],
        Channel::Cargo => &["cargo"],
        Channel::Unknown => &[],
    };
    for tool in tools {
        let present = tool_present(tool);
        checks.push(Check {
            ok: present,
            error: true,
            msg: format!(
                "required tool '{tool}': {}",
                if present { "found" } else { "missing" }
            ),
            fix: Some(format!("install '{tool}' and retry")),
        });
    }

    // Source-only: clean working tree.
    if let (Channel::SourceCheckout, Some(root)) = (channel, &state.repo_root) {
        let clean = crate::pkg::git::is_clean(root).unwrap_or(false);
        checks.push(Check {
            ok: clean,
            error: true,
            msg: format!("source tree clean: {}", if clean { "yes" } else { "no" }),
            fix: Some("commit or stash local changes before upgrading from source".to_string()),
        });
    }

    // Disk space (warn-only).
    if let Some(avail_mb) = avail_disk_mb(&state.bin_dir) {
        checks.push(Check {
            ok: avail_mb >= 200,
            error: false,
            msg: format!("free disk space: {avail_mb} MB"),
            fix: Some("free up space (~200 MB recommended) before upgrading".to_string()),
        });
    }

    checks
}

/// Is a tool on PATH? (`which`/`command -v`.)
fn tool_present(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Available disk space in MB for the filesystem holding `dir` (via `df -k`).
fn avail_disk_mb(dir: &Path) -> Option<u64> {
    let output = Command::new("df").arg("-k").arg(dir).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // Second line, 4th column = available 1K-blocks (BSD/GNU compatible enough).
    let line = text.lines().nth(1)?;
    let avail_k: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail_k / 1024)
}

// ===========================================================================
// Execution (one path per channel)
// ===========================================================================

/// Run a command inheriting stdio; return its exit code (or 1 on spawn error).
fn run_inherit(mut cmd: Command, what: &str) -> i32 {
    match cmd.status() {
        Ok(s) => s.code().unwrap_or(if s.success() { 0 } else { 1 }),
        Err(e) => {
            eprintln!("{RED} failed to run {what}: {e}");
            1
        }
    }
}

fn run_brew_upgrade(cask: bool) -> i32 {
    let mut update = Command::new("brew");
    update.arg("update");
    let _ = run_inherit(update, "brew update");
    let mut up = Command::new("brew");
    if cask {
        up.args(["upgrade", "--cask", "mae"]);
    } else {
        up.args(["upgrade", "mae"]);
    }
    run_inherit(up, "brew upgrade")
}

fn run_source_upgrade(root: &Path) -> i32 {
    println!("Pulling latest source (fast-forward only)…");
    if let Err(e) = crate::pkg::git::pull_ff_only(root) {
        eprintln!("{RED} {e}");
        eprintln!("    Resolve the divergence manually (git pull / rebase) and retry.");
        return 1;
    }
    println!("Rebuilding and reinstalling (make install-upgrade)…");
    let mut make = Command::new("make");
    make.arg("install-upgrade").current_dir(root);
    run_inherit(make, "make install-upgrade")
}

/// Download + verify + extract the release tarball, then run its bundled
/// `install.sh` with the existing install PREFIX.
fn run_tarball_upgrade(state: &InstallState, release: &serde_json::Value) -> i32 {
    let asset_name = match select_asset(std::env::consts::OS, std::env::consts::ARCH) {
        Some(a) => a,
        None => {
            eprintln!(
                "{RED} no prebuilt asset for {}/{} — build from source instead",
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            return 5;
        }
    };

    // Find the asset's download URL (+ digest if the API provides one).
    let assets = release.get("assets").and_then(|a| a.as_array());
    let asset = assets.and_then(|arr| {
        arr.iter()
            .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(asset_name))
    });
    let Some(asset) = asset else {
        eprintln!("{RED} release has no asset named {asset_name}");
        return 1;
    };
    let url = match asset.get("browser_download_url").and_then(|u| u.as_str()) {
        Some(u) => u.to_string(),
        None => {
            eprintln!("{RED} asset {asset_name} has no download URL");
            return 1;
        }
    };
    let expected_digest = asset
        .get("digest")
        .and_then(|d| d.as_str())
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(|s| s.to_string());

    // Work in a temp dir.
    let tmp = std::env::temp_dir().join(format!("mae-upgrade-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    if let Err(e) = std::fs::create_dir_all(&tmp) {
        eprintln!("{RED} could not create temp dir: {e}");
        return 1;
    }
    let archive = tmp.join(asset_name);

    println!("Downloading {asset_name}…");
    let dl = Command::new("curl")
        .args(["-fL", "--max-time", "300", "-o"])
        .arg(&archive)
        .arg(&url)
        .status();
    if !matches!(dl, Ok(s) if s.success()) {
        eprintln!("{RED} download failed");
        let _ = std::fs::remove_dir_all(&tmp);
        return 1;
    }

    // Verify checksum when the API supplied one; otherwise note HTTPS-only trust.
    match (expected_digest, std::fs::read(&archive)) {
        (Some(expected), Ok(bytes)) => {
            let actual = sha256_hex(&bytes);
            if actual != expected {
                eprintln!("{RED} checksum mismatch — refusing to install");
                eprintln!("    expected {expected}\n    actual   {actual}");
                let _ = std::fs::remove_dir_all(&tmp);
                return 1;
            }
            println!("{GREEN} checksum verified");
        }
        (None, _) => println!("{YELLOW} no published checksum; relying on HTTPS transport"),
        (_, Err(e)) => {
            eprintln!("{RED} could not read downloaded archive: {e}");
            let _ = std::fs::remove_dir_all(&tmp);
            return 1;
        }
    }

    // Extract.
    let extract = tmp.join("extract");
    let _ = std::fs::create_dir_all(&extract);
    let untar = Command::new("tar")
        .arg("xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&extract)
        .status();
    if !matches!(untar, Ok(s) if s.success()) {
        eprintln!("{RED} extraction failed");
        let _ = std::fs::remove_dir_all(&tmp);
        return 1;
    }

    // Locate install.sh (tarball may contain a top-level dir).
    let installer = find_installer(&extract);
    let Some(installer) = installer else {
        eprintln!("{RED} install.sh not found in the release tarball");
        let _ = std::fs::remove_dir_all(&tmp);
        return 1;
    };

    // PREFIX = parent of the bin dir (e.g. ~/.local/bin -> ~/.local).
    let prefix = state
        .bin_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| state.bin_dir.clone());

    println!("Running installer (install.sh {})…", prefix.display());
    let mut sh = Command::new("bash");
    sh.arg(&installer).arg(&prefix);
    let code = run_inherit(sh, "install.sh");
    let _ = std::fs::remove_dir_all(&tmp);
    code
}

/// Find `install.sh` at the extract root or one level down.
fn find_installer(extract: &Path) -> Option<PathBuf> {
    let direct = extract.join("install.sh");
    if direct.exists() {
        return Some(direct);
    }
    let entries = std::fs::read_dir(extract).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join("install.sh");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Read a y/N answer from stdin (default No).
fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

// ===========================================================================
// Orchestration
// ===========================================================================

/// CLI entry point for `mae upgrade`. Returns the process exit code.
pub fn run_upgrade_cli(args: &[String]) -> i32 {
    match parse_upgrade_args(args) {
        ArgParse::Help => {
            print_help();
            0
        }
        ArgParse::Unknown(flag) => {
            eprintln!("Unknown flag for `mae upgrade`: {flag}");
            eprintln!("Run `mae upgrade --help` for usage.");
            2
        }
        ArgParse::Opts(opts) if opts.packages => {
            // Old `mae upgrade` behavior — upgrade declared packages only.
            crate::pkg::cli::cmd_upgrade()
        }
        ArgParse::Opts(opts) => run_self_upgrade(opts),
    }
}

fn print_help() {
    println!("mae upgrade — upgrade MAE itself (and packages)");
    println!();
    println!("USAGE:");
    println!("  mae upgrade [--check] [--yes] [--packages]");
    println!();
    println!("OPTIONS:");
    println!("  --check, -n      Report channel + available version; do not modify anything");
    println!("  --yes, -y        Skip the confirmation prompt");
    println!("  --packages, -p   Upgrade only declared packages (same as `mae pkg upgrade`)");
    println!();
    println!("NOTES:");
    println!("  `mae upgrade` now upgrades MAE itself via the detected install channel");
    println!("  (Homebrew / release tarball / source checkout). To upgrade only your");
    println!("  declared packages, use `mae pkg upgrade` or `mae sync`.");
}

fn run_self_upgrade(opts: UpgradeOpts) -> i32 {
    let current = match Version::parse(env!("CARGO_PKG_VERSION")) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{RED} internal: unparseable current version: {e}");
            return 1;
        }
    };

    let (channel, state) = detect_channel();
    println!("{BOLD}MAE self-upgrade{RESET}");
    println!("  channel: {}", channel.label());
    println!("  binary:  {}", state.exe_path.display());
    println!("  current: {current}");

    // Cargo / Unknown: never mutate, just guide.
    if matches!(channel, Channel::Cargo | Channel::Unknown) {
        guidance_for_unmanaged(&channel);
        return if opts.check { 0 } else { 5 };
    }

    // Version check.
    let (latest, release) = match fetch_latest_release() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{YELLOW} could not check for updates: {e}");
            return 1;
        }
    };
    println!("  latest:  {latest}");

    if latest <= current {
        println!("\n{GREEN} MAE is up to date.");
        return 0;
    }

    // Compatibility gate.
    let notes = fetch_notes_in_range(&current, &latest);
    let breaking = scan_notes_for_breaking(&notes);
    let gate = compat_decision(&current, &latest, breaking);

    // Preflight.
    println!("\n{BOLD}Preflight{RESET}");
    let checks = preflight(&state, &channel);
    let mut blocking = false;
    for c in &checks {
        let mark = if c.ok {
            GREEN
        } else if c.error {
            blocking = true;
            RED
        } else {
            YELLOW
        };
        println!("  {mark} {}", c.msg);
        if !c.ok {
            if let Some(fix) = &c.fix {
                println!("      → {fix}");
            }
        }
    }

    // Compatibility refusal takes precedence — clearest guidance for the user.
    if gate != CompatGate::AutoOk {
        println!();
        let reason = match gate {
            CompatGate::RefuseMajor => "this is a major-version upgrade",
            CompatGate::RefuseBreaking => "the changelog lists breaking changes",
            CompatGate::AutoOk => unreachable!(),
        };
        println!("{RED} Refusing automatic upgrade {current} → {latest}: {reason}.");
        println!("    Review the changelog and upgrade manually:");
        println!("    {}", manual_command(&channel));
        println!("    https://github.com/{REPO}/releases");
        return 3;
    }

    if blocking {
        println!("\n{RED} Preflight failed — resolve the errors above and retry.");
        return 4;
    }

    println!(
        "\nWill upgrade {current} → {latest} via {} ({})",
        channel.label(),
        manual_command(&channel)
    );

    if opts.check {
        println!("{GREEN} Update available (run `mae upgrade` to apply).");
        return 0;
    }

    if !opts.yes && !confirm("Proceed?") {
        println!("Aborted.");
        return 0;
    }

    // Execute.
    println!();
    match channel {
        Channel::Homebrew { cask } => run_brew_upgrade(cask),
        Channel::SourceCheckout => match &state.repo_root {
            Some(root) => run_source_upgrade(root),
            None => {
                eprintln!("{RED} source checkout root not found");
                1
            }
        },
        Channel::Tarball => run_tarball_upgrade(&state, &release),
        Channel::Cargo | Channel::Unknown => unreachable!("handled above"),
    }
}

/// The manual upgrade command for a channel (shown on refusal / as the plan).
fn manual_command(channel: &Channel) -> String {
    match channel {
        Channel::Homebrew { cask: false } => "brew update && brew upgrade mae".to_string(),
        Channel::Homebrew { cask: true } => "brew update && brew upgrade --cask mae".to_string(),
        Channel::Tarball => {
            "download the latest release tarball and run its install.sh".to_string()
        }
        Channel::SourceCheckout => "git pull --ff-only && make install-upgrade".to_string(),
        Channel::Cargo => {
            "cargo install --git https://github.com/cuttlefisch/mae mae --force".to_string()
        }
        Channel::Unknown => {
            "reinstall from https://github.com/cuttlefisch/mae/releases".to_string()
        }
    }
}

fn guidance_for_unmanaged(channel: &Channel) {
    println!();
    match channel {
        Channel::Cargo => {
            println!(
                "{YELLOW} Installed via cargo — self-upgrade isn't automated for this channel."
            );
        }
        Channel::Unknown => {
            println!("{YELLOW} Could not determine the install method (AppImage or custom).");
        }
        _ => {}
    }
    println!("    Upgrade manually: {}", manual_command(channel));
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn classify_homebrew_formula_and_cask() {
        assert_eq!(
            classify_channel_from(
                Path::new("/opt/homebrew/Cellar/mae/0.13.9/bin/mae"),
                None,
                false
            ),
            Channel::Homebrew { cask: false }
        );
        assert_eq!(
            classify_channel_from(
                Path::new("/opt/homebrew/Caskroom/mae/0.13.9/MAE.app/Contents/MacOS/mae"),
                None,
                false
            ),
            Channel::Homebrew { cask: true }
        );
        // Matches by configured brew prefix too.
        assert_eq!(
            classify_channel_from(
                Path::new("/home/linuxbrew/.linuxbrew/bin/mae"),
                Some(Path::new("/home/linuxbrew/.linuxbrew")),
                false
            ),
            Channel::Homebrew { cask: false }
        );
    }

    #[test]
    fn classify_cargo_source_tarball() {
        assert_eq!(
            classify_channel_from(Path::new("/home/u/.cargo/bin/mae"), None, false),
            Channel::Cargo
        );
        // Source tree flag wins over a plain path.
        assert_eq!(
            classify_channel_from(Path::new("/src/mae/target/release/mae"), None, true),
            Channel::SourceCheckout
        );
        // Default: a normal installed binary.
        assert_eq!(
            classify_channel_from(Path::new("/home/u/.local/bin/mae"), None, false),
            Channel::Tarball
        );
    }

    #[test]
    fn parse_latest_tag_strips_v() {
        assert_eq!(
            parse_latest_tag(r#"{"tag_name":"v0.14.0"}"#).unwrap(),
            v("0.14.0")
        );
        assert_eq!(
            parse_latest_tag(r#"{"tag_name":"0.13.9"}"#).unwrap(),
            v("0.13.9")
        );
        assert!(parse_latest_tag(r#"{"name":"no tag"}"#).is_err());
        assert!(parse_latest_tag("not json").is_err());
    }

    #[test]
    fn compat_decision_matrix() {
        // patch within minor
        assert_eq!(
            compat_decision(&v("0.13.8"), &v("0.13.9"), false),
            CompatGate::AutoOk
        );
        // cross-minor, no breaking
        assert_eq!(
            compat_decision(&v("0.13.9"), &v("0.14.0"), false),
            CompatGate::AutoOk
        );
        // cross-minor with breaking marker
        assert_eq!(
            compat_decision(&v("0.13.9"), &v("0.14.0"), true),
            CompatGate::RefuseBreaking
        );
        // major jump always refused (even without a marker)
        assert_eq!(
            compat_decision(&v("0.13.9"), &v("1.0.0"), false),
            CompatGate::RefuseMajor
        );
    }

    #[test]
    fn scan_notes_detects_breaking_markers() {
        assert!(scan_notes_for_breaking(
            "### ⚠ BREAKING CHANGES\n- removed X"
        ));
        assert!(scan_notes_for_breaking(
            "- feat!: drop legacy config format"
        ));
        assert!(scan_notes_for_breaking("## Breaking\nstuff"));
        assert!(!scan_notes_for_breaking(
            "### Features\n- added Y\n### Bug Fixes\n- fixed Z"
        ));
        assert!(!scan_notes_for_breaking(""));
    }

    #[test]
    fn select_asset_table() {
        assert_eq!(
            select_asset("macos", "aarch64"),
            Some("mae-macos-aarch64.tar.gz")
        );
        assert_eq!(
            select_asset("linux", "x86_64"),
            Some("mae-linux-x86_64.tar.gz")
        );
        assert_eq!(select_asset("macos", "x86_64"), None);
        assert_eq!(select_asset("linux", "aarch64"), None);
        assert_eq!(select_asset("windows", "x86_64"), None);
    }

    #[test]
    fn parse_args_flags() {
        assert_eq!(
            parse_upgrade_args(&["--check".into()]),
            ArgParse::Opts(UpgradeOpts {
                check: true,
                ..Default::default()
            })
        );
        assert_eq!(
            parse_upgrade_args(&["-y".into(), "--packages".into()]),
            ArgParse::Opts(UpgradeOpts {
                yes: true,
                packages: true,
                ..Default::default()
            })
        );
        assert_eq!(parse_upgrade_args(&["--help".into()]), ArgParse::Help);
        assert_eq!(
            parse_upgrade_args(&["--bogus".into()]),
            ArgParse::Unknown("--bogus".into())
        );
    }

    #[test]
    fn sha256_known_vector() {
        // SHA-256("abc")
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn unknown_flag_exits_two() {
        assert_eq!(run_upgrade_cli(&["--nope".into()]), 2);
    }

    #[test]
    fn help_exits_zero() {
        assert_eq!(run_upgrade_cli(&["--help".into()]), 0);
    }
}
