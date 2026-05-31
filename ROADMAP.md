# MAE Roadmap

**Current version:** v0.11.0 · **Tests:** 5,796+ passing · **Status:** Alpha — Phases 1-13 complete, Phase 12 (collab) protocol-complete + PSK auth + KB sharing E2E, Phase 13 (Scheme runtime) complete. v0.11.1: KB storage architecture (SQLite-first + KbStore trait).

---

## Phase Summary

| Phase | Status | Summary |
|-------|--------|---------|
| 1. Core + Renderer | ✅ Complete | Buffer (rope), event loop, terminal renderer, vi modal editing |
| 2. Scheme Runtime | ✅ Complete | R7RS-small (mae-scheme), `init.scm`, `define-key`, `set-option!`, REPL |
| 3. AI Integration | ✅ Complete | Claude/OpenAI/Gemini/DeepSeek, 450+ tool-calling, conversation UI, permissions |
| 4. LSP + DAP + Syntax | ✅ Complete | Full LSP (rename, format, outline, breadcrumbs, peek), DAP (watches, exceptions), 13-language tree-sitter |
| 5. Knowledge Base | ✅ Complete | SQLite + FTS5, org parser, 200+ nodes, bidirectional links, federation |
| 6. Embedded Shell | ✅ Complete | alacritty_terminal, MCP bridge, file auto-reload, send-to-shell |
| 7. Documentation | ✅ Complete | Tutor (13 lessons), `:describe-configuration`, `--check-config`, `--init-config` |
| 8. GUI Backend | ✅ Complete | winit + Skia 2D, inline images (PNG/JPG/SVG), variable-height, inertial scroll |
| 9. Babel + Export | ✅ Complete | 12-language executor, HTML/Markdown export, noweb, tangle, KB federation |
| 10. AI Agent Efficiency | ✅ Complete | Tiered prompts, provider-aware hints, target dispatch, frame profiling |
| 11. Module System | ✅ Complete | 9 modules extracted (Doom model), `module.toml` manifests, `mae pkg` CLI, flags, live reload |

---

## Known Bugs

### Pre-existing

- [ ] **AI output buffer cursor invisible in GUI**: After AI responds, the cursor in the `*ai*` conversation output buffer is not visible. Root cause: buffer type / layout metadata mismatch — the conversation buffer doesn't provide the same state that the cursor renderer expects. Low priority (output buffer is read-only, navigation still works).
- [ ] **Theme load failure is silent in headless mode**: If config.toml requests a nonexistent theme, `set_theme_by_name()` shows a status bar message but keeps the current theme. In CI/headless mode the user gets zero feedback. Should log to stderr or return non-zero exit from `--check-config`.
- [ ] **Status bar `[ReadOnly]` confusing during collab**: The `[ReadOnly]` badge is the AI permission tier (`status.rs:271`), not a buffer property. During collab sessions users mistake it for a collab-imposed restriction. Consider: rename to `[AI:RO]` or `[Tier:RO]`, or hide when no AI session is active.

### Collaborative Editing (v0.11.0)

- [x] **One-directional sync**: cli1→cli2 works but cli2→cli1 does not. Root cause: `biased` tokio::select starved TCP reads. Fix: remove `biased;` from connected select loop.
- [x] **First `SPC C j` unresponsive from Dashboard**: Join only works after a `SPC C D`/`SPC C i` round-trip. Root cause: splash screen intercept swallows `j` during multi-key sequences. Fix: add `pending_keys.is_empty()` guard.
- [x] **Syntax highlighting differs on join**: Joiner sees wrong colors (purple bullets, green title). Root cause: `set_language` without `invalidate()` leaves no tree-sitter parse tree. Fix: call `syntax.invalidate(idx)` after join.
- [x] **Per-user CRDT undo**: yrs `UndoManager` with per-origin undo stacks. Local edits use origin-tagged transactions; `undo()`/`redo()` generate CRDT-native inverse operations (no more `reconcile_to()` round-trip). Remote edits excluded from local undo stack. `enable_undo()` called in `enable_sync()`/`load_sync_state()`. `capture_timeout_millis: u64::MAX` with explicit `undo_reset()` at dispatch boundaries — vim insert-mode groups all chars into one undo item. *(12f8ce4)*
- [x] **`:w` fails on non-sharer clients**: Save works only for the client that originally opened and shared the file. Other clients (including those that outlive the sharer) get errors. Root cause: `file_path` not properly resolved on join, or save protocol assumes original sharer identity. *(8de53b8)*
- [x] **Sharer quit doesn't notify peers or stop sharing**: When the client that triggered the share disconnects, peers are not notified and the shared document lingers. Need graceful disconnect protocol: server detects client drop → notifies remaining peers → optionally promotes new owner or marks doc read-only. *(8de53b8)*
- [x] **Client disconnect lifecycle undefined**: No documented or tested behavior for: client crash, network drop, graceful quit, last-client-leaves. Must define and implement industry-standard behavior (cf. VS Code Live Share, Google Docs). Document in `docs/COLLABORATION.md`. *(8de53b8)*
- [x] **Collab e2e test harness missing**: 15 E2E tests (in-memory Client harness + 9 TCP network tests) covering share/join/edit/sync/disconnect/eviction/convergence.
- [x] **Edits lost during share round-trip (BUG A)**: Optimistically track doc in `collab_synced_buffers` immediately, with `ShareFailed` rollback on server error.
- [x] **Eviction doesn't delete from SQLite (BUG B)**: `evict_idle()` now deletes from storage after removing from HashMap.
- [x] **Inconsistent snapshots in sync/resync and sync/diff (BUG C)**: Atomic `encode_state_and_sv()` and `encode_diff_and_sv()` methods under single lock.
- [x] **sync/share loses connected_clients (BUG D)**: Atomic `share_doc()` method sets `connected_clients=1` on creation.
- [x] **Missing subscription types (BUG E)**: `send_subscribe()` now includes `peer_joined`, `peer_left`, `save_committed`.

### Deferred to v0.12+ (Collab)

