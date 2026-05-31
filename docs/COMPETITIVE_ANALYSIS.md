# MAE Competitive Analysis

**Modern AI Editor (MAE) v0.11.x** — Rust + Scheme, GPL-3.0

This document compares MAE against four editors that represent the current competitive landscape:
VSCode (mainstream IDE baseline), Zed (Rust-native, CRDT collab), Emacs/Doom (extensibility
and org-mode benchmark), and Cursor (AI-first VSCode fork). Each section examines a different
capability dimension.

Legend: `Y` = fully supported, `P` = partial / limited, `N` = not supported, `WIP` = in active
development, `planned` = on public roadmap.

---

## 1. Core Editing

| Feature | MAE | VSCode | Zed | Emacs/Doom | Cursor |
|---|---|---|---|---|---|
| Modal editing (vi-normal/insert/visual) | Y | via ext | Y | via evil-mode | via ext |
| Visual line / visual block modes | Y | via ext | Y | via evil-mode | via ext |
| Text objects (word, sentence, paragraph, block) | Y | via ext | Y | via evil-mode | via ext |
| Surround (add/change/delete delimiters) | Y | via ext | N | via evil-surround | via ext |
| Dot-repeat (`.`) | Y | via ext | Y | via evil-mode | via ext |
| Named macros (record / replay, `q`/`@`) | Y | via ext | Y | via evil-mode | via ext |
| Registers (named yank/paste, `"a`…`"z`) | Y | via ext | N | Y | via ext |
| Multi-cursor editing | Y | Y | Y | via ext | Y |
| Structural selection (tree-sitter) | Y | via ext | Y | via treesit.el | via ext |
| Undo tree (branching history) | planned | via ext | N | via undo-tree | N |
| Replace mode (`R`) | planned | Y | Y | Y | Y |
| Snippet engine | WIP | Y | Y | via yasnippet | Y |
| Count prefixes for motion/operators | Y | via ext | Y | via evil-mode | via ext |
| Jump list (`C-o`/`C-i`) | Y | P | Y | via evil-mode | via ext |
| Marks (`ma`/`'a`) | Y | via ext | N | via evil-mode | via ext |

**Notes:**

- "via ext" for VSCode/Cursor means VSCode Vim extension, which reimplements vim semantics
  in JavaScript and has known parity gaps (macros across sessions, some text objects, register
  edge cases).
- Zed's modal editing is first-class Rust, not a plugin, giving it better performance parity
  with MAE. Registers and surround are the current gaps.
- Emacs/Doom's evil-mode is the most complete vim emulation outside of actual vim/neovim — 15+
  years of community polish. MAE is the only other editor with a native Rust modal
  implementation at this level of completeness.

---

## 2. Language Intelligence

| Feature | MAE | VSCode | Zed | Emacs/Doom | Cursor |
|---|---|---|---|---|---|
| LSP client (spec-compliant) | Y | Y | Y | via lsp-mode/eglot | Y |
| Go-to-definition / peek definition | Y | Y | Y | Y | Y |
| Find references | Y | Y | Y | Y | Y |
| Hover documentation | Y | Y | Y | Y | Y |
| Completion popup | Y | Y | Y | Y | Y |
| Signature help | Y | Y | Y | Y | Y |
| Inline diagnostics (error squiggles) | Y | Y | Y | Y | Y |
| Diagnostic list / workspace errors | Y | Y | Y | Y | Y |
| Code actions (quick fix, refactor) | Y | Y | Y | Y | Y |
| LSP rename | Y | Y | Y | Y | Y |
| Workspace symbol search | Y | Y | Y | Y | Y |
| Document symbol outline | Y | Y | Y | Y | Y |
| Semantic highlighting (LSP tokens) | planned | Y | Y | via lsp-mode | Y |
| DAP client (breakpoints, step/continue) | Y | Y (built-in) | N | via dap-mode | via ext |
| Conditional breakpoints / logpoints | Y | Y | N | via dap-mode | Y |
| Inline variable values during debug | planned | Y | N | P | Y |
| Call stack inspection | Y | Y | N | via dap-mode | Y |
| Variable watch expressions | Y | Y | N | via dap-mode | Y |
| Gutter: breakpoints + exec line | Y | Y | N | via dap-mode | Y |
| AI access to LSP/DAP context | Y | N | N | N | P |

