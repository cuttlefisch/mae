# ADR-008: CRDT Target Metrics

**Status:** Accepted
**Date:** 2026-05-20
**Context:** ADR-006 (Collaborative State Engine)

## Context

MAE's collaborative editing stack (yrs/YATA, SQLite WAL persistence, TCP sync protocol) is functionally complete (Phases 1-7). However, no documented target metrics exist for performance, resilience, and resource consumption. Without targets, it's impossible to test for regressions or validate production readiness.

This ADR establishes target metrics based on analysis of Notion, Google Docs, VS Code Live Share, Figma, and yrs benchmarks.

## Decision

### Performance Targets

| Metric | Target | Rationale |
|--------|--------|-----------|
| Max concurrent clients/doc | 50 | VS Code Live Share caps at 30; Notion handles 100+; 50 is practical for LAN use |
| Max document size | 10 MB text (~200K lines) | Google Docs: ~1.5M chars; ropey handles 10M+ |
| Max total documents in memory | 1,000 (enforced) | Configured in `SyncConfig` but must be enforced at runtime |
| State vector overhead | <1 KB for 50 clients | 8 bytes/client = 400 bytes at 50 |
| Update propagation latency (LAN) | <50ms p99 | Notion targets ~100ms; LAN should be faster |
| Reconcile throughput | >1 MB/s | `similar` LCS on 10K-line docs takes ~5ms |
| WAL replay recovery time | <5s for 1,000 entries | SQLite sequential read is fast |

### Memory Targets

| Metric | Target | Rationale |
|--------|--------|-----------|
| Memory per document (idle) | <100 KB baseline | yrs doc + ropey mirror + metadata |
| Memory per document (active, 50 clients) | <5 MB | Includes history, state vectors, pending updates |

### Resource Limits (Enforced)

| Limit | Default | Configurable? | Enforcement |
|-------|---------|---------------|-------------|
| Max documents in memory | 1,000 | `sync.max_documents` | Reject `get_or_create` when at capacity |
| Max update payload | 1 MB | `sync.max_update_size_bytes` | Reject before WAL append |
| Max WAL entries between compactions | 5,000 | `storage.max_wal_entries` | Force immediate compaction |
| Max document size (warning) | 10 MB | `sync.max_document_size_bytes` | Log warning (don't reject — breaks convergence) |
| Write timeout | 5s | `collaboration.write_timeout_ms` | Disconnect slow client |
| Consecutive write failures before disconnect | 3 | Named constant | Disconnect poisoned client |

### Persistence Targets

| Metric | Target | Rationale |
|--------|--------|-----------|
| Compaction interval | 60s (configurable) | Already implemented |
| Idle eviction | 300s (configurable) | Already implemented |
| Compaction atomicity | Transaction (snapshot + WAL trim) | Prevents duplicate replay on crash |

## Industry Comparison

| System | Max Clients | Max Doc Size | Latency Target | CRDT/OT |
|--------|-------------|--------------|----------------|---------|
| Notion | 100+ | ~5 MB | ~100ms p50 | CRDT (yjs) |
| Google Docs | 100+ | ~1.5M chars | ~50ms p50 | OT (proprietary) |
| VS Code Live Share | 30 | Unlimited | ~100ms p50 | OT-like |
| Figma | 100+ | ~50 MB canvas | ~16ms (frame) | CRDT |
| Excalidraw | 50+ | ~10 MB | ~50ms p50 | CRDT (yjs) |
| **MAE** | **50** | **10 MB** | **<50ms p99** | **CRDT (yrs)** |

## Consequences

- Runtime enforcement prevents unbounded resource growth
- Target metrics enable regression testing and SLA validation
- Documented limits inform users about system capabilities
- Warning-only for document size preserves CRDT convergence guarantees
