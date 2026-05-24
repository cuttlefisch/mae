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
│  │ state-server │◄────────────│ client-a (test_share.scm)│      │
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

## Test Scenarios

### Scenario 1: Share + Join (client-a / client-b)

**Goal**: Validate bidirectional CRDT sync between a sharer and joiner.

| Step | Container | Action | Validation |
|------|-----------|--------|------------|
| 1 | client-a | Connect to state server | `(collab-status)` returns pair |
| 2 | client-a | Create + open `/workspace/test.txt` | File exists on disk |
| 3 | client-a | Insert "Hello from Client A\n", save | Buffer contains text |
| 4 | client-a | `:collab-share` | Sync enabled on buffer |
| 5 | client-a | Write `/sync/a-shared` signal | — |
| 6 | client-b | Wait 15s, then `:collab-join test.txt` | Buffer created with A's content |
| 7 | client-b | Insert "Hello from Client B\n" | Edit syncs to A via CRDT |
| 8 | client-a | After 30s sleep, verify B's text arrived | `string-contains? "Hello from Client B"` |
| 9 | client-a | Verify no content duplication | No doubled "Hello from Client A" |
| 10 | both | `:save` / `:saveas` to local + shared volumes | Files on disk |

**Verifier checks** (verify.sh):
- `/workspace-a/test.txt` contains both A and B content
- `/workspace-b/test.txt` contains both A and B content
- `/shared-workspace/test.txt` contains both A and B content

### Scenario 2: Per-User CRDT Undo (undo-sharer / undo-joiner)

**Goal**: Validate that undo/redo are per-user (yrs UndoManager) — A's undo
doesn't affect B's edits, and vice versa.

| Step | Container | Action | Validation |
|------|-----------|--------|------------|
| 1 | undo-sharer | Create + share `/workspace/undo-test.txt` with "base\n" | Sync active |
| 2 | undo-sharer | Insert "from-A\n", signal `/sync/a-edit-done` | — |
| 3 | undo-joiner | Wait for signal, join, verify A's content | Has "base" + "from-A" |
| 4 | undo-joiner | Insert "from-B\n", signal `/sync/b-edit-done` | — |
| 5 | undo-sharer | After 30s, verify B's edit arrived | Has "from-B" |
| 6 | undo-sharer | `:undo` — undoes only A's "from-A" | Has "base" + "from-B", NOT "from-A" |
| 7 | undo-sharer | Signal `/sync/a-undo-done` | — |
| 8 | undo-joiner | After 20s, verify A's undo propagated | Has "base" + "from-B", NOT "from-A" |
| 9 | undo-joiner | `:undo` — undoes only B's "from-B" | Has "base" only |
| 10 | undo-joiner | Save via `:saveas /workspace/undo-test.txt` | — |
| 11 | undo-sharer | After 15s, `:redo` — restores A's "from-A" | Has "base" + "from-A", NOT "from-B" |
| 12 | undo-sharer | Save, signal `/sync/a-all-done` | — |

**Verifier checks** (verify.sh):
- `/workspace-undo-a/undo-test.txt` contains "base" + "from-A"
- `/workspace-undo-b/undo-test.txt` contains "base"

## Coordination Mechanism

Tests use **file-based signaling** via a shared `/sync` volume. Each signal
file acts as a gate:

| Signal File | Writer | Reader(s) | Purpose |
|-------------|--------|-----------|---------|
| `/sync/a-shared` | client-a | client-b | A has shared the doc |
| `/sync/a-saved-shared` | client-a | client-b | A saved to shared volume |
| `/sync/a-edit-done` | undo-sharer | undo-joiner | A finished its initial edit |
| `/sync/b-edit-done` | undo-joiner | undo-sharer | B finished its edit |
| `/sync/a-undo-done` | undo-sharer | undo-joiner | A undid its edit |
| `/sync/a-all-done` | undo-sharer | undo-joiner, client-a, client-b | All undo tests complete |
| `/sync/client-a-done` | client-a | — | client-a exited cleanly |
| `/sync/client-b-done` | client-b | — | client-b exited cleanly |

**Important**: `sleep-ms` is the primary coordination mechanism, NOT
`wait-for-file`. The Scheme test runner processes `sleep-ms` between test
steps and drains collab events during the sleep. `wait-for-file` uses
`wait-until` which polls inside a single eval — it does NOT drain collab
events between polls.

## Container Lifecycle

```
Timeline:
  0s   state-server starts, healthcheck passes
  5s   all 4 clients connect
  ~10s client-a shares test.txt
  ~15s undo-sharer shares undo-test.txt, inserts from-A
  ~20s client-b joins test.txt, undo-joiner joins undo-test.txt
  ~25s client-b edits, undo-joiner edits
  ~30s client-a verifies B's edit
  ~35s undo-sharer verifies B's edit, undoes
  ~40s undo-joiner verifies undo, undoes its own
  ~45s undo-sharer redoes, saves, signals a-all-done
  ~55s undo-joiner sees signal, exits
  ~55s client-a/b see signal, exit
  ~60s verifier starts (depends_on: service_completed_successfully)
  ~61s verifier checks all volumes, exits
  ~62s docker compose down --volumes
```

## Orchestration

The Makefile target `docker-collab-test` uses `docker compose wait` (Compose v2.21+):

```makefile
docker compose up --build -d            # start all services detached
docker compose wait verifier            # block until verifier exits
docker compose logs --no-log-prefix     # dump all logs
docker compose down --volumes           # tear down
```

We avoid `--abort-on-container-exit` because it kills slow containers
before the verifier (which `depends_on: service_completed_successfully`)
can start. Instead, each test container exits naturally when done, and
the verifier starts only after all 4 test containers exit with code 0.

## Flakiness Mitigations

| Risk | Mitigation |
|------|------------|
| Timing: B joins before A shares | B uses 15s static sleep; A shares at ~10s |
| Timing: A checks before B's edit arrives | A uses 30s sleep while draining collab events |
| Cross-client crosstalk | Client-side `shared_docs` filter (bridge ignores unsubscribed doc updates) |
| ForceSync destroys undo | Bridge uses `apply_sync_update` (merge) for existing synced buffers |
| Buffer focus stolen | `BufferJoined` only switches focus for new buffers, not resync |
| Container exits prematurely | Undo-joiner waits 25s for sharer; client-a/b signal done immediately |
| WAL seq gap false positives | Server `broadcast_except` + client-side gap detection coexist safely |

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
| "from-B" not found in sharer | Crosstalk: sharer received unsubscribed doc update, switched buffer |
| Redo produces empty result | ForceSync replaced TextSync, wiping UndoManager |
| Test hangs indefinitely | Signal file not written; previous container crashed |
| Verifier never starts | A container exited non-zero; check `docker compose logs <container>` |

## Files

| File | Purpose |
|------|---------|
| `test_share.scm` | Client A: create, share, verify B's edits, save |
| `test_join.scm` | Client B: join, edit, verify convergence, save |
| `test_undo_sharer.scm` | Client A: share, edit, undo, redo, verify isolation |
| `test_undo_joiner.scm` | Client B: join, edit, verify A's undo, undo own |
| `verify.sh` | Final on-disk file content checks |
| `test_smoke.scm` | Single-client smoke test (not in Docker suite) |
| `test_bidir.scm` | Bidirectional sync test (not in Docker suite) |
| `test_rejoin.scm` | Rejoin after disconnect (not in Docker suite) |
| `test_replica.scm` | Replica convergence (not in Docker suite) |
