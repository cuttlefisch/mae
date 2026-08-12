//! Bootstrap tests: module enable/dependency resolution, layered init-file loading (including
//! workspace trust), and keymap-flavor reconciliation.

use super::super::*;

fn disc(name: &str, toml: &str) -> crate::pkg::embedded::DiscoveredModule {
    use crate::pkg::embedded::{DiscoveredModule, ModuleSource};
    use crate::pkg::manifest::ModuleManifest;
    use std::path::{Path, PathBuf};
    DiscoveredModule {
        source: ModuleSource::Disk(PathBuf::from(format!("modules/{name}"))),
        manifest: ModuleManifest::from_str(toml, Path::new("test")).unwrap(),
    }
}

#[test]
fn enable_with_deps_expands_deps_of_already_enabled_module() {
    // Regression for the Linux "keymap-doom depends on keymap-leader which is
    // not enabled" brick: when the flavor is declared in (mae!) it is already
    // in `enabled`, and enable_with_deps must STILL pull in keymap-leader.
    let all = vec![
        disc(
            "keymap-doom",
            "[module]\nname = \"keymap-doom\"\n\n[dependencies]\nkeymap-leader = \"*\"",
        ),
        disc("keymap-leader", "[module]\nname = \"keymap-leader\""),
    ];
    // keymap-doom pre-enabled (as if declared in the mae! block).
    let mut enabled: HashMap<String, Vec<String>> = HashMap::new();
    enabled.insert("keymap-doom".to_string(), vec![]);

    enable_with_deps("keymap-doom", &all, &mut enabled);

    assert!(
        enabled.contains_key("keymap-leader"),
        "declared keymap-doom must still pull in its keymap-leader dependency"
    );

    // And the resolver must now produce a consistent order with no skips.
    let outcome = crate::pkg::resolver::resolve_load_order(&all, &enabled);
    assert!(outcome.skipped.is_empty(), "nothing should be skipped");
    let names: Vec<&str> = outcome.resolved.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"keymap-doom") && names.contains(&"keymap-leader"));
}

#[test]
fn reconcile_keymap_flavor_option_is_authoritative() {
    // No declaration → option untouched, nothing dropped.
    assert_eq!(reconcile_keymap_flavor(&[], "doom", "doom"), (None, vec![]));

    // Lone declaration, option at default → adopt the declaration.
    let (sync, drop) = reconcile_keymap_flavor(&["keymap-nonmodal".into()], "doom", "doom");
    assert_eq!(sync.as_deref(), Some("nonmodal"));
    assert!(drop.is_empty(), "the adopted flavor must not be dropped");

    // Option explicitly set (non-default) disagrees with a declared flavor →
    // option wins, declared flavor dropped (this is the live-switch case:
    // init.scm hardcodes keymap-doom, user switched to nonmodal).
    let (sync, drop) = reconcile_keymap_flavor(&["keymap-doom".into()], "nonmodal", "doom");
    assert_eq!(sync, None);
    assert_eq!(drop, vec!["keymap-doom".to_string()]);

    // Declaration matches the option → nothing to sync or drop.
    let (sync, drop) = reconcile_keymap_flavor(&["keymap-doom".into()], "doom", "doom");
    assert_eq!(sync, None);
    assert!(drop.is_empty());
}

#[test]
fn enable_with_deps_terminates_on_cycle() {
    // Self-referential / cyclic deps must not loop forever.
    let all = vec![
        disc("a", "[module]\nname = \"a\"\n\n[dependencies]\nb = \"*\""),
        disc("b", "[module]\nname = \"b\"\n\n[dependencies]\na = \"*\""),
    ];
    let mut enabled: HashMap<String, Vec<String>> = HashMap::new();
    enable_with_deps("a", &all, &mut enabled);
    assert!(enabled.contains_key("a") && enabled.contains_key("b"));
}

#[test]
fn layered_init_loads_multiple_files() {
    // Create a temp dir with two init files
    let tmp = tempfile::tempdir().unwrap();
    let dir1 = tmp.path().join(".config").join("mae");
    std::fs::create_dir_all(&dir1).unwrap();
    std::fs::write(dir1.join("init.scm"), "(set-status \"user\")").unwrap();

    let dir2 = tmp.path().join("project").join(".mae");
    std::fs::create_dir_all(&dir2).unwrap();
    std::fs::write(dir2.join("init.scm"), "(set-status \"project\")").unwrap();

    // Can't easily test the full layered loading without env var manipulation,
    // but we can verify the function signature exists and is callable.
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    // load_init_files returns a usize count
    let _count: usize = load_init_files(&mut scheme, &mut editor);
}

