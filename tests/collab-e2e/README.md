# Collab E2E Test Suite

Docker-based end-to-end tests for MAE's collaborative editing features.
Validates CRDT sync, per-user undo/redo, and file convergence across
multiple editor instances connected via the state server.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Docker Compose Network                       │
│                                                                  │
│  ┌──────────────┐  TCP:9473   ┌──────────────────────────┐      │
│  │    daemon    │◄────────────│ client-a (test_share.scm)│      │
│  │              │◄────────────│ client-b (test_join.scm) │      │
│  │              │◄────────────│ undo-sharer              │      │
│  │              │◄────────────│ undo-joiner              │      │
│  └──────────────┘             └──────────────────────────┘      │
│                                        │                         │
│                               ┌────────▼────────┐               │
│                               │    /sync volume   │  file-based  │
│                               │  (coordination)   │  signaling   │
│                               └─────────────────┘               │
│                                        │                         │
│                               ┌────────▼────────┐               │
│                               │    verifier      │  checks all   │
│                               │  (verify.sh)     │  workspace    │
│                               │                  │  volumes      │
│                               └─────────────────┘               │
└─────────────────────────────────────────────────────────────────┘
```

## Sync Strategy: Content-Based Barriers

**The #1 design principle: never use `sleep-ms` to wait for CRDT convergence.**

Instead, all CRDT-dependent assertions use **content-based barriers**:

| Barrier | Purpose |
|---------|---------|
| `(wait-for-content BUF SUBSTR TIMEOUT)` | Poll until buffer contains expected text |
| `(wait-content-absent BUF SUBSTR TIMEOUT)` | Poll until buffer does NOT contain text |
| `(wait-synced BUF TIMEOUT)` | Poll until buffer is in synced-buffers list |
| `(wait-connected TIMEOUT)` | Poll until collab status is connected/synced |
| `(wait-buffer-exists BUF TIMEOUT)` | Poll until buffer exists (after join) |
| `(wait-for-file PATH TIMEOUT)` | Poll until a coordination file appears |

All barriers use `wait-until`, which calls `sleep-ms 50` between polls. The
test runner's `eval_with_yields` drains collab events during every `sleep-ms`
yield — so CRDT updates are applied between each poll. This creates a tight
observe→drain→check loop that returns as soon as the expected state is reached.

**File signals** (`/sync/a-shared`, `/sync/b-edit-done`, etc.) coordinate
*sequencing* between containers — they say "my step is done, proceed." But they
do NOT guarantee CRDT convergence. The receiving client always follows a file
signal with a content barrier before asserting on buffer contents.

### Why this works

```
Client A: buffer-insert "from-A" → CRDT tx generated → sync/update sent to server
Client B: wait-for-content "from-A" →
  poll 1: buffer-text → "base\n"          → sleep-ms 50 (drains collab events)
  poll 2: buffer-text → "base\n"          → sleep-ms 50 (CRDT update arrives, applied)
  poll 3: buffer-text → "base\nfrom-A\n"  → ✓ return