- [x] **Offline edit recovery**: Preserve `sync_doc` during disconnect, reconcile on rejoin instead of full-state overwrite. *(b8d4b6a)*
- [x] **Client-side gap detection**: Track `wal_seq` from notifications, trigger auto-resync on gaps. *(b8d4b6a)*
- [x] **Save protocol wiring**: Call `docs/save_intent` + `docs/save_committed` from editor's `:w` for synced buffers.
- [x] **Cursor positioning after CRDT undo**: yrs `StackItem<CursorMeta>` via `observe_item_added/popped` — exact cursor restore after CRDT undo/redo. *(fb5120b)*
- [x] **Undo capture timeout tuning**: Fixed in 12f8ce4 — `capture_timeout_millis: u64::MAX` with explicit `undo_reset()` at dispatch boundaries. Vim insert-mode groups all chars into one undo item.
- [x] **Cursor drift on remote edits**: Snapshot old rope before `apply_sync_update`, find first divergence point, shift cursors by character delta. *(01f11fc, 92a20b8)*
- [x] **Modified flag with CRDT undo**: SHA-256 content hash comparison instead of monotonic state vectors. `saved_content_hash` captured after save, compared in undo/redo paths. *(92a20b8)*
- [x] **Docker E2E test re-enabled**: Phase 13f async/yield wiring complete. `sleep-ms` and `wait-for-file` now yield to the event loop. Docker E2E re-enabled in CI (66 assertions + 9 verifier checks). New event-driven primitives: `yield-tick` (drain one event loop iteration), `await-hook` (suspend until named hook fires), `await-condition` (predicate wait without polling). *(39caf8e)*
- [x] **Undo stack size limit for CRDT**: `set_undo_limit()` on TextSync with `DEFAULT_UNDO_LIMIT` (1000). *(fb5120b)*
- [x] **Awareness protocol**: Cursor/selection sharing via `sync/awareness` JSON-RPC relay. 8-color WCAG AA palette, 50ms throttle, 30s timeout, echo filtering. GUI (2px bar + labels + off-screen ▲/▼) and TUI (underline + initial + ▲/▼) rendering. Status bar presence. Auto-derived user identity (git → $USER → hostname). 12 tests.
- [x] **Heartbeat/keepalive**: Detect silent client death, clean up stale `connected_clients`. *(b8d4b6a)*

### KB Format Canonicalization

- [ ] **Org-mode as canonical KB format**: Enforce org-mode (with properties drawer, filetags, org-roam structure) as the single KB storage format. Convert markdown KBs to org on import. Benefits: properties drawers for metadata, filetags for classification, structured `[[id:...]]` links, babel code blocks, export pipeline. Markdown KBs imported via existing `markdown_to_org` conversion. Track `format: org` in KB instance metadata for forward compatibility.

### Release Artifact Packaging

- [x] **Linux TUI**: `.tar.gz` archive containing `mae` + `mae-state-server` static musl binaries.
- [x] **Linux GUI**: AppImage (`.AppImage`) — portable, no install required. Uses existing `mae.desktop` + `mae.svg`.
- [x] **macOS GUI**: `.app` bundle in `.zip` — Finder-compatible, with `Info.plist`, icon, launcher script.
- [x] **macOS TUI**: `.tar.gz` archive containing `mae` binary.
- [ ] **macOS code signing**: Ad-hoc or Apple Developer signing for Gatekeeper. Currently unsigned (requires `xattr -c` to run).
- [ ] **Linux Flatpak/Snap**: Alternative packaging formats for distro app stores.

### Org-Mode Rendering

- [x] **Org rendering in editing buffers**: Full structural spans via `compute_org_spans()` — TODO/DONE, checkboxes, priorities, drawers, timestamps, directives, links, tables. 46 regression tests. *(12abab8)*
- [x] **KB node edit mode rich formatting**: KB view uses `compute_org_spans()` for full org structural rendering (replaced heading-only spans). *(12abab8)*
- [x] **Word-wrap indentation for list items**: `content_indent_len()` now detects list markers (`- `, `+ `, `* `, `1. `) and indents wrap continuations past the marker. Both GUI and TUI.
- [x] **`fill-paragraph` / `M-q`**: Hard-wrap at `fill_column` (default 80), respects list-item hanging indent. `fill-region` for visual selection is TODO.

### Line Numbers & Wrapping

- [x] **Relative line numbers with word-wrap**: GUI now uses buffer-row distance for relative numbers in wrapped mode, not display-row distance (which inflated counts by including continuation rows).

---

## In Progress / Planned

### Phase G: Feature Crate Extraction + Babel Gaps (v0.9.0) — COMPLETE

- [x] `mae-babel` crate extracted from `mae-core` (7 files, zero import changes downstream)
- [x] `mae-export` crate extracted from `mae-core` (3 files, depends on `mae-babel`)
- [x] Block ref resolution: `resolve_block_ref()` reads cached `#+RESULTS:` (was stub)
- [x] Persistent REPL sessions: `SessionManager` with sentinel-based output capture
- [x] Per-language backends: `LanguageBackend` trait + 4 implementations (shell, script, compiled, internal)
- [x] Compiled backend: hash-based binary caching in `~/.cache/mae/babel/`
- [x] `PendingSchemeEval` variant: scheme blocks routed to editor runtime
- [x] Babel edit-special: `SPC m '` opens src block in dedicated buffer with language mode
- [x] Babel edit-commit: `SPC m '` in edit buffer writes body back to source

### Near-term: Server-Client Architecture