**Notes:**

- MAE exposes both LSP and DAP context to the AI agent as structured tool results — the AI
  can call `lsp_references`, `lsp_hover`, and `debug_state` using the same code paths
  available to keybindings. No other editor in this comparison does this.
- Cursor's AI has indirect access to diagnostics via context injection but does not have
  programmatic tool-call access to the LSP/DAP protocol layer.
- Semantic highlighting (LSP `textDocument/semanticTokens`) is on MAE's roadmap; tree-sitter
  syntax highlighting is already shipping for 13 languages.

---

## 3. AI Integration

| Feature | MAE | VSCode | Zed | Emacs/Doom | Cursor |
|---|---|---|---|---|---|
| Inline completions (ghost text) | planned | via Copilot/ext | Y (Zed AI) | via Copilot.el | Y (Tab) |
| Chat / conversation buffer | Y | via Copilot Chat | Y | via gptel/ellama | Y |
| Streaming responses | Y | Y | Y | Y | Y |
| Multi-provider (Claude/OpenAI/Gemini/DeepSeek) | Y | via ext | P | via gptel | P |
| Tool-calling / function calling | Y | N | N | N | N |
| Agent mode (multi-step autonomous) | Y | P (experimental) | N | via org-ai | N |
| AI calls same API as human keybindings | Y | N | N | N | N |
| AI can read/write buffers directly | Y | P | N | P | P |
| AI can run shell commands | Y (tiered) | N | N | N | Y |
| AI can navigate LSP (definitions/refs) | Y | N | N | N | N |
| AI can inspect DAP debug state | Y | N | N | N | N |
| Permission tiers (read/write/shell/privileged) | Y | N | N | N | N |
| Cost tracking / token budget dashboard | Y | N | N | N | N |
| Context compaction / graceful degradation | Y | N | N | N | N |
| Prompt caching (Claude cache_control) | Y | N | N | N | N |
| MCP server (external AI access) | Y | N | N | N | N |
| Conversation persistence (save/load) | Y | P | N | P | N |
| Input lock during AI operations | Y | N | N | N | N |
| Watchdog / circuit breaker | Y | N | N | N | N |
| AI agent setup wizard | Y | N | N | N | P |
| Self-test suite for AI tools | Y | N | N | N | N |
| Model exam (deterministic tool-call eval) | Y | N | N | N | N |

**Notes:**

- "AI as peer actor" is MAE's defining AI architecture. When the AI calls `buffer-insert` or
  `lsp-references`, it executes the same Scheme function that a user's keypress dispatches.
  There is no parallel AI-only API surface to maintain, no simulated keypresses, no
  reimplemented buffer logic for AI. This means every editor capability is automatically
  available to the AI without additional integration work.
- Cursor's Tab completion is strong for inline suggestions, which is MAE's current gap.
  Cursor's "agent mode" can run terminal commands but does not have structured tool access
  to LSP or DAP — it operates on file text and terminal output.
- Zed AI uses first-party infrastructure and has fewer provider options. There is no public
  tool-calling API for extensions.
- VSCode Copilot Chat is evolving quickly. Its "agent mode" (March 2025) can create/edit
  files and run terminal commands. It does not expose LSP/DAP state as tool call results.

---

## 4. Collaboration

