# MAE exhausts the per-user inotify instance budget

**Status:** **fixed** in `shared/kb/src/watch.rs` — one watcher per process, many watched paths
(see "The fix" below and the `@ai-caution: [resource-exhaustion]` marker on that module). Reported
by Hayden after Sway began alerting on inotify limits — with the observation that no other software
on the machine, *including heavily-used Emacs*, has ever caused it.

## The measurement

```
max_user_instances:    128        <- inotify_init() file descriptors   (SCARCE)
max_user_watches:  250,324        <- watched paths                     (ABUNDANT)

system-wide in use:    124 / 128
of which MAE-family:    89        <- 70% of the entire per-user budget
```

MAE is not using an unreasonable number of *watches*. It is using an unreasonable number of
*instances* — and instances are the scarce resource by three orders of magnitude.

## The antipattern

`OrgDirWatcher::new` (`shared/kb/src/watch.rs:70`) constructs a fresh
`notify::recommended_watcher()` — one `inotify_init()` — and then watches exactly **one**
directory with it:

```rust
pub fn new(dir: impl AsRef<Path>) -> notify::Result<Self> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res| { let _ = tx.send(res); })?;
    watcher.watch(dir.as_ref(), RecursiveMode::Recursive)?;
    ...
}
```

It is then constructed **per KB instance**, in three independent places:

| Site | Shape |
|---|---|
| `crates/core/src/editor/kb_ops/registry.rs:124` | `self.kb.watchers.insert(uuid, watcher)` — one per registered KB |
| `daemon/src/scheduler.rs:166` | `watchers.insert(uuid.clone(), w)` — same, in the daemon |
| `crates/mae/src/bootstrap.rs:2377,2511` | `StoreWatcher` per store, plus one for the registry |

So a single editor costs roughly **N + 2 instances for N registered KBs**. With six KBs registered
that is ~8 per editor, before counting the daemon (4 observed) and any headless instance (3
observed).

Emacs, for contrast, uses **one** inotify instance and adds many watch descriptors to it. That is
the design `inotify` is built for: `inotify_add_watch` is cheap and plentiful, `inotify_init` is
capped at 128 per user.

## Why it compounds

Each instance is held for the process lifetime, so the cost is multiplied by every concurrently
running MAE — editor, GUI, headless, daemon, plus any test harness. It is also the mechanism behind
a self-amplifying test failure: once instances run out, a freshly spawned headless cannot start its
watchers, never binds its socket, the test times out, and leaks the child — consuming the
instances it did acquire and making the next run likelier to fail.

## The fix

**One watcher per process, many watched paths.** `notify`'s watcher supports repeated `watch()`
calls; nothing about the current design requires a watcher per directory. The per-instance state
(`path_to_ids`, `errors`, the receiver) becomes a routing table keyed by path prefix, so an event
is dispatched to the right KB instance by matching its directory.

As implemented:
- A process-wide `SharedDirWatcher` owning one `RecommendedWatcher` and one receiver, created on
  first use (and re-tried, not memoized, if creation fails — the limit being momentarily full is a
  transient condition caused by *other* processes).
- `OrgDirWatcher` and `StoreWatcher` are now registration handles on it: constructing one calls
  `watch()` on the shared instance, dropping one `unwatch()`es (refcounted, so two KBs on the same
  directory share one watch and the first to drop doesn't blind the second).
- Event dispatch resolves each event path to a registration by **longest-prefix match**
  (component-wise, so `/a/bc` never matches `/a/bcd`), which is what makes a KB registered *inside*
  another KB's directory feed the innermost KB rather than both. Ties — two registrations on the
  same root — both receive, matching the old one-watcher-each behavior.
- `StoreWatcher` (the durable store file) and the registry watcher fold into the same instance: an
  exact file path is always a strictly longer match than any directory containing it, so their
  events can never be misrouted to a KB.

Result, measured on the real code (`inotify_init1` calls, counted with an `LD_PRELOAD` shim so the
measurement works even on the exhausted machine): 8 watched directories cost **8** instances
before, **1** after — and that same single instance also absorbs the store and registry watchers,
i.e. **1 per process regardless of KB count**, down from N+2. `mae_kb::watch::inotify_instance_count()`
is the in-tree oracle; two regression tests (in `mae-kb` and in `mae-core`, the latter through the
real `kb_register` path) assert the invariant against `/proc/self/fd` directly.

## Why this is worth doing properly rather than raising the limit

Raising `fs.inotify.max_user_instances` is a machine-config workaround that hides the defect and
does not travel to other users' machines. The per-user cap exists precisely to stop one application
monopolising a shared kernel resource, and MAE is currently taking 70% of it. A user running MAE
alongside an IDE, a file manager and a desktop shell would starve them — which is what the Sway
alert is.