- [ ] **Multi-AI file contention protocol**: When multiple AI-assisted editors (MAE, VS Code + Copilot, Cursor, aider) operate on the same project simultaneously, file writes race, LSP state goes stale, and undo histories diverge. Short-term: git worktree isolation (each agent in its own worktree, merge at commit time). Medium-term: advisory file locks (`.mae.lock`), inotify coordination to detect external changes and pause AI operations. Long-term: canonical state server (see below).
- [x] **State server v1** (`mae-state-server` binary): Standalone CRDT sync server over TCP (port 9473). Per-document locking, WAL-first SQLite persistence, periodic compaction, transport-generic I/O (reuses `mae_mcp` primitives). Sync protocol: `sync/update`, `sync/state_vector`, `sync/full_state`, `sync/diff`. No auth (trusted LAN only).
- [x] **State server v1.5** (scalability + UX): Sharded SQLite pool (4 shards), save protocol (SHA-256 content-hash), event sequence tracking (wal_seq), background compaction + idle eviction. Editor: 7 commands (SPC C prefix), 4 AI tools, status bar segment, 5 options, doctor integration, audit_configuration collab section. New methods: `sync/resync`, `docs/stats`, `docs/save_intent`, `docs/save_committed`, `$/debug`.
- [x] **Client ID echo filtering**: Server `broadcast_except()` skips the originating session on `sync/update`. Eliminates wasted bandwidth/CPU from self-echo and prevents share duplication race.
- [x] **Collab stub audit** (v0.11.0 correctness): Systematic review completed. Resolved items:
  - ~~`docs/save_committed` handler is a no-op stub~~ — NOT a no-op: broadcasts `SaveCommitted` to peers (handler.rs:463-492)
  - ~~`track_client_connect()` / `track_client_disconnect()` dead code~~ — called from handler.rs on `sync/update`, `sync/full_state`, `sync/resync`, `sync/share`, and session teardown
  - ~~`DocAddress` enum never used~~ — used in `compute_doc_address()` (collab_bridge.rs) + `BufferJoined` handler
  - ~~Per-doc `connected_clients` never incremented~~ — tracked by `share_doc()` (=1) + `track_client_connect/disconnect` in handler
  - ~~No `peer_joined`/`peer_left` events~~ — exist in `EditorEvent`, broadcast by server on connect/disconnect
  - `SaveIntentResult` returned by server, now consumed by editor save path ✅
  - `save_intent` now called from editor `:w` for synced buffers ✅
  - `docs/metadata` endpoint added to state server ✅
  - `WalEntry::client_id` stored but never read for audit/attribution (deferred — needs Phase F auth)
  - `StorageError::Io` variant reserved but unused (pluggable backends — by design)
- [ ] **State server v2** (Phase F): Auth tiers (PSK → SSH → OAuth/OIDC), update compression (msgpack), multi-machine sync. Completed: awareness protocol ✅, per-user undo ✅ (yrs `UndoManager`), git-based identity ✅, heartbeat/keepalive ✅, buffer status indicators ✅, Bugs 2-4 ✅ *(8de53b8)*, PSK mutual auth ✅ *(fffa39f)*, KB protocol handlers ✅ *(fffa39f)*, **KB sharing E2E** ✅ (bridge + continuous sync + offline + mDNS). Next: SSH key exchange, msgpack wire format.
  - **SSH Key Exchange Authentication** (deferred from v0.11.0):
    - Ed25519 keypair generation + TOFU trust store (`~/.config/mae/trusted_keys.toml`)
    - `SshAuth` provider implementing existing `AuthProvider` trait (`crates/sync/src/auth.rs`)
    - Client-side auth in `collab_bridge.rs` (currently sends `initialize` with no auth or PSK)
    - Crates: `ed25519-dalek` v2, `ssh-key` v0.6
    - Prior art: SSH RFC 4252 challenge-response, Syncthing device IDs, WireGuard Noise_IKpsk2
    - See `research:ssh-key-exchange-patterns` KB node for full analysis
    - Open questions: reuse `~/.ssh/id_ed25519` vs generate separate key, UI for TOFU accept/reject, key revocation model
- [ ] **Enterprise KB server**: Shared KB instance serving development teams + AI agents. Scaling tiers:
  - *Tier 1* (5-20 users, <20K nodes): Shared SQLite in WAL mode + connection pool + TCP proxy. ~1 week effort.
  - *Tier 2* (20-100 users, <100K nodes): Dedicated `mae-kb-server` microservice with HTTP/gRPC API, write-ahead buffer, read replicas, vector embeddings for semantic search. ~1 month.
  - *Tier 3* (100+ users, 500K+ nodes): PostgreSQL + pgvector, write sharding by namespace, event sourcing for real-time sync. ~3 months.
  - Key bottlenecks: SQLite single-writer ceiling (~50 writes/sec), FTS5 index size at scale (~400MB at 100K nodes), network latency for RAG workflows (5-10 KB queries per AI turn × 30 concurrent agents = ~600 node fetches/sec peak).
- [x] **CRDT collaborative editing (yrs/YATA)**: Sync engine: yrs (Yjs Rust port). Per-user cursors via Awareness protocol, per-user undo via yrs UndoManager, conflict-free merge for concurrent AI and human edits. Dual structure: yrs YText + ropey mirror. See ADR-002, ADR-005, ADR-006.
  - Phase A: `mae-sync` crate (yrs dependency, document schemas, ropey bridge) ✅
  - Phase B: Buffer integration (sync_doc field, local edits → yrs transactions) ✅
  - Phase C: MCP sync methods (state_vector, apply_update) ✅
  - Phase D: Push-based sync event broadcasting ✅
  - Phase E (state-server): TCP transport, WAL persistence, per-doc locking ✅
  - Phase F: Awareness protocol ✅, per-user undo ✅, multi-machine sync
- [ ] **Networked feature E2E coverage gate**: Every networked feature (sync, save, awareness, auth) requires E2E test coverage before release. Coverage targets:
  - Save protocol: save_intent → hash check → save_committed → peer notification (~80%)
  - WAL gap recovery: trigger gap via server restart, verify ForceSync completes (~50%)
  - Disconnect/reconnect: pending sends, timeout, partition, duplicate updates (~80%)
  - Multi-document: doc ID collisions, focus switching, cross-doc isolation (~60% — multi-doc tests added 463e859)
  - Error paths: oversized updates, malformed CRDT, server errors (~70%)
  - Notifications: sharer_left, peer_count_changed, peer_saved (~60%)
  - SQLite persistence: WAL durability, crash recovery (~60% — WAL recovery test added 463e859)
  - Awareness: cursor/selection relay, timeout, echo filtering (~80% — E2E awareness tests dc13e13)
  Methodology: verify protocol soundness → validate test methodology → ensure containers work without tests → wire tests one by one. Event-driven testing primitives (`yield-tick`, `await-hook`, `await-condition`) eliminate sleep-based coordination.

