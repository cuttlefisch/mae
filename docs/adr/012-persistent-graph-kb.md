# ADR-012: Persistent Graph KB with Pre-Built Manual

**Status:** Accepted
**Date:** 2026-06-02
**Deciders:** @cuttlefisch

## Context

MAE's KB had a split-brain problem: `seed_kb()` rebuilt ~860 manual nodes from compiled-in `&'static str` constants on every startup. These nodes lived only in the in-memory `KnowledgeBase` HashMap. Meanwhile, CozoDB stored user-created nodes, typed relationships, views, and version history persistently.

Additionally, the dependency declarations were triply inconsistent:
- `Cargo.toml` declared `features = ["storage-new-rocksdb"]` (non-existent in cozo 0.7.6)
- `Cargo.lock` resolved to `sled`
- `cozo_store.rs` opened with `DbInstance::new("sqlite", ...)` (wrong engine)

`rusqlite` was declared in mae-kb despite the dead `sqlite-legacy` feature flag, and its `libsqlite3-sys` creates a linker conflict (`links = "sqlite3"`) with CozoDB's `sqlite3-src`.

## Decision

### Phase 0: Remove rusqlite, fix backend

1. Remove `rusqlite` from mae-kb entirely (SqliteKbStore, persist.rs deleted)
2. Fix CozoDB to use sled storage (the only backend that compiles without conflict)
3. Consolidate `kind_to_str`/`str_to_kind` into `NodeKind::as_str()`/`from_str_lossy()`
4. Remove `kb_backend` config option (CozoDB is the sole backend)
5. Deprecate `:kb-save`/`:kb-load` commands (persistence is now automatic)

### Phase 1: Pre-built manual KB

1. `build-manual-kb` binary generates a versioned `.cozo` file from `seed_kb()` output
2. At startup, locate and validate the pre-built manual KB (SHA-256 checksum)
3. Load manual nodes from CozoDB into in-memory KB (fast) instead of re-seeding
4. User KB is separate (read-write, XDG data dir)
5. `Editor::with_kb()` constructor enables pre-populated KB creation

## Storage Backend: Sled (not SQLite)

The plan called for CozoDB's native SQLite storage (`storage-sqlite-src`), but this
conflicts with `rusqlite` via the shared `links = "sqlite3"` Cargo metadata. The editor
workspace uses sled for CozoDB. The daemon (separate workspace/Cargo.lock) uses CozoDB
with `storage-sqlite` + the `sqlite` crate for collab WAL persistence, avoiding the conflict.

| Backend | Status | Notes |
|---------|--------|-------|
| `storage-sled` | **Current** | Pure Rust, compiles everywhere, no linker conflicts |
| `storage-sqlite-src` | Deferred | Conflicts with daemon's rusqlite (separate workspace) |
| `storage-rocksdb` | Rejected | 35MB binary overhead, C++ build deps |

## Architecture

```
mae binary
+-- Pre-built Manual KB (versioned .cozo file, read-only)
|   Built from seed content at release time
|   SHA-256 checksum validated at startup
|   862 nodes (commands, concepts, lessons, tutorials, scheme API)
|
+-- User KB (read-write, XDG data dir)
|   ~/.local/share/mae/kb/local/primary.cozo
|
+-- Imported KBs (read-write, per-source)
    ~/.local/share/mae/kb/instances/{slug}/
```

### Startup Flow

1. Locate pre-built manual KB (env var > config > well-known paths)
2. Validate SHA-256 checksum against known releases
3. Load manual nodes into in-memory `KnowledgeBase`
4. Open user KB (CozoDB), load user nodes
5. Open registered imported KBs, load their nodes
6. Editor starts with fully populated KB

### Build-Time Flow

```
make manual-kb
  -> cargo run --bin build-manual-kb -- assets/mae-manual.cozo
  -> seed_kb() -> persist_nodes() -> seed_type_system() -> seed_typed_relationships() -> seed_views()
  -> SHA-256 checksum written to assets/mae-manual.cozo.sha256
```

### Phase 2: CozoDB-Direct Ingestion Pipeline

1. `import_org_dir_to_store()` writes parsed nodes directly to CozoDB (no intermediate SQLite)
2. `IngestMode` enum: `Full` (re-parse all files) vs `Incremental` (skip unchanged files via content hash)
3. `source_files` CozoDB relation tracks file paths, content hashes, and associated node IDs
4. Enhanced `ImportReport` with `nodes_updated`, `nodes_unchanged`, `nodes_removed`, `duration_ms`
5. `:kb-reimport <name> [full|incremental]` command and AI tool support mode selection
6. `kb_register()` and `kb_reimport()` route through CozoDB-direct ingestion automatically

### Phase 3: Scale Validation

Integration test (`cozo_scale_test.rs`) validates CozoDB at roamnotes-scale:
- 2,500 nodes with realistic body text + tags
- ~15,000 typed links across 8 relationship types
- Benchmarked operations: get_node, FTS, links_from/to, neighborhood, Datalog, load_all
- All operations complete within thresholds (debug build; release is 5-10x faster)

## Consequences

### Positive

- **Instant AI context**: Manual KB loads from CozoDB in <100ms vs ~500ms for seed_kb()
- **Clean separation**: Manual (read-only) vs user (read-write) vs imported KBs
- **No rusqlite in mae-kb**: Eliminates ~2,500 lines of dead SQLite code
- **Single backend**: No backend selection logic, no migration paths
- **Shippable artifact**: `.cozo` file is a versioned, checksummed release artifact

### Negative

- **Sled is unmaintained**: No new releases since 2021, but functional for our use case
- **Directory-based storage**: Sled creates a directory tree, not a single file (complicates packaging slightly)
- **Build step required**: `make manual-kb` must run before shipping (CI handles this)

### Neutral

- Org source files remain ground truth for manual content (in `kb_seed/`)
- `seed_kb()` is still called when no pre-built manual is found (fallback)
- State-server still uses rusqlite independently (no change)

## References

- ADR-011: KB Storage (updated: SQLite removed)
- Plan: `~/.claude/plans/twinkly-tickling-tarjan.md`
