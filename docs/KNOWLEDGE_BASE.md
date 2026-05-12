# Knowledge Base

MAE's knowledge base is a typed graph of nodes with bidirectional links. It serves as both the built-in help system and a personal knowledge graph (org-roam equivalent).

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

Federation lets you register external org directories as searchable KB instances alongside MAE's built-in help.

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

The AI agent uses the same tools as the help system:

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

## Philosophy

1. **Plain text is the only immortal format.** SQLite is derived. Cloud sync is a dependency. Org files survive every tool transition.
2. **AI as peer, not plugin.** MAE's AI calls `kb_search`, `kb_get`, `kb_graph` — the same query surface the human uses. No impedance mismatch.
3. **Federation > monolithic vault.** Life has multiple knowledge domains. Obsidian forces one vault or vault-switching. MAE federates: each domain is a registered instance, searchable together.
4. **Ownership means exit.** Your org files are yours. No account, no sync service, no API key required to read your own notes.
5. **Performance at the editor layer.** In-memory graph with pre-lowercased search cache. FTS5 with porter stemmer. Sub-millisecond search across thousands of nodes. No Electron, no browser runtime.