| Feature | MAE | VSCode | Zed | Emacs/Doom | Cursor |
|---|---|---|---|---|---|
| Real-time collaborative editing | Y | via Live Share | Y | via crdt.el | N |
| CRDT algorithm | yrs (YATA) | OT (proprietary) | yrs (YATA) | CRDT (custom) | N |
| Persistent document identity (survives disconnect) | Y | N | N | N | N |
| Local-first (works offline, sync when reconnected) | Y | N | P | P | N |
| Self-hosted state server | Y | N | N | P | N |
| No vendor cloud dependency for collab | Y | N | N | Y | N |
| Offline edit queue (sync on reconnect) | Y | N | N | N | N |
| Per-user undo stack (CRDT-safe) | Y | N | Y | N | N |
| Cursor awareness (other users' positions) | Y | Y | Y | P | N |
| PSK authentication | Y | N/A | N/A | N | N |
| E2E encryption | planned | N | N | N | N |
| mDNS LAN discovery | Y | N | N | N | N |
| P2P (no server required) | planned | N | N | N | N |
| Collaborative knowledge base sharing | Y | N | N | N | N |
| Status line collab indicator | Y | N | Y | N | N |

**Notes:**

- Zed and MAE both use yrs (the Rust port of Yjs), so they share the same CRDT algorithm.
  The critical difference is **persistent document identity**: Zed's doc IDs are
  session-scoped (a new session = new document). MAE's doc IDs persist across sessions,
  enabling asynchronous collaboration — a contributor can edit while the host is offline,
  and changes merge when both reconnect. This is unusual in any collaborative editor.
- VSCode Live Share uses operational transformation over Microsoft's servers. There is no
  self-hosted option and no offline capability.
- MAE's collaborative KB sharing is unique in this comparison: participants can share an
  entire knowledge base (org-mode nodes + bidirectional link graph), not just text buffers.
- Emacs `crdt.el` is a real-time collab package over TCP, but it is not widely deployed and
  lacks the GUI awareness features of Zed or MAE.

---

## 5. Knowledge Management

| Feature | MAE | VSCode | Zed | Emacs/Doom | Cursor |
|---|---|---|---|---|---|
| Knowledge base / note graph | Y | via ext | N | via org-roam | N |
| Org-mode syntax support | Y | via ext | N | Y (native) | N |
| Org babel (code block execution) | Y | N | N | Y (native) | N |
| Supported babel languages | 5+ | N/A | N/A | 40+ | N/A |
| SQLite-backed persistence | Y | N/A | N/A | via org-roam | N/A |
| Full-text search (FTS5) | Y | via ext | N | via org-roam | N |
| Bidirectional links | Y | via ext | N | via org-roam | N |
| Link graph navigation | Y | via ext | N | via org-roam | N |
| KB graph visualization | planned | via ext | N | via org-roam | N |
| Daily notes | Y | via ext | N | via org-journal | N |
| Agenda / TODO tracking | Y | via ext | N | Y (native) | N |
| Org export (HTML / Markdown) | Y | N | N | Y (native) | N |
| Collaborative KB sharing (CRDT) | Y | N | N | N | N |
| Federated KB instances | Y | N | N | N | N |
| AI access to KB (search, get, links) | Y | N | N | N | N |
| KB nodes as help system | Y | N | N | N | N |
| User-authored help nodes | Y | N | N | P | N |

**Notes:**

- Emacs/Doom org-mode is the clear benchmark for knowledge management. Org-mode has 20+
  years of development: agenda, capture templates, clock, archive, publish, citation, and
  babel supporting 40+ languages. MAE's org implementation is younger but covers the core
  structural editing, linking, export, and babel use cases, with a novel CRDT-collaborative
  layer that org-mode does not have.
- MAE's KB is the backing store for the built-in help system — the same node format that
  users write for personal notes is the format used for editor documentation, tutorial
  lessons, command references, and concept pages. This unifies the note-taking and
  documentation experiences.
- "Federated KB instances" means multiple running MAE instances (or the state server) can
  share KB subsets over the same yrs CRDT transport used for text buffers.
- VSCode extensions like Foam and Dendron provide wiki-link graphs, but they are not
  integrated with the editor's AI or embedded in the help system.

---

## 6. Extensibility and Platform

| Feature | MAE | VSCode | Zed | Emacs/Doom | Cursor |
|---|---|---|---|---|---|
| Extension language | Scheme (R7RS-small) | TypeScript/JS | WASM (Rust/Go/etc.) | Emacs Lisp | TypeScript/JS |
| Live REPL for extension language | Y | N | N | Y | N |
| Hot reload (redefine functions at runtime) | Y | N | N | Y | N |
| Defadvice / function wrapping | Y (Scheme) | N | N | Y | N |
| MCP server (external programmatic control) | Y (130+ tools) | N | N | N | N |
| Package manager | Y (Scheme modules) | Y (marketplace) | Y (extension registry) | Y (straight/use-package) | Y (marketplace) |
| Config language | Scheme + TOML | JSON + JS | JSON | Elisp | JSON + JS |
| TUI rendering (terminal) | Y | N | N | Y | N |
| GPU rendering (native window) | Y (Skia) | Electron (Chromium) | Y (GPUI/Metal/Vulkan) | N (Pgtk/Cairo) | Electron |
| Dual TUI + GPU (same codebase) | Y | N | N | N | N |
| Startup time (GUI, cold) | < 200ms (target) | 2-4s | < 500ms | 1-5s | 2-5s |
| Startup time (TUI, cold) | < 100ms | N/A | N/A | 1-5s | N/A |
| Self-contained binary | Y | N | N | N | N |
| Linux support | Y | Y | Y | Y | Y |
| macOS support | Y | Y | Y | Y | Y |
| Windows support | planned | Y | Y | Y | Y |
| Wayland / HiDPI | Y | Y | Y | P | Y |
| Inline image rendering (GUI) | Y | Y | N | via imagemagick | Y |
| Native SVG rendering | Y | Y | N | N | Y |
| Test suite size | 5,796 | N/A | N/A | N/A | N/A |

**Notes:**

- MAE's Scheme runtime is purpose-built (R7RS-small, no Steel dependency as of v0.11.x). It
  provides a real Lisp REPL, hygienic macros, proper tail calls, and first-class
  continuations. This is architecturally closer to Emacs than to Zed's WASM extension model,
  while gaining Rust's memory safety for the core.
- Zed's WASM extensions are sandboxed and constrained by the extension API surface. There is
  no equivalent to Emacs's `defadvice` or MAE's ability to redefine any function at runtime.
- The MCP server (130+ tools over JSON-RPC on a Unix socket) is MAE's unique external
  control plane. It is what allows Claude Code to operate MAE as a development environment
  for itself — the same interface used internally by the AI agent is exposed externally.
- Electron (VSCode, Cursor) ships Chromium as a rendering dependency (~150MB), which
  dominates startup time and memory use. MAE and Zed are native binaries.
- Windows support is planned for MAE. The primary blockers are Skia build pipeline on MSVC
  and terminal emulator (alacritty_terminal) Windows compatibility.

---

## MAE's Unique Advantages

These are capabilities that no single competitor in this analysis fully provides. Each
represents a deliberate architectural decision rather than a feature gap in competitors.

### 1. AI as Peer Actor

In every other editor, AI is a plugin, a sidebar, or a subprocess. It operates on file
text and terminal output, occasionally with file read/write access.

In MAE, the AI agent calls the same Scheme functions as user keybindings:

```scheme
;; A user pressing 'gd' calls this
(lsp-goto-definition)

;; The AI tool `lsp_definition` calls this exact same function
;; No separate implementation, no simulated keypresses
```

This means every editor capability — modal editing operators, LSP navigation, DAP debug
inspection, KB queries, shell commands, collab sync — is available to the AI at zero
additional integration cost. New Scheme functions are immediately available to both
humans and AI. The permission tier system (`ReadOnly`/`Write`/`Shell`/`Privileged`) enforces
security boundaries at the dispatch layer, not by limiting which functions exist.

### 2. Persistent Document Identity for Collaboration

MAE assigns each buffer a document ID that persists across editor sessions. This is unique
among CRDT-based editors in this comparison.

- Zed: session-scoped doc IDs. If the host closes the editor, the session ends.
- VSCode Live Share: session-scoped, server-mediated.
- MAE: doc IDs survive host disconnection. Contributors can edit offline. Changes
  merge via CRDT when connectivity resumes. The state server persists documents across
  machine restarts with WAL-backed SQLite.

This enables genuinely asynchronous collaboration — closer to git's model than to Google
Docs's model — without sacrificing real-time capabilities when all participants are online.

### 3. Local-First CRDT with Self-Hosted Infrastructure

MAE satisfies five of the seven Ink & Switch local-first ideals today:

1. No loading spinners for local work (all editing is local, sync is background)
2. Your work is not trapped in the cloud (data lives in `~/.local/share/mae/`)
3. Network is optional (offline edits queue and sync on reconnect)
4. Collaboration without conflict (yrs CRDT handles concurrent edits)
5. The user owns their data (GPL, self-hosted, no vendor API required for collab)

The `mae-state-server` binary is a standalone TCP server that any user can run on their
own hardware. There is no MAE-operated cloud service. Collaboration does not require an
account, a subscription, or outbound connections to a third-party server.

### 4. Collaborative Knowledge Base

Other editors share text buffers. MAE shares knowledge graphs.

The KB sharing protocol uses the same yrs CRDT transport as text buffer sync. KB nodes
(org-mode headings with properties, tags, and links) become `YMap` entries. Bidirectional
links become `YArray` entries. Multiple participants can concurrently add nodes, edit
content, and create links, with CRDT merge semantics handling conflicts.

This means a team's shared knowledge base — architecture decisions, runbooks, meeting
notes, concept maps — can be collaboratively edited with the same guarantees as code.

### 5. MCP Server as External Control Plane

MAE exposes 130+ tools over a JSON-RPC MCP server on a Unix socket. Any process that
speaks MCP (including Claude Code) can:

- Read and write any buffer
- Execute any editor command
- Query LSP diagnostics and symbols
- Inspect DAP debug state
- Search the knowledge base
- Evaluate Scheme expressions
- Inspect editor configuration and state

This is how this document was produced: Claude Code connected to MAE's MCP server and
used `buffer_write`, `project_search`, and `eval_scheme` as part of its workflow. The
editor is its own development environment.

No other editor in this comparison provides this level of external programmability. VSCode
has an extension API but not an MCP server accessible to external processes.

### 6. Dual TUI + GPU Rendering from One Codebase

MAE has two rendering backends — `mae-renderer` (ratatui/crossterm, terminal) and
`mae-gui` (winit/Skia, native GPU window) — sharing a `Renderer` trait and all layout
logic in `mae-core`. The same editor binary can run in a terminal or as a native desktop
application.

This matters for deployment flexibility: SSH sessions use TUI, desktop use uses GPU.
The GPU backend (Skia) enables features impossible in a terminal: sub-pixel text rendering,
inline image display, smooth sub-line scrolling, native SVG rendering, and HiDPI support.
The TUI backend enables the editor to run on servers and in CI environments with no
display dependency.

Zed is GPU-only (GPUI). Emacs has separate TUI and GUI builds with significant code
divergence. MAE shares the entire application layer.

---

## Feature Gap Analysis

### High Priority — Blocks Adoption

These gaps prevent users from switching from their current editor as a daily driver.

| Gap | Affected Users | Current Status | Notes |
|---|---|---|---|
| Inline AI completions (ghost text) | All users expecting Copilot/Cursor-style flow | Planned | Chat agent exists; inline Tab completions not yet implemented |
| Snippet engine | All developers (boilerplate, templates) | WIP (`mae-snippets` crate) | yasnippet/LuaSnip parity needed |
| Replace mode (`R`) | Vim users doing character-level overwrite | Planned | Normal/insert/visual complete; replace missing |
| Semantic highlighting (LSP tokens) | Users from VSCode/Zed | Planned | Tree-sitter highlighting ships; LSP semantic tokens not yet consumed |
| Windows support | Windows-primary developers | Planned | Skia build pipeline is the main blocker |

### Medium Priority — Power-User Expectations

These gaps are noticed by experienced users but do not prevent adoption.

| Gap | Affected Users | Current Status | Notes |
|---|---|---|---|
| Undo tree visualization | Emacs/Vim power users | Planned | Branching undo exists in yrs; no visual browser |
| KB graph visualization | Org-roam / Obsidian users | Planned | Graph API exists; no canvas renderer |
| Inline DAP variable values | Debug-heavy users | Planned | Debug panel exists; inline value overlay not yet implemented |
| E2E encryption for collab | Security-conscious teams | Planned | PSK auth exists; encryption of document content not yet applied |
| Auto-import via LSP code actions | Language-heavy users (TypeScript, Java) | P | Code actions exist; auto-import trigger on completion not wired |
| Org-mode citation / bibliography | Academics | Not started | Core org structural editing complete; `#+cite:` not parsed |
| Magit-level git UI | Emacs/Doom users | P | Status buffer and stage/commit exist; interactive rebase, log graph not done |

### Low Priority — Differentiation and Future Work

These are forward-looking capabilities that extend MAE's unique position.

| Gap | Affected Users | Current Status | Notes |
|---|---|---|---|
| RAG pipeline (embeddings + vector search) | AI-heavy research workflows | Planned (ROADMAP Phase 12) | Would inject semantically relevant KB nodes into AI context automatically |
| Web UI for KB | Non-editor users, team wikis | Not started | Requires HTTP server for KB export; read-only view would suffice initially |
| Per-node KB permissions | Enterprise teams | Not started | Current sharing is all-or-nothing per KB instance |
| True P2P (no state server required) | Fully decentralized workflows | Planned | mDNS discovery ships; direct peer transport without state server not done |
| AI harness (per-model prompt tuning) | Teams running multiple model providers | Planned (ROADMAP) | Model profiles exist in `mae-ai`; prompt template per model not surfaced to users |
| PDF preview | LaTeX / org-export users | Planned (Phase 8 M8) | Org HTML export ships; PDF via LaTeX pipeline not yet integrated |
| Org-mode publish (static site) | Bloggers, docs sites | Not started | Would require a multi-file export pipeline |

---

## Summary

| Dimension | MAE Strength | Primary Gap vs. Best-in-Class |
|---|---|---|
| Core Editing | Native Rust vi-modal, full parity with evil-mode | Replace mode, undo tree visualization |
| Language Intelligence | LSP + DAP both first-class, AI has structured access to both | Semantic highlighting, inline debug values |
| AI Integration | Peer actor model, tool-calling, multi-provider, permission tiers, MCP | Inline ghost-text completions |
| Collaboration | Persistent doc IDs, local-first, self-hosted, offline queue, collab KB | E2E encryption, P2P transport |
| Knowledge Management | Org-mode + babel + SQLite FTS + collaborative sharing | Babel language breadth (vs. Emacs), graph visualization |
| Extensibility | Live Scheme REPL, hot reload, MCP server, dual TUI+GPU | Windows support, snippet engine |

MAE's architectural bets — AI as peer, persistent identity, local-first CRDT, Scheme
extensibility, dual rendering — are each individually present in research or niche tools
but have not been combined in a single shipping editor. The feature gap analysis above
represents the execution work required to close parity on established expectations before
these bets compound into a durable advantage.