### KB Enterprise Readiness & Hardening

- [x] **Change management**: `node_changelog` table with full audit trail (create/update/delete, old/new values, timestamps, author, reason). Schema v6 migration.
- [x] **Incremental sync**: `sync_to_sqlite()` — only writes changed nodes, records all mutations in changelog.
- [x] **Structured timestamps**: `created_at` / `updated_at` INTEGER columns on `nodes`. Enables `ORDER BY updated_at` without JSON parsing.
- [x] **Changelog query API**: `node_history()`, `changes_since()` for auditing and time-travel.
- [ ] **Point-in-time restore**: `kb_restore` command + MCP tool to revert a node to any prior state from changelog.
- [ ] **Node blame**: Per-change author tracking. Requires session identity propagation from MCP client → KB write path.
- [ ] **Changelog pruning**: Configurable retention policy (default: 90 days). `kb-changelog-prune` command.
- [ ] **KB backup/export**: `kb-export` dumps full KB + changelog to portable format (SQLite file or JSON). `kb-import` restores.
- [ ] **Conflict detection**: When multi-client writes land on same node, detect via version counter and surface conflict to user (not silent last-write-wins).
- [ ] **KB replication**: Read replicas for high-read-throughput scenarios (AI agents doing 600+ node fetches/sec). WAL mode enables this natively for same-host.

### KB Storage Architecture (v0.11.1 — ADR-011)

**Status**: Phase A COMPLETE, Phase B COMPLETE. CozoDB backend available behind feature flag.

The KB had a dual source of truth problem: org files re-parsed on startup, SQLite declared but unused in hot path. Every collaborative tool at scale uses a database (Notion/Postgres, Roam/Datascript, Logseq migrating FROM files TO DB).

**Decision**: CozoDB-first with SQLite bridge period. See `docs/adr/011-kb-storage.md`.

- [x] **KbStore trait** (`crates/kb/src/store.rs`): Database-agnostic persistence interface — node CRUD, FTS search, link queries, CRDT ops, pending update queue. `SqliteKbStore` implementation with 11 tests.
- [x] **SQLite-first startup**: Federated KB instances load from SQLite first, fall back to org import + one-time migration to SQLite.
- [x] **Write-through persistence**: `kb_create_node`, `kb_update_node`, `kb_delete_node` write through to `SqliteKbStore`.
- [x] **Durable offline queue**: Pending CRDT updates stored in SQLite `pending_updates` table (survives crashes). Drained on reconnect.
- [x] **Primary KB store**: `KbContext.store` field holds `Arc<dyn KbStore>` for the primary KB instance (supports any backend).
- [x] **CozoKbStore**: `#[cfg(feature = "cozo")]` feature-flagged CozoDB backend — Datalog queries, typed relationships, shortest path, neighborhood BFS. 12 tests.
- [x] **SQLite → CozoDB migration**: `migrate_between_stores()` in `crates/kb/src/migrate.rs` — cross-store data migration with report.
- [x] **Graph-native AI tools**: `kb_shortest_path`, `kb_neighborhood`, `kb_add_link`, `kb_raw_query` — delegate to KbStore, graceful NotSupported on SQLite.
- [x] **KB lifecycle E2E**: 24 Rust tests + 3 Scheme test files covering persistence, CRDT, offline queue, import/export, performance.
- [ ] **GraphRAG** (v0.14.0): Hybrid vector + graph retrieval via CozoDB — single Datalog query combining HNSW entry points + graph expansion.

### Phase 13: MAE Scheme Runtime (v0.12.0)

**Status**: Phases 13a–13h COMPLETE. Purpose-built R7RS-small runtime replaces
the previous Steel dependency. 1,800+ mae-scheme tests passing, 261 stdlib
functions, 41 special forms, 23 opcodes, hygienic macros, module system, call/cc,
dynamic-wind, exception handling. All 177 editor registrations ported. In-process
LSP + DAP for Scheme (first Scheme DAP ever). Introspection + observability.

#### Core: R7RS-small Compliance (COMPLETE)

- **Standard library**: All R7RS-small libraries implemented (`(scheme base)`,
  `(scheme write)`, `(scheme time)`, `(scheme file)`, `(scheme process-context)`,
  `(scheme char)`, `(scheme read)`, `(scheme lazy)`, `(scheme case-lambda)`,
  `(scheme inexact)`, `(scheme cxr)`, `(scheme eval)`, `(scheme r5rs)`)
- **Proper tail calls**: All tail contexts (if, cond, case, when, unless, and, or,
  begin, let, do, guard, dynamic-wind)
- **First-class continuations**: `call/cc`, `call-with-current-continuation`,
  `dynamic-wind` with VM-level winder stack
- **Hygienic macros**: `syntax-rules` with SRFI-46 custom ellipsis
- **Multiple values**: `values` / `call-with-values` (list representation)
- **Libraries**: `(define-library ...)` / `(import ...)` / `(export ...)` with
  `only`, `except`, `prefix`, `rename` modifiers
- **Numeric tower**: i64 fixnums + f64 floats (no bignums/rationals/complex)
- **Exception system**: R7RS §6.11, Chibi-Scheme unified handler stack pattern

#### Extensions: `mae:` Namespace

All MAE-specific functionality lives in `(mae ...)` libraries:

```scheme
(import (scheme base)
        (mae buffer)      ; buffer-insert, buffer-string, buffer-undo, etc.
        (mae editor)      ; dispatch, modes, keymaps, options
        (mae async)       ; sleep, wait-for, yield, spawn-fiber
        (mae test)        ; describe, it, should, should-equal
        (mae collab)      ; collab-status, collab-share, sync primitives
        (mae lsp)         ; definition, references, hover, diagnostics
        (mae dap)         ; breakpoints, step, inspect
        (mae kb)          ; search, get, create, link
        (mae shell))      ; send, read-output, cwd
```

#### Key Design Decisions