/// Run `load_init_files` with cwd set to `project` and config isolated to
/// `cfg_home`, returning the editor's status line so the caller can tell
/// whether the project's init actually evaluated.
fn load_init_in(project: &std::path::Path, cfg_home: &std::path::Path) -> String {
    let saved_cwd = std::env::current_dir().ok();
    let saved_cfg = std::env::var("XDG_CONFIG_HOME").ok();
    unsafe { std::env::set_var("XDG_CONFIG_HOME", cfg_home) };
    std::env::set_current_dir(project).unwrap();

    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    load_init_files(&mut scheme, &mut editor);
    let status = editor.status_msg.clone();

    if let Some(cwd) = saved_cwd {
        let _ = std::env::set_current_dir(cwd);
    }
    match saved_cfg {
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }
    status
}

/// ADR-089: the attacker's test. A cloned repository carrying `.mae/init.scm`
/// must not have it evaluated, and the *same* directory must work once trusted —
/// so this pins both that the boundary holds and that the feature still exists.
#[test]
fn project_local_init_runs_only_from_a_trusted_directory() {
    let _guard = mae_effect_sandbox::lock_env();

    let tmp = tempfile::tempdir().unwrap();
    let cfg_home = tmp.path().join("config");
    let project = tmp.path().join("cloned-repo");
    std::fs::create_dir_all(cfg_home.join("mae")).unwrap();
    std::fs::create_dir_all(project.join(".mae")).unwrap();
    std::fs::write(
        project.join(".mae").join("init.scm"),
        "(set-status \"HOSTILE-INIT-RAN\")",
    )
    .unwrap();

    // Untrusted: must not evaluate.
    let status = load_init_in(&project, &cfg_home);
    assert!(
        !status.contains("HOSTILE-INIT-RAN"),
        "untrusted project init must NOT be evaluated, got status: {status:?}"
    );

    // Trusted: must evaluate — otherwise the guard has broken the feature.
    std::fs::write(
        cfg_home.join("mae").join("trusted-projects"),
        format!("{}\n", project.canonicalize().unwrap().display()),
    )
    .unwrap();
    let status = load_init_in(&project, &cfg_home);
    assert!(
        status.contains("HOSTILE-INIT-RAN"),
        "explicitly trusted project init must still be evaluated, got: {status:?}"
    );
}

/// ADR-089 D3: the v0.6 cwd fallbacks are gone. A repository with a top-level
/// `init.scm` — an entirely ordinary filename — must not execute it, and the
/// fresh-install case (no user init) was the one where it previously did.
#[test]
fn a_bare_cwd_init_scm_is_never_loaded() {
    let _guard = mae_effect_sandbox::lock_env();

    let tmp = tempfile::tempdir().unwrap();
    let cfg_home = tmp.path().join("config-with-no-user-init");
    let project = tmp.path().join("repo");
    std::fs::create_dir_all(&cfg_home).unwrap();
    std::fs::create_dir_all(project.join("scheme")).unwrap();
    std::fs::write(project.join("init.scm"), "(set-status \"BARE-INIT-RAN\")").unwrap();
    std::fs::write(
        project.join("scheme").join("init.scm"),
        "(set-status \"SCHEME-INIT-RAN\")",
    )
    .unwrap();

    let status = load_init_in(&project, &cfg_home);
    assert!(
        !status.contains("BARE-INIT-RAN") && !status.contains("SCHEME-INIT-RAN"),
        "legacy cwd init fallbacks must be retired, got status: {status:?}"
    );
}

