//! Tests for [`super`] — the KB migration module.
//!
//! Split out of `migrate.rs` so the production half stays a readable ~490
//! lines. The file sat at 798 against an 800-line ceiling on `main`, i.e. one
//! comment away from tripping it regardless; ~40% of it was tests. Follows the
//! sibling `cozo_store/tests/` convention, and stays a CHILD module so
//! `use super::*` still reaches `migrate`'s private helpers (`slugify`,
//! `node_to_orgroam`, `select_nodes`, …) unchanged.

use super::*;

#[test]
fn select_by_prefix() {
    let mut kb = KnowledgeBase::new();
    kb.insert(Node::new("roadmap:a", "A", NodeKind::Note, "body a"));
    kb.insert(Node::new("roadmap:b", "B", NodeKind::Note, "body b"));
    kb.insert(Node::new("concept:c", "C", NodeKind::Concept, "body c"));

    let opts = MigrateOptions {
        id_prefix: Some("roadmap:".to_string()),
        ..Default::default()
    };
    let nodes = select_nodes(&kb, &opts);
    assert_eq!(nodes.len(), 2);
}

#[test]
fn select_by_tags() {
    let mut kb = KnowledgeBase::new();
    kb.insert(Node::new("n1", "N1", NodeKind::Note, "").with_tags(["mae", "roadmap"]));
    kb.insert(Node::new("n2", "N2", NodeKind::Note, "").with_tags(["personal"]));

    let opts = MigrateOptions {
        tags: vec!["mae".to_string()],
        ..Default::default()
    };
    let nodes = select_nodes(&kb, &opts);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].id, "n1");
}

#[test]
fn migrate_writes_files() {
    let tmp = tempfile::tempdir().unwrap();
    let mut kb = KnowledgeBase::new();
    kb.insert(Node::new("test:a", "Test A", NodeKind::Note, "Body A.").with_tags(["test"]));
    kb.insert(Node::new("test:b", "Test B", NodeKind::Note, "Body B."));

    let opts = MigrateOptions {
        orgroam_naming: false,
        ..Default::default()
    };
    let report = migrate_to_org_dir(&kb, tmp.path(), &opts).unwrap();
    assert_eq!(report.written, 2);
    assert!(tmp.path().join("test-a.org").exists());
    assert!(tmp.path().join("test-b.org").exists());

    let content = std::fs::read_to_string(tmp.path().join("test-a.org")).unwrap();
    assert!(content.contains(":ID: test:a"));
    assert!(content.contains("#+title: Test A"));
    assert!(content.contains("#+filetags: :test:"));
}

#[test]
fn skip_existing_ids() {
    let tmp = tempfile::tempdir().unwrap();

    // Pre-create a file with matching ID
    std::fs::write(
        tmp.path().join("existing.org"),
        ":PROPERTIES:\n:ID: test:a\n:END:\n#+title: Existing\n",
    )
    .unwrap();

    let mut kb = KnowledgeBase::new();
    kb.insert(Node::new("test:a", "Test A", NodeKind::Note, "Body."));
    kb.insert(Node::new("test:b", "Test B", NodeKind::Note, "Body."));

    let opts = MigrateOptions {
        orgroam_naming: false,
        ..Default::default()
    };
    let report = migrate_to_org_dir(&kb, tmp.path(), &opts).unwrap();
    assert_eq!(report.written, 1);
    assert_eq!(report.skipped, 1);
}

#[test]
fn overwrite_existing() {
    let tmp = tempfile::tempdir().unwrap();

    std::fs::write(
        tmp.path().join("existing.org"),
        ":PROPERTIES:\n:ID: test:a\n:END:\n#+title: Old\n",
    )
    .unwrap();

    let mut kb = KnowledgeBase::new();
    kb.insert(Node::new(
        "test:a",
        "Test A New",
        NodeKind::Note,
        "New body.",
    ));

    let opts = MigrateOptions {
        overwrite: true,
        orgroam_naming: false,
        ..Default::default()
    };
    let report = migrate_to_org_dir(&kb, tmp.path(), &opts).unwrap();
    assert_eq!(report.written, 1);
    assert_eq!(report.skipped, 0);
}

