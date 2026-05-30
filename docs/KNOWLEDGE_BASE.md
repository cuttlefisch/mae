# Knowledge Base

MAE's knowledge base is a typed graph of nodes with bidirectional links. It serves as both the built-in MAE manual and a personal knowledge graph (org-roam equivalent).

## Architecture

```
┌─────────────────────────────────────────┐
│           MAE Knowledge Base            │
├─────────────┬───────────────────────────┤
│  Local KB   │  Federated Instances      │
│  (seed +    │  ┌─────────┐ ┌─────────┐ │
│   user help │  │RoamNotes│ │ Work KB │ │
│   + AI)     │  │ 2,500   │ │ 87      │ │
│  200+ nodes │  │ nodes   │ │ nodes   │ │
│             │  └─────────┘ └─────────┘ │
└─────────────┴───────────────────────────┘
         ↕ search / get / graph
    ┌─────────────────────┐
    │  :help  │  AI tools │
    │  SPC h  │  kb_*     │
    └─────────────────────┘
```

## Graph Model

- **Typed nodes** — each has an `id`, `title`, `kind`, `body`, `tags`, and optional `source` provenance.
- **Bidirectional links** — `[[id|display text]]` markers in body text. Reverse index provides O(1) `links_to()`.
- **Search** — pre-lowercased substring cache + FTS5 full-text search (porter stemmer + unicode61). Fuzzy fallback when no substring matches.
- **Node kinds**: Index, Command, Concept, Key, Note, Project.
- **Node sources**: Seed (compiled-in), UserOrg (from `~/.config/mae/help/`), Manual, Federation.

## Node Namespaces

| Prefix | Description | Example |
|--------|-------------|---------|
| `index` | Entry page | `index` |
| `cmd:` | One per registered command | `cmd:save`, `cmd:delete-line` |
| `concept:` | Architectural concepts | `concept:buffer`, `concept:ai-as-peer` |
| `key:` | Keybinding summaries | `key:normal-mode`, `key:leader-keys` |
| `option:` | Editor options | `option:line-numbers` |
| `module:` | Module documentation | `module:dashboard` |
| `scheme:` | Scheme API functions | `scheme:buffer-insert` |
| `lesson:` | Interactive tutorials | `lesson:getting-started` |

## Federation

Federation lets you register external org directories as searchable KB instances alongside MAE's built-in manual.

### Design Principle

**The org directory is READ-ONLY for the KB layer. SQLite is derived.**

Your org files remain the canonical source of truth. MAE reads them, builds an in-memory graph, and never writes to your org directory (except one sentinel file: `eor-instance.org`).

### Registry

Stored at `~/.config/mae/kb-registry.toml`. Each instance has:
- UUID (generated or read from sentinel file)
- Name (user-chosen display label)
- Org directory path
- Enabled flag
- Last import timestamp

### Import Pipeline

1. Recursive `walkdir` over the org directory.
2. Parse each `.org` file for `:ID:` properties (file-level + heading-level).
3. Files without `:ID:` are counted but skipped.
4. File-path links (images, attachments) are NOT treated as KB links.
5. Duplicate `:ID:` values are detected and reported.
6. Health metrics (orphans, broken links, namespaces) computed automatically.

### Commands

| Command | Description |
|---------|-------------|
| `:kb-register <name> <path>` | Register and import an org directory |
| `:kb-unregister <name>` | Remove an instance from the registry |
| `:kb-reimport <name>` | Refresh after editing org files |
| `:kb-instances` | List registered instances with node counts |
| `:kb-health` | Health report (orphans, broken links, namespace counts) |

### Link Scheme

- `eor:node-id` — local-first lookup (checks local KB, then instances).
- `eor:uuid/node-id` — targeted lookup in a specific instance.

## Workflows

### Exploration

- `:help <topic>` or `SPC h h` — fuzzy search all KB nodes.
- `SPC h s` — full-text search.
- `SPC n f` / `:kb-find` — search with fuzzy matching.
- Tab/Enter in Help buffer — follow links, navigate graph.
- `C-o` — jump back in help history.

### Authoring

- Create `.org` files in `~/.config/mae/help/` with `:ID:` properties.
- `:help-edit <topic>` — open/create a user help node for editing.
- User-authored nodes are loaded on startup alongside seed nodes.

### Migration from org-roam

```
:kb-register MyNotes ~/RoamNotes
```

Or ask the AI: "import my KB at ~/RoamNotes"

MAE recursively walks the directory, parses all `.org` files, and reports results:

```
Registered 'MyNotes': 2,342 nodes, 4,891 links
  Health: 45 orphans, 12 broken links, 3 duplicate IDs
```

### Backup and Restore

**Your org files ARE the backup.** They're plain text on disk — version them with git, sync them with any tool you like.

- The SQLite cache is disposable: delete it and reimport, zero data loss.
- `:kb-save <path>` exports a SQLite snapshot (useful for sharing the index).
- `:kb-load <path>` imports a snapshot.
- `:kb-reimport <name>` rebuilds from org source.
- **There is no new data format to manage.** Your existing org files + git workflow = complete data lifecycle.

### Health Monitoring

- `:kb-health` — orphan nodes, broken links, namespace distribution.
- Health metrics are also reported automatically after `:kb-register` and `:kb-reimport`.

## AI Integration

The AI agent uses the same tools as the manual and KB:

| Tool | Description |
|------|-------------|
| `kb_search` | Full-text search across all KB nodes |
| `kb_get` | Fetch a specific node by ID |
| `kb_links_from` / `kb_links_to` | Navigate the link graph |
| `kb_graph` | BFS neighborhood subgraph around a node |
| `kb_health` | Health report with orphans, broken links, namespace counts |
| `kb_instances` | List registered federated instances |
| `kb_register` | Register an org directory (AI can handle "import my KB") |
| `kb_unregister` | Remove an instance |
| `kb_reimport` | Refresh after org file edits |