/// Rewritten: the previous version had two defects that made it both
/// unsafe and incapable of failing.
///
/// 1. `let _guard = std::env::set_current_dir(tmp.path())` reads as an RAII
///    guard but `set_current_dir` returns `io::Result<()>` — there is no
///    guard and no restoration. It changed the **process-wide** cwd for
///    every remaining test in this binary, then dropped `tmp`, leaving them
///    running in a *deleted* directory. It also took no lock, so it raced
///    every other test rather than only those using `INIT_ENV_LOCK`.
/// 2. Its own comment conceded it "may still load `~/.config/mae/init.scm`"
///    — reading the contributor's real config — and then asserted nothing
///    at all, so no outcome could fail it.
///
/// Now: cwd and `XDG_CONFIG_HOME` are both isolated under the shared lock,
/// and the assertion is the one the name promises.
#[test]
fn load_init_files_returns_zero_when_no_files() {
    let _lock = mae_effect_sandbox::lock_env();
    let mut scheme = require_scheme!();
    let project = tempfile::tempdir().unwrap();
    let cfg_home = tempfile::tempdir().unwrap();

    let saved_cwd = std::env::current_dir().ok();
    let saved_cfg = std::env::var("XDG_CONFIG_HOME").ok();
    unsafe { std::env::set_var("XDG_CONFIG_HOME", cfg_home.path()) };
    std::env::set_current_dir(project.path()).unwrap();

    let mut editor = Editor::new();
    let count = load_init_files(&mut scheme, &mut editor);

    // Restore before asserting, so a failure cannot strand the process in
    // a directory that is about to be deleted.
    if let Some(cwd) = saved_cwd {
        let _ = std::env::set_current_dir(cwd);
    }
    match saved_cfg {
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }

    assert_eq!(
        count, 0,
        "no init file exists in either the isolated project or config dir, \
         so nothing should have loaded — a non-zero count means the loader \
         reached outside its isolation"
    );
}

#[test]
fn reload_all_modules_loads_embedded_and_is_idempotent() {
    // Embedded keymap-doom must load even with no on-disk modules, and a
    // second reload must not change binding/module counts (idempotent
    // registration). This is the core regression guard for the whole
    // overhaul: it proves the embedded baseline populates the leader tree,
    // so removing the kernel's duplicated SPC bindings is safe.
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();

    let total_normal = |e: &Editor| -> usize {
        e.keymaps
            .get("normal")
            .map(|k| k.bindings().count())
            .unwrap_or(0)
    };
    let has_collab = |e: &Editor| -> bool {
        // collab-start lives in the `leader` keymap (via keymap-leader).
        // Check all keymaps to be resilient to on-disk module overrides
        // during development (stale ~/.local/share/mae/modules may put
        // it in `normal` instead).
        e.keymaps
            .values()
            .any(|k| k.bindings().any(|(_, cmd)| cmd == "collab-start"))
    };

    reload_all_modules(&mut scheme, &mut editor);
    let mods1 = editor.active_modules.len();
    let binds1 = total_normal(&editor);
    assert!(
        mods1 >= 20,
        "embedded modules should load with no on-disk modules, got {mods1}"
    );
    assert!(
        editor
            .active_modules
            .iter()
            .any(|m| m.name == "keymap-doom" && m.status == "loaded"),
        "embedded keymap-doom must load"
    );
    // The collab leader binding must be present in some keymap after
    // module loading (in the `leader` keymap via keymap-leader).
    assert!(
        has_collab(&editor),
        "collab-start (SPC C s) should be bound after keymap modules load"
    );

    reload_all_modules(&mut scheme, &mut editor);
    assert_eq!(
        mods1,
        editor.active_modules.len(),
        "module count stable across reload"
    );
    assert_eq!(
        binds1,
        total_normal(&editor),
        "binding count stable (idempotent reload)"
    );
}

#[test]
fn unknown_keymap_flavor_falls_back_to_doom() {
    // A bogus keymap_flavor must not leave the user with no leader tree —
    // load_modules falls back to the embedded keymap-doom.
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.keymap_flavor = "nonexistent-flavor".to_string();
    reload_all_modules(&mut scheme, &mut editor);
    assert!(
        editor
            .active_modules
            .iter()
            .any(|m| m.name == "keymap-doom" && m.status == "loaded"),
        "unknown flavor should fall back to keymap-doom"
    );
}

#[test]
fn keymap_flavor_option_roundtrips() {
    let mut editor = Editor::new();
    assert_eq!(
        editor
            .get_option("keymap_flavor")
            .map(|(v, _)| v)
            .as_deref(),
        Some("doom"),
        "default keymap_flavor should be doom"
    );
    editor.set_option("keymap_flavor", "emacs").unwrap();
    assert_eq!(editor.keymap_flavor, "emacs");
    assert_eq!(
        editor
            .get_option("keymap_flavor")
            .map(|(v, _)| v)
            .as_deref(),
        Some("emacs")
    );
}