| Decision | Rationale | Precedent |
|----------|-----------|-----------|
| R7RS-small core, not R7RS-large | Small spec = complete implementation. Large spec is optional modules | Chibi-Scheme, Chicken, Guile |
| `mae:` namespace, not flat global | Prevent collisions as API grows. Clear provenance | Emacs `emacs-`, Guile modules, Racket collections |
| Async/yield via VM opcodes | `sleep`, `wait-for-file`, `wait-until` yield to host event loop | Guile fibers, Racket threads, Chez `engine` |
| Rust FFI raises Scheme errors | `register_fn` returns `Result<Value, LispError>` | Guile's `scm_throw`, Racket's `raise` |
| Rc-based GC (stage 1) | Simple, correct. Tracing GC planned for stage 2 | Architecture Principle #1 |
| Bytecode VM, not tree-walking | Performance for hot paths (rendering hooks, input processing) | Guile 3.0, Chez, Racket BC |
| Immutable strings (Rc<str>) | Thread-safe, SRFI-140 compatible | Racket, Chibi-Scheme |
| Immutable pairs (Rc) | No RefCell overhead, simpler GC | Racket (default) |

#### Prior Art Study

| System | What MAE takes | What MAE avoids |
|--------|---------------|-----------------|
| **Emacs Lisp** | Dynamic scope option for hooks, `defadvice`, `defcustom` pattern, buffer-local variables | Dynamic scope as default, no modules, no TCO, no hygiene |
| **Guile Scheme** | Module system (`define-module`), delimited continuations, Rust/C FFI patterns | Slow startup (~200ms), heavy runtime, complex build |
| **Racket** | `#lang` extensibility, contract system, exceptional docs | 200MB runtime, poor embedding story, non-standard |
| **Chibi-Scheme** | Minimal R7RS-small, <1MB, designed for embedding, exception system architecture | Limited ecosystem, no JIT, slow numerics |
| **Chez Scheme** | Compilation strategy, `engine` for preemption | Complex bootstrap, not designed for embedding |

#### Implementation Phases

- [x] **Phase 13a**: Reader/parser (S-expressions, datum labels, `#;` comments)
- [x] **Phase 13b**: Bytecode compiler + VM (stack-based, tail-call elimination)
- [x] **Phase 13c**: R7RS-small base library (261 functions, 13 libraries)
- [x] **Phase 13d**: Hygienic macros + module system (`define-library`, `import`)
- [x] **Phase 13e**: FFI layer — port all 177 editor registrations to mae-scheme VM
- [x] **Phase 13f**: Async/yield — `sleep-ms`/`wait-for-file` yield to event loop, auto-flush wrappers, Docker E2E re-enabled
- [x] **Phase 13g**: LSP + DAP for mae-scheme — in-process Swank-style (first Scheme DAP ever)
  - LSP: completion (live globals), hover (docstrings), diagnostics (check-syntax), symbols, signature help
  - Source maps: compiler-tracked locations, `read_all_located()` + `compile_top_level_located()`
  - DAP: yield-based breakpoints (Guile VM trap model), step modes, frame inspection
  - Bridge: `scheme_lsp_bridge.rs` + `scheme_dap_bridge.rs` intercept intents in-process
- [x] **Phase 13h**: Introspection + observability — `introspect.rs`, docstring extraction, `gc-stats`, KB auto-seeding
- [x] **Phase 13i**: Migration — Steel fully removed (13e), test files clean R7RS, no workarounds remain
- [x] **Phase 13j**: Documentation — ADR-009, EXTENSION_GUIDE updated with libraries/async/debug/introspection

#### Success Criteria

- [x] All 177 editor registrations ported from previous runtime
- [x] `register_fn` returns `Result` (errors propagate as Scheme exceptions)
- [x] `define_global` properly updates existing bindings (no shadowing)
- [x] No unmaintained transitive dependencies (`steel-core` removed)
- [x] Module system prevents namespace collisions
- [x] 1,800+ mae-scheme tests passing (5,494 workspace total)
- [x] `wait-for-file` and `wait-until` actually block/yield (Docker E2E re-enabled)
- [x] In-process LSP + DAP for Scheme files
- [x] Introspection: `procedure-arity`, `procedure-documentation`, `gc-stats`, KB auto-seeding
- [x] ADR-009 documenting the architecture decision
- [x] All existing `init.scm` configs load with at most deprecation warnings

### Future: Scheme Introspection Enhancements (from prior art research)
- [ ] **Execution history ring buffer** — MIT Scheme's debugger records expressions in a ring buffer, providing history for tail-called expressions that no longer appear on the stack. Valuable for debugging tail-recursive code in mae-scheme. Ref: [[RoamNotes: Scheme Debugger Architectures]]
- [ ] **Cross-reference analysis** — SLIME/Swank provides `who-calls`, `who-binds`, `who-sets`, `who-references` via compiler metadata. mae-scheme could build a call graph during compilation for `:who-calls` / `:who-references` commands.
- [ ] **Type-ranked completion** — scheme-langserver (Chez) ranks completion candidates by type compatibility. Could enhance mae-scheme LSP completion with arity/type hints from call context.
- [ ] **Buffer-source mapping** — SBCL/Swank records source locations referencing editor buffers (not just files), enabling compile-in-place from REPL. mae-scheme could map `eval`-ed code back to the `*scheme-repl*` buffer.
- [ ] **Live recompilation in debugger** — Swank's SLDB allows fixing a function while paused at a breakpoint, then resuming. mae-scheme's `define_global` already supports hot reload; wiring it to the DAP resume flow would complete the picture.

### Near-term: Other
- [ ] **Version compatibility policy**: Semver enforcement on upgrade — protocol version negotiation in state-server (`initialize` params), config schema migration on major bumps, `make install-upgrade` blocking on incompatible major versions (currently warns only). Prerequisite for v1.0.
- [ ] PDF preview (GUI inline rendering via `hayro` pure-Rust rasterizer + midnight mode)
- [ ] Semantic code search (vector embeddings)
- [x] Org ↔ Markdown bidirectional conversion (`:markdown-to-org`, `:org-to-markdown`)

### Phase 12: RAG Pipeline (planned)

