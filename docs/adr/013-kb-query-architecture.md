# ADR-013: KB Query Architecture

**Status:** Accepted
**Date:** 2026-06-04
**Supersedes:** Partial aspects of ADR-011, ADR-012

## Context

MAE's knowledge base had a dual architecture causing data inconsistencies:

1. **In-memory `KnowledgeBase`** (HashMap + body-parsed links): All runtime queries went here. Links discovered by parsing `[[...]]` in bodies — knew nothing about CozoDB typed links.
2. **CozoDB `CozoKbStore`** (Datalog, FTS, typed links, graph algorithms): Write-through only. Rich query capabilities unused at runtime.

**Symptoms:** 39 false orphans in health report (typed links invisible to in-memory orphan detection), typed link labels not rendered in help buffers, FTS/graph algorithms unused, all nodes loaded into memory at startup (`load_all()`) — won't scale past ~20K nodes.

## Decision

### 1. KbQueryLayer Trait

Introduce a read-only query abstraction (`KbQueryLayer`) that all runtime reads go through:

```rust
pub trait KbQueryLayer: Send + Sync {
    fn get(&self, id: &str) -> Option<Node>;
    fn contains(&self, id: &str) -> bool;
    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit>;
    fn links_from(&self, id: &str) -> Vec<Link>;
    fn links_to(&self, id: &str) -> Vec<Link>;
    fn list_ids(&self, prefix: Option<&str>) -> Vec<String>;
    fn id_title_pairs(&self, prefix: Option<&str>) -> Vec<(String, String)>;
    fn health_report(&self) -> Option<HealthReport>;
    fn neighborhood(&self, id: &str, depth: u32) -> Option<SubGraph>;
    fn namespace_prefixes(&self) -> Vec<String>;
}
```

### 2. Three Implementations

| Implementation | Backing Store | Use Case |
|---|---|---|
| `CozoQueryLayer` | `Arc<CozoKbStore>` | Production reads — Datalog, FTS, typed links |
| `FederatedQuery` | Primary + N instance layers | Multi-KB fan-out (manual + user + imported KBs) |
| `InMemoryQueryLayer` | `Mutex<KnowledgeBase>` | Fallback when no CozoDB available (tests, first-run) |

### 3. Query Layer Always Available

`KbContext::query_layer()` returns `Option<&dyn KbQueryLayer>` — `Some` when a CozoDB store is present, `None` otherwise. All read call sites use the pattern:

```rust
if let Some(q) = self.kb.query_layer() {
    q.get(id)  // CozoDB-first path
} else {
    self.kb.primary.get(id).cloned()  // In-memory fallback
}
```

### 4. Typed Links Surface Through Query Layer

`KbQueryLayer::links_from()` and `links_to()` return `Vec<Link>` where `Link` includes `rel_type` (e.g., "teaches", "requires", "implements"). This enables:

- Help buffer neighborhood display with typed labels ("teaches → concept:buffer")
- AI tools returning structured link metadata
- Health reports that count typed links (eliminating false orphans)

### 5. Unified Health Report

`store::HealthReport` is the single health report type (replacing the old dual `KbHealthReport` + `HealthReport`). CozoDB produces accurate reports with typed link awareness. The in-memory fallback still exists for tests.

### 6. NodeCache (LRU)

`NodeCache` (500-node LRU) + `CachedQueryLayer` wrapper available for hot-path optimization. Cache checks on `get()`, CozoDB on miss. Search/links always go to CozoDB.

## Migration

46 call sites across 14 files migrated to use `query_layer()` first:

| File | Sites | Notes |
|---|---|---|
| `help_ops.rs` | 12 | Typed link labels now rendered |
| `kb_ops.rs` | 10 | CRUD reads + search + validation |
| `ai/tool_impls/kb.rs` | 10 | AI tools: get, list, links, graph, search_context |
| `ai/tool_impls/help.rs` | 2 | Help open |
| `ai/tool_impls/introspect.rs` | 1 | Node count |
| `ai/executor/collab_exec.rs` | 1 | KB listing for collab |
| `dispatch/kb.rs` | 1 | Rebuild count |
| `file_ops.rs` | 1 | Help tab-completion |
| `command.rs` | 3 | Help/describe lookups |
| `collab_bridge.rs` | 1 | KB listing |
| `key_handling/normal.rs` | 1 | Describe-key |
| `scheme/runtime.rs` | 1 | Scheme primitives |

Writes (`insert`, `remove`, `upsert_with_crdt`, `get_mut`) remain on `kb.primary` — the in-memory KB is still the write target with CozoDB write-through.

## Future Work

### Phase 4: Remove KnowledgeBase (Deferred)

Making CozoDB the sole data store requires:
- `CozoKbStore::open_mem()` for tests (CozoDB supports `"mem"` engine)
- Moving writes from `KnowledgeBase::insert()` to `KbStore::insert_node()`
- 888 `Editor::new()` test calls would need lightweight CozoDB instances
- `KnowledgeBase` becomes `#[cfg(test)]` or deleted entirely

Deferred because the test infrastructure change is large and the current architecture works correctly with the fallback pattern.

### Phase 7.6: Delete Rust Editorial Constants (Deferred)

3,089 lines of editorial content in `kb_seed/{concepts,lessons,tutorials,keys}.rs` duplicate the 232 org files in `assets/manual/`. Deletion blocked by Phase 4 — tests rely on `seed_kb()` populating the in-memory KB with these constants.

## Consequences

**Positive:**
- Typed links visible in help buffers for the first time
- Health report accuracy improved (0 false orphans with CozoDB)
- Foundation for scaling past 20K nodes (CozoDB handles pagination)
- AI tools return richer link metadata (rel_type)

**Negative:**
- Verbose `if let Some(q) = query_layer()` pattern at every read site
- Two code paths maintained (CozoDB + in-memory fallback) until Phase 4
- No performance improvement yet (both paths active, no LRU cache in use)

**Neutral:**
- `KnowledgeBase` remains in codebase for writes and test fallback
- Binary size unchanged (editorial constants not yet removed)