#[test]
fn orgroam_naming() {
    let tmp = tempfile::tempdir().unwrap();
    let mut kb = KnowledgeBase::new();
    kb.insert(Node::new(
        "test:hello",
        "Hello World",
        NodeKind::Note,
        "Body.",
    ));

    let opts = MigrateOptions {
        orgroam_naming: true,
        ..Default::default()
    };
    let report = migrate_to_org_dir(&kb, tmp.path(), &opts).unwrap();
    assert_eq!(report.written, 1);

    // File should have timestamp prefix
    let files: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("-hello_world.org"));
    assert!(files[0].len() > 20); // timestamp prefix
}

#[test]
fn slugify_title() {
    assert_eq!(slugify("Hello World"), "hello_world");
    assert_eq!(slugify("MAE Phase 1 — Snippets"), "mae_phase_1___snippets");
    assert_eq!(slugify("simple"), "simple");
}

#[test]
fn days_to_ymd_epoch() {
    assert_eq!(days_to_ymd(0), (1970, 1, 1));
}

#[test]
fn days_to_ymd_known_date() {
    // 2026-05-31 = day 20604 since epoch (approx)
    let (y, m, _d) = days_to_ymd(20604);
    assert_eq!(y, 2026);
    assert!((5..=6).contains(&m)); // May or June depending on exact calc
}

#[test]
fn migrate_between_cozo_stores() {
    let tmp1 = tempfile::tempdir().unwrap();
    let tmp2 = tempfile::tempdir().unwrap();
    let src = crate::CozoKbStore::open(tmp1.path().join("src.cozo")).unwrap();
    let dst = crate::CozoKbStore::open(tmp2.path().join("dst.cozo")).unwrap();

    // Populate source
    src.insert_node(&Node::new("m:1", "Migrate One", NodeKind::Note, "body 1"))
        .unwrap();
    src.insert_node(&Node::new(
        "m:2",
        "Migrate Two",
        NodeKind::Concept,
        "body 2",
    ))
    .unwrap();
    src.push_pending_update("kb-a", "m:1", &[10, 20]).unwrap();

    let report = super::migrate_between_stores(&src, &dst).unwrap();
    assert_eq!(report.nodes_migrated, 2);
    assert_eq!(report.pending_migrated, 1);

    // Verify destination
    let loaded = dst.load_all().unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(dst.get_node("m:1").unwrap().unwrap().title, "Migrate One");

    let pending = dst.drain_pending_updates().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].update_bytes, vec![10, 20]);
}

fn seed_sled(path: &Path, n_nodes: usize) {
    let sled = crate::CozoKbStore::open_with_engine(path, "sled").unwrap();
    sled.seed_type_system().unwrap();
    for i in 0..n_nodes {
        sled.insert_node(&Node::new(
            format!("user:{i}"),
            format!("Note {i}"),
            NodeKind::Note,
            "body",
        ))
        .unwrap();
    }
    if n_nodes >= 2 {
        sled.add_typed_link_with_confidence("user:0", "user:1", "ref", 1.0, 0.9)
            .unwrap();
    }
}

