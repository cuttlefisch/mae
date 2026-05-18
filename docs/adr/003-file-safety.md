# ADR-003: Multi-Editor File Safety Protocol

**Status**: Accepted
**Date**: 2026-05-16
**KB Source**: `concept:adr-file-safety`

## Context

When multiple AI-assisted editors (MAE, VS Code+Copilot, Cursor, aider) operate
on the same project simultaneously, several failure modes arise:

1. **Write-write conflicts**: Two editors save to the same file within the same
   second — mtime comparison can't detect the race.
2. **Stale LSP state**: External file changes invalidate LSP caches.
3. **Watcher storms**: inotify fires for every save, triggering cascading
   reloads across editors.
4. **Lock contention**: Advisory locks from different editors don't interoperate.
5. **Undo divergence**: Local undo stacks become invalid after external edits.
6. **Git index races**: Multiple editors running `git add` simultaneously corrupt
   the index.

## Decision

Layered defense with four tiers, each catching failures the previous misses:

### Layer 1: Content-Hash Verification (SHA-256)

On every file load and save, compute SHA-256 of the content. Before save, re-read
the file and compare hashes. If different from stored hash AND buffer is dirty,
warn the user about external modification.

**Catches**: Sub-second edits, NFS clock skew, container time drift.
**Cost**: ~10ms for 1MB file.
**Implementation**: `content_hash: Option<String>` field on `Buffer`,
`compute_content_hash()` in `buffer.rs`.

### Layer 2: Advisory File Locks

When MAE opens a file, create `.{filename}.mae.lock` alongside it containing
`{pid, hostname, timestamp}` as JSON. On save, verify lock is still ours. On
close, remove lock. On open, if lock exists and PID is dead (check `/proc/{pid}`),
remove stale lock.

**Catches**: MAE-MAE conflicts (multiple MAE instances on same project).
**Limitation**: Other editors ignore `.mae.lock` — that's Layer 1's job.
**Implementation**: `crates/core/src/file_lock.rs`

### Layer 3: inotify External Change Detection

Existing `notify` crate infrastructure watches open files. When external
modification is detected, warn user with Reload/Ignore options. Pause AI
operations on affected buffers until resolved.

**Catches**: Real-time detection of external edits (< 50ms on Linux).
**Limitation**: Platform-specific (inotify on Linux, FSEvents on macOS).
**Status**: Already implemented for KB files in `crates/kb/src/watch.rs`.

### Layer 4: Git Worktree Isolation

For multi-AI workflows, each agent works in its own git worktree
(`git worktree add`). No file contention within a worktree. Merge at
completion time.

**Catches**: All file-level contention for AI-only workflows.
**Cost**: Storage (one full copy per agent), git overhead.
**Status**: Recommended practice, not enforced by MAE.

## Failure Mode Registry

| System | Failure | Root Cause | MAE Layer |
|--------|---------|-----------|-----------|
| VS Code Remote | File lock invisible to local editors | Platform-specific locks | Layer 1 (hash) |
| Emacs server.el | Single-user only | No concurrent buffer access | Layer 2 (locks) |
| NFS/CIFS | Stale locks after crash | No cleanup on network FS | Layer 2 (PID check) |
| IntelliJ | False positive reloads | Mtime granularity (1s) | Layer 1 (hash) |
| Atom Teletype | Full-doc transfer on reconnect | No incremental sync | Layer 3 (watch) |

## Consequences

- `.mae.lock` files will appear in project directories. Added to default
  `.gitignore` template.
- Content-hash computation adds ~10ms overhead per save for large files.
  Negligible for typical source files (<100KB).
- Advisory locks are best-effort — they don't prevent other editors from
  writing, but they prevent data loss between MAE instances.
- Git worktree isolation is the recommended workflow for multi-AI setups.

## References

- VS Code: hash + debounce (1s default)
- IntelliJ: mtime + size + FSEvents
- Emacs: `#lockfile` (Emacs-style lock files)
