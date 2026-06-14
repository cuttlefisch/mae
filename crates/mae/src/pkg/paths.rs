//! Filesystem path resolution for MAE config, data, and module discovery.
//!
//! A single home for the path families MAE resolves, so call sites don't
//! re-derive XDG vs platform conventions ad hoc (previously these lived
//! scattered in `bootstrap.rs`):
//!
//!   - [`dirs_candidate`]      — user CONFIG path (`$XDG_CONFIG_HOME` or `~/.config`)
//!   - [`data_dir_candidate`]  — user DATA path (`$XDG_DATA_HOME` or `~/.local/share`)
//!   - [`builtin_module_dirs`] — ordered built-in module search path (the single
//!     source of truth shared by the editor loader and the `mae` package CLI)
//!
//! `bootstrap` re-exports all three, so existing `crate::bootstrap::*` call
//! sites are unaffected.

use std::path::PathBuf;

/// User config path: `$XDG_CONFIG_HOME/<rel>`, else `~/.config/<rel>`.
pub fn dirs_candidate(rel: &str) -> Option<PathBuf> {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .map(|base| base.join(rel))
}

/// User data path: `$XDG_DATA_HOME/<rel>`, else `~/.local/share/<rel>`.
pub fn data_dir_candidate(rel: &str) -> Option<PathBuf> {
    std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local/share"))
        })
        .map(|base| base.join(rel))
}

/// Ordered list of directories searched for built-in modules.
///
/// This is the single source of truth for module discovery, shared by the
/// editor's `load_modules` (first existing dir that yields modules wins) and
/// the `mae` package CLI (`list`/`doctor`/…) so the CLI reports exactly what
/// the editor would load — no divergent second copy. Order is explicit/most-
/// specific first:
///
/// 0. `$MAE_MODULES_PATH` — explicit override (AppImage, custom installs).
/// 1. `./modules` — dev: `cargo run` from repo root.
/// 2. `<exe>/modules` and `<exe>/../share/mae/modules` — tarball + FHS/Homebrew
///    (a `bin/mae` install keeps modules at `share/mae/modules`).
/// 3. `$XDG_DATA_HOME/mae/modules` (or `~/.local/share/...`) **and** the
///    platform-native data dir (`~/Library/Application Support/mae/modules` on
///    macOS) — `make install` / user installs, regardless of convention.
/// 4. compile-time `CARGO_MANIFEST_DIR` repo `modules` — dev builds only.
///
/// Note: with built-in modules embedded in the binary, this list governs
/// *on-disk overrides* of the embedded baseline (see `pkg::embedded`); an
/// empty result no longer means "no modules", only "no on-disk modules".
pub fn builtin_module_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // 0. explicit override
    if let Ok(path) = std::env::var("MAE_MODULES_PATH") {
        dirs.push(PathBuf::from(path));
    }
    // 1. CWD/modules (dev)
    dirs.push(PathBuf::from("modules"));
    // 2. next to the executable + FHS/Homebrew share layout
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            dirs.push(exe_dir.join("modules"));
            if let Some(prefix) = exe_dir.parent() {
                dirs.push(prefix.join("share/mae/modules"));
            }
        }
    }
    // 3. data dir(s): XDG (honoring XDG_DATA_HOME) AND the platform-native data
    //    dir (macOS ~/Library/Application Support), so an install works whether
    //    the installer followed XDG or the macOS convention.
    if let Some(data) = data_dir_candidate("mae/modules") {
        dirs.push(data);
    }
    if let Some(platform) = dirs::data_dir().map(|d| d.join("mae/modules")) {
        if !dirs.contains(&platform) {
            dirs.push(platform);
        }
    }
    // 4. compile-time repo modules (dev builds only)
    if let Some(repo_modules) = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|repo| repo.join("modules"))
    {
        dirs.push(repo_modules);
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_module_dirs_honors_env_override_first() {
        // MAE_MODULES_PATH must take precedence so AppImage/custom installs win.
        let prev = std::env::var("MAE_MODULES_PATH").ok();
        std::env::set_var("MAE_MODULES_PATH", "/custom/mae/modules");
        let dirs = builtin_module_dirs();
        assert_eq!(
            dirs.first(),
            Some(&PathBuf::from("/custom/mae/modules")),
            "MAE_MODULES_PATH should be searched first"
        );
        // Always includes the dev `./modules` and at least one data-dir candidate.
        assert!(dirs.contains(&PathBuf::from("modules")));
        assert!(
            dirs.len() >= 3,
            "expected env + cwd + install paths, got {dirs:?}"
        );
        match prev {
            Some(v) => std::env::set_var("MAE_MODULES_PATH", v),
            None => std::env::remove_var("MAE_MODULES_PATH"),
        }
    }

    #[test]
    fn builtin_module_dirs_has_no_duplicates() {
        // The XDG and platform-native data dirs collapse to one entry on Linux;
        // the helper must not emit the same path twice (wasted stat + confusing
        // diagnostics in the "searched: {:?}" warning).
        let dirs = builtin_module_dirs();
        let mut seen = std::collections::HashSet::new();
        for d in &dirs {
            assert!(seen.insert(d.clone()), "duplicate search dir: {d:?}");
        }
    }
}
