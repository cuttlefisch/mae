# `collabtest` — KB test fixture

A tiny, **throwaway** knowledge base (3 org nodes) used as a committed test
fixture for the trusted-peer **KB replication & per-KB membership** feature
(ADR-017). It carries no real user data, so it is safe to commit, share between
machines, and replicate to a peer during testing.

## Nodes

| ID | Kind | Sentinel | Links |
|----|------|----------|-------|
| `collabtest:overview` | concept | `ZEPHYRINE` | → alpha, → beta |
| `collabtest:alpha`    | note    | `QUOKKA`    | → overview, → beta |
| `collabtest:beta`     | note    | `NARWHAL`   | → overview, → alpha |

Each node carries a **unique sentinel token** (`ZEPHYRINE` / `QUOKKA` /
`NARWHAL`) that appears nowhere else in the tree or the codebase, so a test can
assert that the KB actually replicated to a peer — e.g. after a peer joins,
`kb-search "ZEPHYRINE"` must resolve to `collabtest:overview` — rather than
matching incidental content.

## Usage

Ingest into the active/primary KB:

```
:kb-ingest <repo>/tests/fixtures/kb/collabtest
```

…or register as a separate named instance (keeps it out of your real KB):

```
:kb-register collabtest <repo>/tests/fixtures/kb/collabtest
```

Consumers:
- `scripts/collab-membership-e2e.sh` — alice ingests this fixture, shares the
  (now non-empty) KB, and the per-KB membership flow (deny → add → allow) runs
  against real content.

Validation: the `mae --test` runtime does **not** register the KB query layer
(`kb-search`/`kb-health` are unavailable there — the whole `tests/kb-lifecycle`
suite is orphaned for the same reason), so the fixture is validated through the
membership e2e above and via MCP on a live editor:

```
:kb-register collabtest <repo>/tests/fixtures/kb/collabtest   # → 3 nodes
# then: kb_instances → "collabtest: 3 nodes"; kb_search "ZEPHYRINE" → collabtest:overview
```