- [ ] **Embedding storage**: `sqlite-vec` extension for f32 vectors in KB SQLite. Schema: `node_embeddings(node_id, model, vector BLOB, updated_at)`.
- [ ] **Embedding generation**: Support local models (GGUF/llama.cpp) and API-based (OpenAI, Voyage). `mae-embed` crate or integration in `mae-kb`.
- [ ] **Vector search**: `kb_semantic_search(query, top_k)` MCP tool + `(kb-semantic-search QUERY K)` Scheme fn. Cosine similarity, FTS5 fallback.
- [ ] **Retrieval pipeline**: Before each AI turn, auto-retrieve relevant KB nodes by: buffer context, semantic similarity, explicit references. Budget: `rag_max_context_tokens` option (default 2048).
- [ ] **Context injection**: Retrieved nodes as structured `<context>` blocks in system prompt. Dedup, TTL cache (5 min).
- [ ] **Incremental re-embedding**: `kb-reindex` command, background task, status bar progress.
- [ ] **Multi-source indexing**: Code files (tree-sitter chunked), docs (section chunked), git history (recent commits).

### AI Harness & Per-Model Tuning (planned)

- [ ] **Model profiles**: `ModelProfile` type — max tokens, cache control, tool reliability, prompt style. Stored in `~/.config/mae/models.toml`. Built-in defaults for Claude family, GPT-4o/4.1, Gemini 2.5, DeepSeek V3/R1.
- [ ] **Prompt template engine**: Template files in `~/.config/mae/prompts/` with variables (`{buffer_name}`, `{language}`, `{tools}`, `{context_budget}`). Per-model overrides.
- [ ] **Tool tier selection**: Core (15 tools) / Extended (50) / Full (450+). Auto-selected by model reliability score. User-overridable via `ai_tool_tier` option.
- [ ] **Capability detection**: Auto-run `model_exam` on first use. Cache in `~/.local/share/mae/model-capabilities.json`. Drive tool tier + prompt style.
- [ ] **Prompt A/B harness**: `mae --prompt-eval` mode — standardized coding tasks x models x configs. Outputs comparison table (accuracy, tokens, latency).
- [ ] **Per-model tokenizer**: tiktoken (OpenAI), anthropic tokenizer (Claude) for accurate budget math. Character fallback for unknown models.
- [ ] **Graceful degradation**: Circuit breaker -> reduce tool tier -> simplify prompt -> add examples -> surface warning.

### Doom Parity Roadmap: Future Feature Crates

**Tier 1: High-value, self-contained (next 2-3 releases)**

| Crate | Doom Equivalent | Description |
|-------|----------------|-------------|
| `mae-snippets` | `:editor/snippets` | YASnippet-style templates with tab-stops, mirrors, transforms |
| `mae-spell` | `:checkers/spell` | Spellcheck via hunspell/aspell, inline markers, `z=` suggest |
| `mae-format` | `:editor/format` | Formatter bridge (prettier, black, rustfmt) — complements LSP format |
| `mae-lookup` | `:tools/lookup` | Unified lookup: LSP def + docs URL + man + Dash/Devdocs |
| `mae-make` | `:tools/make` | Build runner: detect Makefile/Cargo.toml/package.json, `SPC c b` |

**Tier 2: Language IDE modules (Scheme, not Rust crates)**

| Module | Doom Equivalent | What it configures |
|--------|----------------|-------------------|
| `lang-python` | `:lang/python` | pyright/pylsp, debugpy, virtualenv, REPL |
| `lang-rust` | `:lang/rust` | rust-analyzer, lldb-dap, cargo, test runner |
| `lang-go` | `:lang/go` | gopls, dlv, go commands |
| `lang-javascript` | `:lang/javascript` | ts-ls, chrome-debugger, npm |
| `lang-cc` | `:lang/cc` | clangd, lldb-dap, cmake |

**Tier 3: Editor enhancement crates (future releases)**

| Crate | Doom Equivalent | Description |
|-------|----------------|-------------|
| `mae-dired` | `:emacs/dired` | Buffer-based file manager |
| `mae-undo-tree` | `:emacs/undo` | Undo tree visualization |
| `mae-workspace` | `:ui/workspaces` | Named workspace sessions, per-project layouts |
| `mae-zen` | `:ui/zen` | Distraction-free writing mode |

### AI
- [x] Semantic tool search (`search_tools` — fuzzy match over 146+ tool names/descriptions)
- [x] Dynamic MCP tool discovery (external MCP server connections, `mcp_{server}_{tool}` namespacing)
- [x] `request_tools` accepts specific tool names (completes search→request workflow)
- [x] Memory synthesis (`synthesize_memory()` in bootstrap.rs — categorizes/deduplicates/budgets memory per model)
- [x] Verification specialist (verifier profile, `AiCommand::Delegate`, scoped tools)
- [ ] AI session playback & undo (step-through replay of code changes)
- [x] Network status command (`:ai-ping`, `connectivity_check()` — HTTP HEAD, no LLM round-trip)
- [ ] `:mcp-status` / `:mcp-reconnect` commands (MCP server management UI)

### Org-mode
- [ ] Table formulas (`#+TBLFM:` with Calc-like syntax)
- [ ] Table sorting (alphabetic, numeric, time)
- [ ] Table import/export (CSV/TSV, org ↔ markdown)

### Editor
- [ ] PR comment summaries (auto-summarize changes on PR amend)
- [ ] Free AI-assisted setup (Gemini free tier for first-run guidance)
- [ ] Step-through command execution (inspect AI tool call stdin/stdout)

### Keymap Architecture Migration

> **Goal**: Kernel provides only vi-modal primitives. All leader-key (SPC) bindings move to keymap flavor modules.
>
> 1. Trim `keymaps.rs` to minimal vi: Escape, hjkl, operators, text objects, `:`, search
> 2. Make `keymap-doom` the sole source of the SPC tree
> 3. Ship `keymap-emacs` and `keymap-minimal` flavor modules
> 4. Auto-load the selected `keymap_flavor` module regardless of `(mae!)` declarations
> 5. Expose `(clear-keymap-prefix)` for users who want to override, not just extend
> 6. Group names (`set-group-name`) should come from the keymap flavor module, not the kernel