```

The key insight: `sleep-ms` yields to the event loop, which calls
`drain_collab_events()` → `handle_collab_event()` → CRDT update applied to
buffer. So each poll cycle both checks content AND processes pending network
events.

## Test Scenarios

### Scenario 1: Share + Join (client-a / client-b)

**Goal**: Validate bidirectional CRDT sync between a sharer and joiner.

| Step | Container | Action | Barrier |
|------|-----------|--------|---------|
| 1 | client-a | Connect | `wait-connected 30000` |
| 2 | client-a | Create + save test.txt | — |
| 3 | client-a | `collab-share` | `wait-synced "test.txt" 15000` |
| 4 | client-a | Signal `/sync/a-shared` | — |
| 5 | client-b | Wait for A's signal | `wait-for-file` |
| 6 | client-b | `collab-join test.txt` | `wait-buffer-exists "test.txt" 30000` |
| 7 | client-b | Verify A's content | `wait-for-content "test.txt" "Hello from Client A" 30000` |
| 8 | client-b | Insert "Hello from Client B" | — |
| 9 | client-a | Verify B's content | `wait-for-content "test.txt" "Hello from Client B" 60000` |
| 10 | both | Save to local + shared volumes | — |

### Scenario 2: Per-User CRDT Undo (undo-sharer / undo-joiner)

**Goal**: Validate per-user undo isolation (yrs UndoManager).

| Step | Container | Action | Barrier |
|------|-----------|--------|---------|
| 1 | undo-sharer | Share + insert "from-A" | `wait-synced` |
| 2 | undo-joiner | Join + verify A's content | `wait-for-content "from-A"` |
| 3 | undo-joiner | Insert "from-B" | — |
| 4 | undo-sharer | Verify B's content | `wait-for-content "from-B"` |
| 5 | undo-sharer | Undo (removes from-A only) | `wait-content-absent "from-A"` |
| 6 | undo-joiner | Verify A's undo propagated | `wait-content-absent "from-A"` |
| 7 | undo-joiner | Undo (removes from-B only) | `wait-content-absent "from-B"` |
| 8 | undo-sharer | Redo (restores from-A) | `wait-for-content "from-A"` |
| 9 | undo-sharer | Verify B's undo propagated | `wait-content-absent "from-B"` |

## Coordination Signals

| Signal File | Writer | Reader(s) | Purpose |
|-------------|--------|-----------|---------|
| `/sync/a-shared` | client-a | client-b | A has shared the doc |
| `/sync/a-saved-shared` | client-a | client-b | A saved to shared volume |
| `/sync/a-edit-done` | undo-sharer | undo-joiner | A finished its initial edit |
| `/sync/b-edit-done` | undo-joiner | undo-sharer | B finished its edit |
| `/sync/a-undo-done` | undo-sharer | undo-joiner | A undid its edit |
| `/sync/b-undo-done` | undo-joiner | undo-sharer | B undid its edit |
| `/sync/a-all-done` | undo-sharer | undo-joiner | All undo tests complete |
| `/sync/client-a-done` | client-a | — | client-a exited cleanly |
| `/sync/client-b-done` | client-b | — | client-b exited cleanly |

**Critical**: File signals coordinate *sequencing* only. They do NOT replace
content barriers. Every client must `wait-for-content` or `wait-content-absent`
before asserting on buffer contents after a CRDT-dependent step.

## Container Lifecycle

All timing is dominated by content barriers, not fixed sleeps:

```
Timeline (approximate — barriers make exact timing variable):
  0s   daemon starts, healthcheck passes
  ~3s  all 4 clients connect (wait-connected)
  ~5s  client-a shares test.txt (wait-synced)
  ~5s  undo-sharer shares undo-test.txt (wait-synced)
  ~8s  client-b joins test.txt (wait-buffer-exists + wait-for-content)
  ~8s  undo-joiner joins undo-test.txt (wait-buffer-exists + wait-for-content)
  ~10s client-b edits, undo-joiner edits
  ~12s client-a sees B's edit (wait-for-content), saves
  ~12s undo-sharer sees B's edit (wait-for-content), undoes
  ~15s undo-joiner sees undo (wait-content-absent), undoes its own
  ~18s undo-sharer redoes, waits for B's undo (wait-content-absent), saves
  ~20s all clients exit
  ~21s verifier checks all volumes
  ~22s docker compose down
```

## Running

```bash
make docker-collab-test     # full Docker E2E suite
```

## Debugging

### Enable verbose logging

In `docker-compose.collab-test.yml`, change `MAE_LOG` to:
```
MAE_LOG: "mae::collab_bridge=trace,mae::test_runner=debug,info"
```

### Run a single scenario

Comment out unused services in the compose file, or run directly:
```bash
docker compose -f docker-compose.collab-test.yml run --rm undo-sharer
```

### Test runner diagnostics

On test failure, the runner dumps:
- Active buffer name, text length, text preview
- All buffers: name, text_len, sync state, collab_doc_id

### Common failure patterns

| Symptom | Likely Cause |
|---------|-------------|
| wait-for-content timeout | CRDT update not propagating — check daemon logs |
| wait-content-absent timeout | Undo not generating CRDT update — check UndoManager setup |
| wait-synced timeout | Share intent not reaching bridge — check drain_collab_intents |
| Buffer not found after join | Join intent lost — check collab_bridge join handler |
| Verifier file check fails | Buffer content correct but save didn't flush — check write-file |

## Files

| File | Purpose |
|------|---------|
| `lib/test-helpers.scm` | Content-barrier helpers (wait-for-content, wait-synced, etc.) |
| `test_share.scm` | Client A: create, share, verify B's edits, save |
| `test_join.scm` | Client B: join, edit, verify convergence, save |
| `test_undo_sharer.scm` | Client A: share, edit, undo, redo, verify isolation |
| `test_undo_joiner.scm` | Client B: join, edit, verify A's undo, undo own |
| `verify.sh` | Final on-disk file content checks |
| `test_smoke.scm` | Single-client smoke test (not in Docker suite) |
| `test_bidir.scm` | Bidirectional sync test (not in Docker suite) |
| `test_rejoin.scm` | Rejoin after disconnect (not in Docker suite) |
| `test_replica.scm` | Replica convergence (not in Docker suite) |
