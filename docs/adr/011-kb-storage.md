# ADR-011: KB Storage Architecture — CozoDB-First with SQLite Bridge

**Status**: Implemented (all phases complete as of v0.12.0)
**Date**: 2026-05-31 (accepted), 2026-06-02 (all phases complete)
**Related**: ADR-004 (KB scaling), ADR-005 (KB CRDT)

## Context

MAE's knowledge base currently has a dual source of truth problem. The runtime
architecture involves three overlapping representations:

1. **Org files on disk** — re-parsed and re-ingested on every startup via
   `kb_ingest`. This is slow, creates startup latency proportional to KB size,
   and means the canonical state is scattered across the filesystem.

2. **In-memory HashMap** — the hot-path data structure used by all KB queries,
   link traversal, and AI tools. Populated from org parsing on startup. Lost
   entirely on crash.

3. **SQLite (persist.rs)** — declared and partially implemented but not wired
   into the hot path. Nodes are written to SQLite but reads still come from the
   HashMap populated by org parsing.

This creates several problems:

- **Reconciliation complexity.** Three representations must agree. Edits made
  via `:kb-edit-source` modify org files, which are re-ingested to update the
  HashMap, but SQLite may drift. Edits made via CRDT sync (ADR-005) update the
  HashMap but may not persist to org files.

- **Crash recovery gaps.** If MAE crashes after an in-memory edit but before
  the org file is written, the edit is lost. SQLite could solve this but is not
  in the read path.

- **Startup cost scales with KB size.** Re-parsing every org file on every
  startup is O(total KB content). A database-backed startup would be O(1) for
  the index load, with content fetched lazily.

- **No graph-native queries.** The HashMap supports direct lookup and one-hop
  link traversal. Multi-hop queries (e.g., "find all nodes within 3 hops of
  concept:buffer"), PageRank, community detection, and shortest-path algorithms
  require either reimplementation in Rust or a graph-native backend.

Every collaborative knowledge tool at scale has converged on database-first
storage: Notion uses Postgres, Roam Research uses Datascript (Datalog),
Logseq is actively migrating FROM flat files TO a database backend (their
"DB version"). File-first architectures do not scale beyond single-user,
single-machine use.

## Decision

A three-phase migration from org-file-first to database-first KB storage:

### Phase 1: v0.11.1 — SQLite as Source of Truth

Wire the existing `persist.rs` SQLite implementation into the hot path. On
startup, load nodes from SQLite instead of re-parsing org files. Org ingestion
(`kb_ingest`) writes to SQLite; subsequent reads come from SQLite. The
in-memory HashMap becomes a cache layer, not the source of truth.

This phase requires no new dependencies and uses code that already exists but
is not wired into the read path.

### Phase 2: v0.12.0 — KbStore Trait Abstraction

Extract a `KbStore` trait with operations: `get`, `put`, `delete`, `search`
(FTS), `links_from`, `links_to`, `graph_query`. Implement `SqliteKbStore` as
the default backend. Implement `CozoKbStore` behind a `cozo` feature flag.

The trait boundary allows testing both backends against the same test suite
and lets users choose based on their needs (SQLite for simplicity, CozoDB for
graph power).

### Phase 3: v0.12.0 — CozoDB as Default Backend (COMPLETE)

CozoDB is now the default KB backend. Feature flag removed, always compiled.
SQLite available as explicit fallback via `kb_backend = "sqlite"` option.

Capabilities shipped:
- **14 NodeKind variants** — Index, Command, Concept, Key, Note, Project,
  Category, Lesson, Tutorial, Meta, Block, SchemeApi, Task, View
- **20 typed relationship types** with declared inverses — `implements`,
  `teaches`, `requires`, `configures`, `documents`, etc.
- **95+ typed seed relationships** replacing flat `related_to` links
- **9 CozoDB relations** beyond nodes/links — node_types, rel_types, blocks,
  meta_members, node_versions, views, hygiene_suggestions, instance_meta,
  embeddings
- **Meta-node composition** — cached body from component nodes, refresh on demand
- **Block-level addressing** — `parent_id#N` for paragraph-level references
- **Agenda queries via Datalog** — Todo, Priority, Tag, Stale, Orphan, DeadEnd,
  Custom filters
- **Node versioning** — snapshot on update, history, point-in-time restore
- **HNSW vector index** — 384-dim F32 Cosine distance (schema ready, populated
  in v0.13.0 when embedding providers are wired)
- **6 pre-built view seeds** — kanban, backlog, sprint, timeline, agenda, custom
- **Federation instance identity** — UUID per instance in instance_meta
- **28-test graph validation suite** — full seed manual as structured test fixture
- **5 new AI tools** — kb_agenda, kb_history, kb_restore, kb_view_query,
  kb_vector_search

## Rationale

- **CozoDB is Rust-native and embeddable.** No external process, no socket
  protocol, no version mismatch. Links as a regular Rust dependency via
  `cozo` crate. Same deployment story as SQLite.

- **KbStore trait preserves fallback.** Users who do not need graph queries
  can stay on SQLite. The trait boundary is the escape hatch.

- **Org files become import/export, not runtime state.** This matches how
  every successful knowledge tool handles the file-vs-database tension. Files
  are for interchange and version control. The database is for runtime.

- **Incremental migration.** Phase 1 is a wiring change with zero new deps.
  Phase 2 is a refactor. Phase 3 is additive. No big-bang migration required.

## Consequences

- **Org files become an import/export format.** They are no longer re-parsed on
  every startup. Users who want git-friendly snapshots of their KB use
  `:kb-export` to write org files on demand. `:kb-ingest` remains for initial
  import.

- **SQLite/rusqlite removed (v0.12.0).** `SqliteKbStore` and `persist.rs` deleted.
  CozoDB with sled storage is the sole KB backend. See ADR-012.

- **CozoDB learning curve.** Datalog is unfamiliar to most developers. This is
  mitigated by a thin Rust wrapper in `mae-kb` that exposes idiomatic Rust
  methods (`kb.related_nodes(id, depth)`) rather than raw Datalog strings.

- **CRDT sync (ADR-005) must target KbStore trait.** The `node_to_crdt` /
  `crdt_to_node` bridge in mae-kb must write through the trait, not directly
  to HashMap or SQLite. This is a Phase 2 requirement.

- **Startup time improves.** Database-backed startup replaces full org parse.
  For a 500-node KB, expected improvement from ~2s (parse all org) to ~50ms
  (SQLite index load).

## References

- [CozoDB documentation](https://docs.cozodb.org/)
- [CozoDB crate](https://crates.io/crates/cozo)
- [Logseq DB version announcement](https://blog.logseq.com/logseq-db-version/)
- ADR-004 (KB scaling)
- ADR-005 (KB CRDT)