### Architecture Debt (v0.9.1+)

- [x] **Editor struct field extraction**: ~69 fields after 6 extractions — `CollabState` (18), `ShellIntents` (12), `ViState` (41), `AiState` (34), `KbContext` (21), `DapContext` (2). Remaining candidate: `LspContext` (7 fields).
- [x] **dispatch/ui.rs split**: Split into dispatch/config.rs, dispatch/terminal.rs, dispatch/project.rs, dispatch/help.rs, dispatch/kb.rs. *(0829dd5)*
- [ ] **Custom theme filesystem loading**: Only bundled themes work. No user theme search path (~/.config/mae/themes/). Emacs, Vim, Helix all support this.
- [ ] **Binding ownership audit**: Every kernel-dispatched command should have a kernel default binding. Module bindings are for module-specific commands or user-facing overrides only.
- [ ] **Ad-hoc solution review**: Thorough code review for hardcoded values, duplicated logic between TUI/GUI, and workarounds that should be proper abstractions — in prep for server-client architecture.
- [ ] **Which-key idle delay**: Wire `which-key-idle-delay` option to event loop timer (default 0ms = immediate).
- [ ] **Which-key floating popup mode**: Option to render which-key as a centered floating popup (like find-file/command-palette) instead of docked to bottom. Controlled by a `which-key-display` option (`docked` | `floating`).
- [ ] **Scheme configurability audit**: Audit ALL OptionRegistry entries for missing `config_key` (prevents `:set-save` persistence). Verify every option round-trips through config.toml. Document full option surface in `:help concept:options` KB node.
- [x] **Performance regression testing**: Criterion benchmark suite for buffer_ops + crdt_ops. `make bench/bench-save/bench-compare`. *(0829dd5)*
- [ ] **KB search scoping**: Allow per-project KB search that excludes MAE internal nodes (scheme:*, cmd:*, option:*). Add `kb_search_scope` option: `"all"` (default), `"user"` (exclude internal), `"project"` (only project-registered KBs). AI tools respect scope; `:help` always searches all.
- [ ] **KB node visibility**: Add `visibility` property to nodes: `public` (default), `internal` (MAE system nodes), `private` (user personal notes). Internal nodes hidden from user-facing search unless explicitly queried with `:help` or `kb_get` by ID.
- [ ] **Per-workspace KB isolation**: When multiple projects are open, `kb_search` defaults to the active project's registered KB instances. Cross-project search available via `kb_search --all` or `(kb-search-all query)` Scheme API.
- [ ] **KB tangle pipeline**: `make docs-tangle` extracts ADR markdown from KB concept nodes. CI job validates freshness (same as code-map pattern). Enables KB as single source of truth for architecture docs.
- [ ] **Checkbox toggle in KB view mode**: Allow toggling checkboxes in read-only help/KB buffers without entering edit mode. Requires refactoring view-mode to allow targeted mutations.
- [ ] **Replace mode (R)**: Standard vim replace mode where keystrokes overwrite characters.
- [ ] **Doc store eviction TOCTOU**: Between identifying eviction candidates (read lock) and evicting (write lock), a client could reconnect. Low probability; fix requires holding write lock during entire eviction.
- [ ] **Unified buffer-switching strategy**: Three patterns exist (`switch_to_buffer`, `display_buffer_and_focus`, palette). Should converge on one with consistent view state management.
- [ ] **KB fuzzy body search**: `kb_search` currently matches node titles and tags via FTS5 but not node body content in a fuzzy/substring way. Searching for a term like "DeltaDB" that only appears in the body of some nodes returns no results. Add full-text indexing of node bodies (FTS5 `content` column) so `kb_search` and `:help` fuzzy completion can find concepts mentioned anywhere in the knowledge graph, not just in titles.

---

## Collab Data Lifecycle (Future)

Items E1–E8 track open design questions and planned improvements for the collaborative editing data model. All are `Future` / `Planned` — none are committed to a specific release yet.

- [x] **E1. Git-based project identity for collab** *(Complete — b8d4b6a)*
  4-tier identity: git remote → `.project` name → directory basename → FNV-1a hash. `compute_doc_address()` uses `git remote get-url origin` → normalize → FNV-1a. Persistent across sessions (unique in the industry).

- [ ] **E2. KB sync model** *(Future)*
  KB notes (`DocAddress::KbNode`) shared between peers via yrs docs on state server. Conflict resolution for bidirectional link graph.

- [ ] **E3. Directory creation policy for collab saves** *(Future)*
  `collab_create_parent_dirs` option (default: false) — auto-create missing parent dirs on `:saveas`. Safety: prompt before creating directories.

- [ ] **E4. Collab save conflict detection** *(Planned)*
  Two clients both `:w` to shared filesystem path simultaneously. Advisory lock system + content-hash verification.

- [ ] **E5. File-change notification for collab** *(Future)*
  When Bob saves locally, notify Alice via `file-changed-on-disk` hook + inotify.

- [x] **E6a. KB sharing end-to-end** *(v0.11.0)*
  - KB↔CRDT bridge: `node_to_crdt`/`crdt_to_node` in mae-kb ✅
  - Intent wiring: `drain_collab_intents` handles ShareKb/JoinKb/LeaveKb/KbNodeUpdate ✅
  - Event wiring: `handle_collab_event` handles KbShared/KbJoined/KbLeft/KbNodeUpdate ✅
  - Continuous sync: shared_kbs tracking, on_save mode, CRDT update generation ✅
  - Server handler fixes: scoped join (collection manifest), scoped leave ✅
  - Offline queue: pending_kb_updates accumulate while disconnected, drain on reconnect ✅
  - Status line: `[KB:N|synced/offline/pending]` indicator ✅
  - mDNS discovery: `_mae-sync._tcp.local` register/browse via mdns-sd ✅
  - 8 E2E TCP tests, 8 continuous sync tests, 3 offline tests, 5 status tests ✅
  - `collab_kb_sync_mode` option: "manual" | "on_save" ✅

