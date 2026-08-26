//! Story B / R11 — a project KB is keyed on a durable identity, not a path.
//!
//! **The bug being prevented has a name**: VS Code's `workspaceStorage` keys
//! per-project state on an MD5 of the raw, un-canonicalized path salted with the
//! inode, under a source comment reading `DO NOT CHANGE. IDENTIFIERS HAVE TO
//! REMAIN STABLE`. Rename or move a project and its state is permanently
//! orphaned — Microsoft classified that as a backlog feature request.
//!
//! So the load-bearing test here is not "provisioning works". It is **"the
//! project moves and its KB comes with it"**.

use super::*;
use std::path::Path;

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn git_project(parent: &Path, name: &str) -> std::path::PathBuf {
    let root = parent.join(name);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    root
}

/// **The property `project_key` alone can deliver**, and the one Story B is
/// actually named for: *KB content OUT of tree*.
///
/// Falsifying the in-tree version of this test showed it proved nothing about
/// the identity: `.mae-kb/eor-instance.org` sits inside the project, so the
/// sentinel travels with a move and the uuid survives without any durable key.
/// The moment the KB's org dir lives OUTSIDE the project — which is the whole
/// point of a project-scoped KB whose content is not committed — the sentinel
/// stays put, path equality fails, and only the minted key can still say "this
/// is the same project".
#[test]
fn an_out_of_tree_project_kb_survives_the_project_being_moved() {
    let home = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    let project = git_project(home.path(), "widget");
    let out_of_tree = home.path().join("kb-content");
    std::fs::create_dir_all(&out_of_tree).unwrap();

    let registered = editor.kb_register("Widget", &out_of_tree).unwrap();
    let key = mae_kb::project_identity::resolve(&project).unwrap().key();
    let canonical_project = project.canonicalize().unwrap();
    {
        let inst = editor
            .kb
            .registry
            .instances
            .iter_mut()
            .find(|i| i.uuid == registered.uuid)
            .unwrap();
        inst.kind = mae_kb::federation::KbInstanceKind::Project;
        inst.project_root = Some(canonical_project.clone());
        inst.project_key = Some(key.clone());
    }

    // The project moves. The out-of-tree KB content does NOT.
    let moved = home.path().join("renamed-widget");
    std::fs::rename(&project, &moved).unwrap();
    let canonical_moved = moved.canonicalize().unwrap();
    let moved_key = mae_kb::project_identity::resolve(&moved).unwrap().key();
    assert_eq!(moved_key, key, "the minted id must survive the move itself");

    let (uuid, repaired) = editor
        .kb
        .registry
        .adopt_moved_project(&canonical_moved, Some(&moved_key))
        .expect("the moved project must still be recognised by its durable key");

    assert_eq!(uuid, registered.uuid);
    assert!(repaired, "and its stale project_root must be corrected");
    assert_eq!(
        editor
            .kb
            .registry
            .find(&uuid)
            .unwrap()
            .project_root
            .as_deref(),
        Some(canonical_moved.as_path())
    );

    // The negative case that must fail: strip the key and the same lookup misses,
    // which is precisely the VS Code orphaning bug.
    editor
        .kb
        .registry
        .instances
        .iter_mut()
        .find(|i| i.uuid == uuid)
        .unwrap()
        .project_key = None;
    editor
        .kb
        .registry
        .instances
        .iter_mut()
        .find(|i| i.uuid == uuid)
        .unwrap()
        .project_root = Some(canonical_project);
    assert!(
        editor
            .kb
            .registry
            .adopt_moved_project(&canonical_moved, Some(&moved_key))
            .is_none(),
        "without a durable key there is nothing left but the stale path"
    );
}