// The three tests below exercise the sled->sqlite migration itself, so
// they need BOTH engines compiled in. This crate's own default features
// are sled-only (`Cargo.toml`), and cozo resolves the engine by *runtime*
// string match — so under a bare `cargo test -p mae-kb` they did not skip,
// they FAILED, with cozo's opaque "engine 'sqlite' not supported (maybe
// not compiled in)". They passed in CI only because `cargo test
// --workspace` unifies `storage-sqlite` in via `mae-core`.
//
// Gating on the feature makes the standalone run honest. Note the same
// asymmetry is why `kb_build`'s round-trip test selects its engine rather
// than hardcoding one.
#[test]
#[cfg_attr(
    not(feature = "storage-sqlite"),
    ignore = "needs storage-sqlite; run via the workspace or -F storage-sqlite"
)]
fn sled_to_sqlite_preserves_nodes_links_and_backs_up() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.cozo");
    seed_sled(&path, 5);
    assert!(path.is_dir(), "sled store is a directory");

    let backup = match migrate_sled_to_sqlite(&path).unwrap() {
        SledToSqliteOutcome::Migrated {
            nodes,
            links,
            backup,
        } => {
            assert_eq!(nodes, 5, "all nodes migrated");
            assert_eq!(links, 1, "the link migrated");
            backup
        }
        other => panic!("expected Migrated, got {other:?}"),
    };

    // Post-migration the store is a sqlite FILE; a fresh open (i.e. "restart")
    // sees every node + the link — the migration is durable.
    assert!(path.is_file(), "post-migration store is a sqlite file");
    let reopened = crate::CozoKbStore::open_with_engine(&path, "sqlite").unwrap();
    for i in 0..5 {
        assert!(
            reopened.get_node(&format!("user:{i}")).unwrap().is_some(),
            "node user:{i} present after migration"
        );
    }
    // Fidelity: the link is a non-body, non-`related_to` ("ref") edge — exactly the
    // kind `update_links_for_node` would DROP if the migration re-derived from body.
    // bulk_import writes it verbatim, so it must survive with its rel_type intact.
    let out_links = reopened.links_from("user:0").unwrap();
    assert_eq!(out_links.len(), 1, "outgoing link preserved");
    assert_eq!(
        out_links[0].rel_type, "ref",
        "non-body / non-related_to link preserved verbatim (not re-derived)"
    );
    // The sled data is preserved (reversible), not deleted.
    assert!(backup.is_dir(), "sled store preserved as a .bak directory");
}

#[test]
// slow fixture (3k inserts) — run explicitly with --ignored; also needs
// storage-sqlite, same as its two siblings above.
#[ignore]
fn bulk_migration_is_fast_not_per_commit() {
    // Regression guard: the bulk `$rows` path must migrate thousands of nodes in
    // ~a second. A per-node (per-commit-fsync) migration took ~13s for 3k here,
    // which would freeze startup + trip the watchdog for a real KB.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.cozo");
    let n = 3000usize;
    let sled = crate::CozoKbStore::open_with_engine(&path, "sled").unwrap();
    sled.seed_type_system().unwrap();
    for i in 0..n {
        sled.insert_node(&Node::new(
            format!("user:{i}"),
            format!("Note {i}"),
            NodeKind::Note,
            "some body text",
        ))
        .unwrap();
    }
    drop(sled);
    let start = std::time::Instant::now();
    let out = migrate_sled_to_sqlite(&path).unwrap();
    let elapsed = start.elapsed();
    eprintln!("migrate {n} nodes took {elapsed:?} — {out:?}");
    assert!(
        elapsed.as_secs() < 6,
        "bulk migration of {n} nodes must be fast (was {elapsed:?}); a regression to \
         per-commit inserts would freeze startup"
    );
}

#[test]
#[cfg_attr(
    not(feature = "storage-sqlite"),
    ignore = "needs storage-sqlite; run via the workspace or -F storage-sqlite"
)]
fn sled_to_sqlite_is_idempotent_and_noop_when_not_sled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.cozo");

    // Absent path → NotNeeded.
    assert!(matches!(
        migrate_sled_to_sqlite(&path).unwrap(),
        SledToSqliteOutcome::NotNeeded
    ));

    // Seed + migrate once.
    seed_sled(&path, 3);
    assert!(matches!(
        migrate_sled_to_sqlite(&path).unwrap(),
        SledToSqliteOutcome::Migrated { .. }
    ));

    // Second run sees a sqlite file → NotNeeded (no double-migration).
    assert!(matches!(
        migrate_sled_to_sqlite(&path).unwrap(),
        SledToSqliteOutcome::NotNeeded
    ));
}
