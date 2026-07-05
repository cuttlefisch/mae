# Multi-Instance KB (daemon-less & daemon)

How MAE lets several editor instances share one knowledge base **safely**, in both
the daemon-less and daemon-hosted configurations. This is the local, single-machine
story; cross-machine collaboration over a daemon mesh is `KB_SHARING.md`.

## The model

The durable **CozoDB store is the source of truth**. Each editor process keeps an
in-memory `KnowledgeBase` **mirror** loaded from that store at startup (in the
background, off the UI thread — Phase 1a) and used to serve most reads. Writes go
through to the store immediately (write-through), so the store is always current;
the mirror is a cache.

```
 process A  ──write──▶ ┌───────────────┐ ◀──write──  process B
 (mirror A)  ◀─load──  │  primary.cozo │  ──load──▶  (mirror B)
                       │ sqlite (WAL)  │
                       └───────────────┘
```

## Daemon-less: N processes share one file

The primary store uses the cozo **sqlite** backend (option `kb_storage_engine`,
default `sqlite`). Unlike the legacy **sled** backend — which takes an exclusive
directory lock, so a second process boots with no KB — sqlite lets multiple processes
open the same `primary.cozo` file.

- **Write safety.** cozo 0.7's sqlite backend sets no `busy_timeout`, so two
  concurrent writers can transiently collide. `CozoKbStore::run_with_busy_retry`
  retries on the storage-lock error with full-jitter backoff (an application-level
  `busy_timeout`), so concurrent writers converge with no lost writes and no
  corruption. See the adversarial test `sqlite_multi_instance_concurrent_writes_converge`.
- **Cross-instance freshness (Phase 4).** Each process runs a `StoreWatcher`
  (filesystem notify) on `primary.cozo`. When another process commits, the watcher
  fires and the editor reloads its mirror in the background (`drain_kb_store_watch`
  → `drain_kb_preload`). Our own writes are suppressed by a short cooldown so local
  edits don't churn. **Reflected:** external adds + edits, on the next idle tick.
  **Not yet reflected live:** cross-instance *deletes* (they clear on the next full
  reload / restart), and refresh of an *already-rendered* view buffer — re-open or
  re-run the view to see external changes in it.

### Migration (sled → sqlite)

Existing sled stores auto-migrate **once** on the first launch under a sqlite default
(`mae_kb::migrate::migrate_sled_to_sqlite`): the sled data is read, bulk-imported to a
new sqlite store in one transaction (fast — ~1s per few-thousand nodes, links
preserved verbatim), then the sled directory is renamed to `primary.cozo.sled.bak-<ts>`
(never deleted — reversible) and the sqlite file moves into place. Idempotent; on any
failure the intact sled store is opened as-is.

> **Practical note.** With sqlite the multi-instance data is safe, but the migration
> must run with **no other mae holding the old sled store** — quit all instances
> before the first post-upgrade launch.

## Daemon-hosted

When `daemon_mode` opts the daemon into hosting the primary KB, the daemon owns the
store and editors are **thin clients**: reads route through the daemon read layer,
writes flow through the CRDT sync channel, and the daemon broadcasts updates to all
connected editors (live). This is orthogonal to the engine choice above.

Known gap (**#118**): when the daemon hosts the primary, `:kb-agenda` and ranked
search are routed through `query_layer()` (which prefers the daemon), but the daemon
side does not yet answer the agenda/ranked-search RPC — so they degrade to empty under
a thin mirror. Agenda + history already route uniformly through the query layer in the
daemon-**less** path; closing #118 is a daemon-side RPC follow-up.

## Configuration

| Option | Default | Meaning |
|---|---|---|
| `kb_storage_engine` | `sqlite` | `sqlite` (WAL, multi-instance-safe) or `sled` (legacy single-writer). Switching to sqlite auto-migrates. |
| `daemon_mode` | `off` | `off` / `on-demand` / `shared` — whether/how a daemon participates. Independent of the storage engine. |

## Failure surfacing

If the durable store fails to open (a lock held by another sled process, or
corruption), the editor no longer boots a silent empty KB: it flags the store
unavailable, surfaces a status, and KB mutations refuse with an actionable message
(`kb_write_blocked`) rather than writing to a mirror that will never persist.