/// The in-tree case, which survives for a DIFFERENT reason worth pinning
/// separately: `.mae-kb/eor-instance.org` moves with the project, so
/// `KbRegistry::register` reads the same uuid back out of it.
#[test]
fn a_project_kb_survives_the_project_being_moved() {
    let home = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    let original = git_project(home.path(), "widget");
    let first = editor.kb_init_project(Some(original.clone())).unwrap();

    let moved = home.path().join("renamed-widget");
    std::fs::rename(&original, &moved).unwrap();

    let second = editor.kb_init_project(Some(moved.clone())).unwrap();

    assert_eq!(
        second.uuid, first.uuid,
        "the moved project must adopt its EXISTING KB, not be handed a new one"
    );
    // **The oracle that is not vacuous.** uuid equality alone proves nothing
    // here: `KbRegistry::register` reads a uuid back out of the `eor-instance.org`
    // sentinel, and that sentinel moves WITH the project — so a path-keyed
    // registry produces two rows carrying the SAME uuid, and the naive assertion
    // above passes while the registry is quietly corrupt. Falsifying this test
    // is what surfaced that; count the ROWS.
    assert_eq!(
        editor.kb.registry.instances.len(),
        1,
        "a moved project must not append a second row: {:?}",
        editor
            .kb
            .registry
            .instances
            .iter()
            .map(|i| (i.name.clone(), i.uuid.clone(), i.project_root.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        editor.kb.registry.instances[0].effective_kind(),
        mae_kb::federation::KbInstanceKind::Project,
        "and the surviving row is still the project KB"
    );
}

/// The stale path is **repaired**, not merely tolerated — otherwise every later
/// path-keyed lookup (`KbScope::Project`, the graph view) still misses.
#[test]
fn adopting_a_moved_project_repairs_the_stale_root_on_disk() {
    let home = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    let original = git_project(home.path(), "widget");
    let result = editor.kb_init_project(Some(original.clone())).unwrap();

    let moved = home.path().join("elsewhere");
    std::fs::rename(&original, &moved).unwrap();
    editor.kb_init_project(Some(moved.clone())).unwrap();

    let canonical_moved = moved.canonicalize().unwrap();
    let inst = editor.kb.registry.find(&result.uuid).unwrap();
    assert_eq!(
        inst.project_root.as_deref(),
        Some(canonical_moved.as_path()),
        "the path is a repairable cache of where the project was last seen"
    );

    // And the repair is durable, not just in-memory.
    let data_dir = editor.mae_data_dir().unwrap();
    let reloaded = mae_kb::federation::KbRegistry::load(&data_dir);
    assert_eq!(
        reloaded
            .find(&result.uuid)
            .and_then(|i| i.project_root.clone()),
        Some(canonical_moved),
        "a repair that does not survive a restart is not a repair"
    );
}

/// **Two different projects must never collide**, which is the sibling risk of
/// path keying and the reason VS Code's identically-named-folder case was a
/// *security* bug rather than a correctness one.
#[test]
fn two_projects_get_distinct_identities_and_distinct_kbs() {
    let home = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    let a = git_project(&home.path().join("one"), "widget");
    let b = git_project(&home.path().join("two"), "widget");

    let ra = editor.kb_init_project(Some(a)).unwrap();
    let rb = editor.kb_init_project(Some(b)).unwrap();

    assert_ne!(
        ra.uuid, rb.uuid,
        "two same-named projects at different paths are two projects"
    );
    let keys: Vec<_> = editor
        .kb
        .registry
        .instances
        .iter()
        .filter_map(|i| i.project_key.clone())
        .collect();
    assert_eq!(keys.len(), 2);
    assert_ne!(keys[0], keys[1], "and their minted keys must differ");
}

/// A registry row written before `project_key` existed must behave exactly as it
/// did — path equality — or shipping this field breaks every existing project KB.
#[test]
fn a_keyless_legacy_instance_still_matches_by_path() {
    let home = TempDir::new().unwrap();
    let root = home.path().join("legacy");
    std::fs::create_dir_all(&root).unwrap();
    let canonical = root.canonicalize().unwrap();

    let mut inst = mae_kb::federation::KbInstance::joined(
        "u-legacy".into(),
        "Legacy",
        std::path::PathBuf::from("/tmp/legacy.db"),
        "now".into(),
    );
    inst.kind = mae_kb::federation::KbInstanceKind::Project;
    inst.project_root = Some(canonical.clone());
    inst.project_key = None;

    assert!(
        inst.matches_project(&canonical, Some("kbid:something-new")),
        "with no key stored, the instance must fall back to path equality — a \
         registry written before this field existed cannot start missing"
    );
    assert!(!inst.matches_project(Path::new("/tmp/other"), None));
}

/// Not a git repo: the identity is honest about being weaker rather than
/// pretending to be stable.
#[test]
fn a_non_git_project_gets_a_path_fallback_that_says_so() {
    let home = TempDir::new().unwrap();
    let root = home.path().join("plain");
    std::fs::create_dir_all(&root).unwrap();

    let id = mae_kb::project_identity::resolve(&root).unwrap();
    assert!(
        !id.is_stable(),
        "a path fallback must not claim to survive a move: {id:?}"
    );
}

/// **The defect the falsification pass exposed, pinned directly.**
///
/// `KbRegistry::register` reads a uuid back out of the org dir's
/// `eor-instance.org` sentinel — and that sentinel travels with the directory.
/// So registering a moved (or copied) org dir used to append a SECOND row
/// carrying an already-present uuid. Every uuid lookup in the tree is a
/// `find(|i| i.uuid == …)` — first match wins — so `find()` and
/// `KbRegistry::update` would then disagree about which row they meant.
#[test]
fn a_moved_org_dir_repoints_its_row_instead_of_appending_a_duplicate_uuid() {
    let home = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    let first_dir = home.path().join("notes");
    std::fs::create_dir_all(&first_dir).unwrap();
    let first = editor.kb_register("Notes", &first_dir).unwrap();

    let moved_dir = home.path().join("notes-moved");
    std::fs::rename(&first_dir, &moved_dir).unwrap();
    let second = editor.kb_register("NotesMoved", &moved_dir).unwrap();

    assert_eq!(second.uuid, first.uuid, "the sentinel carries the uuid");

    let uuids: Vec<&str> = editor
        .kb
        .registry
        .instances
        .iter()
        .map(|i| i.uuid.as_str())
        .collect();
    let mut deduped = uuids.clone();
    deduped.sort_unstable();
    deduped.dedup();
    assert_eq!(
        uuids.len(),
        deduped.len(),
        "no two registry rows may share a uuid: {uuids:?}"
    );
    assert_eq!(
        editor
            .kb
            .registry
            .find(&first.uuid)
            .and_then(|i| i.org_dir.canonicalize().ok()),
        moved_dir.canonicalize().ok(),
        "and the surviving row must point at where the directory actually is"
    );
}