- [ ] **E6b. Peer-to-Peer collaborative editing** *(Future)*
  - P2P-LAN: mDNS discovery + symmetric TCP. Transport layer already generic (`AsyncWrite`/`AsyncBufRead`). mDNS module implemented ✅
  - P2P-KB: KB node replication ✅, link graph merge (future)
  - P2P-Internet: WebRTC/QUIC NAT traversal
  - P2P-E2E: End-to-end encryption (Noise protocol)
  - Remaining: wire mDNS into collab-start/collab-discover commands, WebRTC, E2E encryption

- [ ] **E7. Operation-based version control** *(Future)*
  Inspired by Zed DeltaDB ($32M Series B) — every keystroke tracked, character-level permalinks. yrs already stores operations; annotate with timestamp/user_id/commit message. Timeline scrubber UI showing who changed what.

- [x] **E8. Collab buffer status indicators** *(8de53b8)*
  - Visual distinction for pathless vs mapped collab buffers in status bar
  - Show sync state (in-sync, pending, disconnected) per buffer
  - Show peer count

---

## Completed Features (collapsed)

<details>
<summary>Phase 3 details — AI Integration + Editor Essentials (v0.3–v0.4.1)</summary>

- Tool-calling transport (Claude, OpenAI, Gemini, DeepSeek APIs)
- 450+ commands mapped to AI tool definitions
- Conversation buffer with streaming, tool call display
- Permission tiers (ReadOnly/Write/Shell/Privileged)
- Full vi modal editing: visual mode, text objects, marks, macros, dot-repeat
- Multi-file AI tools, conversation persistence
- Editor.rs split into 9 submodules, AI security (blocklist, circuit breaker)
- Registers, clipboard, vim-surround, command palette
- Second modularization pass (6 god files → module directories)

</details>

<details>
<summary>Phase 5 details — AI Reliability (v0.5.0)</summary>

- Progress checkpoint system (semantic stagnation detection)
- Softened oscillation detector (warn-then-abort)
- Watchdog recovery (cancel AI on prolonged stall)
- Prompt caching (Claude cache_control, OpenAI cached_prompt_tokens)
- Token budget dashboard (cache hit rate, context utilization)
- Context compaction (extractive summarization before hard trimming)
- Graceful degradation (auto-shed tools at >85%/92% context pressure)
- Web fetch tool, ANSI-only themes, XDG transcript logging

</details>

<details>
<summary>Phase 6 details — Embedded Shell + MCP</summary>

- Terminal emulator via `alacritty_terminal` (full VT100, colors)
- ShellInsert mode, Ctrl-\ Ctrl-n exit, process lifecycle
- MCP bridge: Unix socket server, JSON-RPC, stdio shim
- File auto-reload: mtime tracking, dirty buffer warning
- Send-to-shell: `SPC e s` (line), `SPC e S` (region)

</details>

<details>
<summary>Phase 8 details — GUI Backend (v0.6.0–v0.7.0)</summary>

- winit + skia-safe rendering, full keyboard/mouse input
- Pixel-based variable-height lines, cached lazy theme resolution
- Native SVG via `skia_safe::svg::Dom`
- Code folding, file tree sidebar, Magit-style git status
- Org/Markdown structural editing (cycle, promote/demote, narrow/widen)
- Autosave, swap files, crash recovery
- Multi-cursor, inline markup rendering, display overlays
- Desktop launcher (.desktop + SVG icon)
- Large document performance (binary search, graceful degradation)
- Per-phase render timing, frame profiling, scroll stress benchmarks

</details>

<details>
<summary>Phase 9–10 details — Babel, Export, Agent Efficiency</summary>

- 12-language babel executor, noweb expansion, tangle directive
- HTML/Markdown export with TOC, syntax highlighting, tag filtering
- Tiered prompt system (Full/Compact), provider-aware hints
- AI target dispatch (`save-excursion` pattern)
- Org table editing (align, navigate, insert/delete row/column)
- LSP+DAP polish: rename, format, symbol outline, breadcrumbs, watch expressions

</details>

<details>
<summary>Phase 11 details — Module System + KB Federation (v0.9.0)</summary>

- 9 modules extracted: dashboard, surround, marks-jumps, search, registers, macros, tables, multicursor, file-tree
- Doom Emacs-inspired three-file model: `module.toml`, `autoloads.scm`, `init.scm`
- `mae pkg` CLI: list, doctor, info, create, sync, upgrade
- Module flags (`+flag` syntax), dependency resolution, live reload
- `describe-module`, `describe-mode`, `describe-bindings` introspection commands
- KB federation fully wired: `:kb-register`, `:kb-unregister`, `:kb-reimport`, `:kb-instances`
- Recursive org-dir import with health metrics (orphans, broken links, namespaces)
- Federated search across local KB + N external instances
- AI tools: `kb_register`, `kb_unregister`, `kb_reimport` with structured JSON responses
- Registry persistence (`~/.config/mae/kb-registry.toml`), startup auto-load
- KB documentation: `concept:kb-federation`, `concept:kb-workflows`, `concept:kb-vs-alternatives`
- Tutorial: `lesson:kb-import-roam` (Lesson 13)
- Self-test categories: `modules`, `federation`
- Session detach/resume (tmux-style): persist editor state, reconnect from another terminal
- Shared P2P sessions with focus handoff: collaborative cursor, presence indicators
- Granular KB connection/search configuration: users can select/deselect which KB instances are active by default, run scoped queries across a subset of KBs, AI tool parity (e.g. `kb_search` accepts optional `instances` filter param)

</details>

---

## Architecture Invariants

These are non-negotiable constraints derived from Emacs git history analysis:

1. **No file exceeds 3,500 lines** — Emacs's `xdisp.c` is 38,605 lines. We enforce module splits. (Exception: `window.rs` at ~3,434 lines — variable-height line math and smooth scrolling justify the size. Under active monitoring.)
2. **No Global Interpreter Lock** — Emacs spent 23,901 commits on GC retrofit. We use Rust ownership.
3. **AI is a peer, not a plugin** — same `dispatch_builtin()` for human and AI.
4. **Module boundaries enable distributed ownership** — 11 crates with clear responsibilities.
5. **Runtime redefinability is sacred** — Scheme `defadvice`, live REPL, hot reload.