## Comparison with Alternatives

| Feature | MAE KB | Obsidian | Roam Research |
|---------|--------|----------|---------------|
| Data format | Org-mode (plain text) | Markdown | Proprietary JSON |
| Storage | Local files + SQLite index | Local vault | Cloud (proprietary) |
| Link model | Typed graph, reverse index | Wiki-links, backlinks | Block references |
| Search | FTS5 + fuzzy + substring | Basic full-text | Full-text + filter |
| AI integration | Peer actor (same API) | Plugin (Copilot) | None native |
| Federation | Multi-directory, cross-KB | Single vault | Single graph |
| Open source | GPL-3.0 | Freemium, closed core | Proprietary |
| Offline | Full offline | Full offline | Requires sync |
| Extensibility | Scheme + module system | JS plugins | CSS themes only |

### Migration from Obsidian

- Convert Markdown to org via pandoc: `pandoc -f markdown -t org input.md -o output.org`
- Add `:ID:` properties (org-roam's `org-roam-migrate-wizard` helps).
- Register: `:kb-register MyVault ~/converted-vault`

### Migration from Roam Research

- Export as Markdown or JSON from Roam settings.
- Convert to org, assign `:ID:` properties.
- `((block-ref))` maps lossy to heading-level `:ID:` nodes.
- Register: `:kb-register RoamExport ~/roam-export`

## Collaborative Knowledge Bases

MAE knowledge bases can be shared between peers using the same CRDT infrastructure as collaborative text editing. Each KB node and collection is a yrs document, enabling concurrent edits with automatic conflict resolution.

### CRDT-Backed Nodes

Each shared node is a `KbNodeDoc` backed by a yrs `YMap`:

| Field | yrs Type | Notes |
|-------|----------|-------|
| `title` | `YText` | Concurrent character-level edits |
| `body` | `YText` | Full CRDT text (org-mode content) |
| `tags` | `YArray` | Set-like semantics (add/remove) |
| `links` | `YArray` | Bidirectional link targets |
| `meta` | `YMap` | Arbitrary key-value metadata |

All yrs documents use UTF-16 offset encoding (`OffsetKind::Utf16`) per the Yjs standard. Change detection uses SHA-256 content hashing rather than state vector comparison (yrs state vectors are monotonically increasing even after undo).

### KB Collections

A `KbCollectionDoc` is a CRDT manifest representing a shared knowledge base. It tracks:

- **Members** — peer identities with read/write roles.
- **Node manifest** — the set of node IDs belonging to this collection.
- **Convergent state** — collection-level metadata (name, description, sync mode).

Collections are addressed as `kbc:{kb_id}` in the sync protocol and route through the same state-server infrastructure as collaborative buffers.

### Data Directory Layout

KB data follows XDG conventions under `$XDG_DATA_HOME/mae/kb/` (default `~/.local/share/mae/kb/`):

```
kb/
├── local/          # Local-only KB instances (SQLite + org cache)
├── shared/         # CRDT-synced KB instances (yrs docs + SQLite)
├── backups/        # Periodic SQLite snapshots
└── meta.toml       # Per-KB metadata (UUID, name, sync config)
```

See `crates/kb/src/data_dir.rs` for path resolution and directory initialization.

### Backup & Restore

- **Periodic snapshots** — SQLite database snapshots taken at configurable intervals.
- **Configurable retention** — old backups pruned by age or count.
- **Pre-sync backup** — automatic snapshot before first sync with a remote peer.

See `crates/kb/src/backup.rs` for implementation details.

### Sharing Protocol (Preview)

Sharing a KB follows the same lifecycle as collaborative buffers:

1. **Share** — host publishes the collection doc to the state-server, which creates per-node CRDT documents.
2. **Join** — peer requests the collection manifest, then syncs individual node documents on demand.
3. **Leave** — peer unsubscribes from updates; local state is retained for offline use.

Offline reconciliation uses yrs state vector exchange on reconnect. Two sync modes are supported: **continuous** (real-time push/pull) and **manual** (explicit `:collab-sync`).

### Export

`export_kb()` supports Org and Markdown output formats:

- Link syntax is converted between formats (`[[id|text]]` to `[text](id)` for Markdown).
- Subset export by node IDs (e.g., export a single topic cluster).
- Full KB export produces one file per node with a manifest index.

See `crates/kb/src/export.rs`.

### Authentication

- **PSK mutual auth** — HMAC-SHA256 challenge-response before `initialize`. Both peers must share a pre-configured key.
- **SSH key auth** — planned for v0.12.0, replacing PSK for multi-user deployments.

Until authentication is enabled, collaborative KB access is trusted-LAN only (same security model as the state-server).

## Philosophy

1. **Plain text is the only immortal format.** SQLite is derived. Cloud sync is a dependency. Org files survive every tool transition.
2. **AI as peer, not plugin.** MAE's AI calls `kb_search`, `kb_get`, `kb_graph` — the same query surface the human uses. No impedance mismatch.
3. **Federation > monolithic vault.** Life has multiple knowledge domains. Obsidian forces one vault or vault-switching. MAE federates: each domain is a registered instance, searchable together.
4. **Ownership means exit.** Your org files are yours. No account, no sync service, no API key required to read your own notes.
5. **Performance at the editor layer.** In-memory graph with pre-lowercased search cache. FTS5 with porter stemmer. Sub-millisecond search across thousands of nodes. No Electron, no browser runtime.
