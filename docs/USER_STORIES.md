# MAE User Stories — End-to-End Workflows

> Modern AI Editor (MAE) v0.11.x — Rust + Scheme, vi-modal, GPU/TUI dual backend

This document collects eight representative user stories covering the full breadth of MAE's capabilities. Each story is written from the perspective of a real persona, with concrete commands, verification steps, and an honest assessment of gaps relative to competing tools.

---

## Table of Contents

1. [Solo Dev — Multi-Project Git](#1-solo-dev--multi-project-git)
2. [Solo Dev — AI-Assisted Feature Dev](#2-solo-dev--ai-assisted-feature-dev)
3. [Knowledge Worker — Personal KB](#3-knowledge-worker--personal-kb)
4. [Team — Shared KB + AI](#4-team--shared-kb--ai)
5. [Developer — LSP Code Navigation](#5-developer--lsp-code-navigation)
6. [Developer — DAP Debug Session](#6-developer--dap-debug-session)
7. [Developer — Org + Babel Notebooks](#7-developer--org--babel-notebooks)
8. [Power User — Scheme Extension](#8-power-user--scheme-extension)

---

## 1. Solo Dev — Multi-Project Git

**Status:** Complete

### Persona

An individual Rust developer juggling two or three active projects at once. They expect snappy file navigation, a tight git integration loop, and zero ceremony around switching contexts between repos.

### Goal

Open a project, make a targeted change, stage and commit it, then switch to a second project — all without leaving the editor.

### Workflow

1. Launch MAE pointing at the first project:
   ```
   mae ~/src/project-alpha
   ```
   MAE auto-detects the project root from `Cargo.toml` / `.git` and registers it.

2. Find a source file by fuzzy name:
   ```
   SPC f f       → find-file (project-root-relative fuzzy picker)
   ```
   Type a partial filename; Enter opens it in the current window.

3. Make the edit in normal/insert mode (standard vi keybindings: `i`, `a`, `o`, `ciw`, etc.).

4. Open the Magit-style git status buffer:
   ```
   SPC g s       → git-status
   ```
   The status buffer lists unstaged changes, staged changes, and untracked files grouped by section.

5. Stage individual hunks or whole files:
   ```
   s             → stage file/hunk under cursor
   u             → unstage file/hunk under cursor
   TAB           → toggle fold to inspect diff inline
   ```

6. Commit the staged changes:
   ```
   cc            → git-commit (opens commit message buffer)
   ```
   Write the commit message, then `ZZ` or `SPC m c` to confirm.

7. Push to the remote:
   ```
   SPC g p       → git-push
   ```
   Output streams into a temporary buffer; MAE reports success or failure.

8. Switch to the second project:
   ```
   SPC p s       → project-switch (fuzzy picker over registered projects)
   ```
   Selecting a project changes the working directory, resets the file picker root, and restores any saved session for that project.

9. Repeat steps 2–7 for the second project.

### What to Verify

- `SPC f f` scopes the fuzzy picker to the current project root, not the global filesystem.
- Staging a hunk (`s` on a hunk header) stages only that hunk, leaving adjacent hunks unstaged.
- `SPC g s` refreshes automatically after a save (`after-save` hook wired to git-status refresh).
- `SPC p s` switches project context without leaving open buffers from the previous project orphaned — they remain accessible via `SPC b b`.
- Push output appears in a dedicated `*git-output*` buffer, not a blocking modal dialog.

### Key Features Used

- CWD-based project detection at startup
- Magit-style git status buffer with hunk-level staging
- Fuzzy file picker (`find-file`) scoped to project root
- Project registry and session persistence (`SPC p s`, `session-save` / `session-load`)
- `git-commit`, `git-push`, `git-pull` commands

### Gaps vs Competitors

| Competitor | Gap |
|---|---|
| VSCode / GitLens | No pull request authoring UI inside MAE yet (planned) |
| Emacs / Magit | Rebase interactive (`git rebase -i`) workflow not yet exposed |
| Zed | No built-in merge conflict resolution UI; users fall back to shell |

---

## 2. Solo Dev — AI-Assisted Feature Dev

**Status:** Complete

### Persona

A developer who wants to describe a feature in plain English and have the AI read the relevant code, propose a concrete implementation, and apply it — while the developer stays in the review seat.

### Goal

Implement a new feature by directing the AI through natural language, reviewing its proposed changes, and accepting or rejecting them without manual copy-paste.

### Workflow

1. Open the project and navigate to the relevant file:
   ```
   mae ~/src/my-project
   SPC f f       → open src/lib.rs
   ```

2. Open the AI chat buffer:
   ```
   SPC a c       → ai-chat (opens *AI* buffer in a split)
   ```
   The AI buffer shows the conversation history and a prompt input at the bottom.

3. Describe the feature in the prompt input (insert mode in the AI buffer):
   ```
   Please add a retry mechanism to the `fetch_data` function in src/lib.rs.
   It should retry up to 3 times with exponential backoff on network errors.
   ```
   Press Enter to send.

4. The AI uses its tool-calling interface to read the codebase:
   - `buffer_read` — reads `src/lib.rs`
   - `project_search` — searches for related error types and call sites
   - `lsp_hover` — resolves types at relevant positions

5. The AI proposes changes and writes them via `buffer_write`. MAE marks proposed regions with a diff-style overlay (green for additions, red for deletions).

6. Review the proposed diff inline. Accept or reject:
   ```
   SPC a a       → ai-accept (applies all pending AI changes)
   SPC a r       → ai-reject (reverts all pending AI changes)
   ```
   Alternatively, accept individual hunks from the diff overlay.

7. Run the AI-assisted prompt for a quick one-shot task (no interactive back-and-forth):
   ```
   SPC a p       → ai-prompt
   ```
   Type a task string; MAE sends it with the current buffer as context and applies the result.

8. Check AI status and token budget:
   ```
   SPC a s       → ai-status (shows model, tier, context utilization, cache hit rate)
   ```

9. Save and commit as in story 1.

### What to Verify

- AI tool calls (`buffer_read`, `buffer_write`, `project_search`) appear in the chat buffer as collapsible entries, not opaque black-box operations.
- Proposed changes are shown as a diff overlay before acceptance — the user is never surprised by silent edits.
- `ai-accept` and `ai-reject` operate atomically; a partial accept is possible at the hunk level.
- Input is locked during active AI inference (the status bar shows a spinner); `Esc` or `C-c` cancels the in-flight request.
- Token budget dashboard updates in real time; MAE auto-sheds non-critical tools at >85% context pressure.

### Key Features Used

- `ai-chat` (SPC a c) — conversational AI buffer with streaming output
- `ai-prompt` (SPC a p) — one-shot task with current buffer as context
- AI tool-calling: `buffer_read`, `buffer_write`, `project_search`, `lsp_hover`, `create_file`
- `ai-accept` / `ai-reject` — diff-overlay review workflow
- Permission tiers (Write tier required for `buffer_write`)
- Token budget dashboard with cache hit rate and context utilization
- Input lock + cancellation (`Esc` / `C-c`)

### Gaps vs Competitors

| Competitor | Gap |
|---|---|
| Cursor / GitHub Copilot | No inline ghost-text completions (tab-to-accept) yet — planned |
| Cursor Composer | No multi-file diff review panel; MAE shows diffs per-buffer |
| Zed AI | No inline "Apply" button at message level; accept/reject is buffer-wide or hunk-wide |
| Emacs gptel | MAE has richer tool-calling (LSP + DAP aware); gptel is text-only |

---

## 3. Knowledge Worker — Personal KB

**Status:** Complete

### Persona

A researcher or technical writer who organizes long-form notes in org-mode, wants bidirectional links between concepts, and needs to export to HTML or Markdown for sharing.

### Goal

Build and navigate a personal knowledge base of linked org-mode nodes, find information quickly via full-text search, capture daily notes, and export finished content.

### Workflow

1. Open MAE in the notes directory:
   ```
   mae ~/notes
   ```

2. Create a new KB node:
   ```
   :kb-create "My Concept"
   ```
   MAE creates an org-mode file in `~/.local/share/mae/kb/local/my-concept/`, opens it, and registers it in the SQLite graph store.

3. Write content in org-mode syntax: headings (`*`, `**`), prose, code blocks, lists.

4. Link to another node using double-bracket syntax:
   ```
   [[kb:other-concept][Other Concept]]
   ```
   MAE resolves the link on save and registers the bidirectional edge in the graph.

5. Follow a link under the cursor:
   ```
   C-c C-o  (or  RET  in normal mode)   → org-open-link
   ```

6. Search the KB by full-text:
   ```
   :kb-find "exponential backoff"
   ```
   Results appear in a picker showing node title, matched excerpt, and relevance score. Enter opens the node.

7. Open today's daily note:
   ```
   :daily            → daily-goto-today
   ```
   MAE creates `~/notes/daily/2026-05-31.org` if it does not exist.

8. Navigate to yesterday's or tomorrow's note:
   ```
   SPC n p           → daily-prev
   SPC n n           → daily-next
   ```

9. Export a node or subtree to HTML:
   ```
   SPC m e h         → org-export-html
   ```
   The exported file is written alongside the source; MAE reports the output path.

10. Export to Markdown:
    ```
    SPC m e m         → org-export-markdown
    ```

11. View the neighborhood graph of a node (links in, links out, two hops):
    ```
    :kb-view "my-concept"
    ```

### What to Verify

- Creating a node via `:kb-create` immediately makes it searchable via `:kb-find`.
- Bidirectional links: after adding `[[kb:B]]` to node A and saving, `:kb-links-to B` lists A.
- Daily notes persist across restarts; `:daily` re-opens the existing file, not a blank one.
- HTML export renders org headings, code blocks, and inline markup faithfully.
- The help buffer (`:help`) can navigate KB nodes directly: `Tab` cycles links, `Enter` follows them, `C-o` goes back.

### Key Features Used

- `:kb-create`, `:kb-find`, `:kb-view` commands
- `[[kb:node-id]]` bidirectional link syntax
- `org-open-link` (C-c C-o / RET)
- Daily notes (`:daily`, `daily-prev`, `daily-next`)
- Org export: HTML (`org-export-html`), Markdown (`org-export-markdown`), subtree export
- SQLite-backed graph store with FTS5 full-text search
- Help buffer KB navigation (Tab / Enter / C-o)

### Gaps vs Competitors

| Competitor | Gap |
|---|---|
| Obsidian | No graphical canvas / visual graph view in TUI; GUI graph overlay is partial |
| Roam Research | No block-level transclusion (node-level only) |
| Emacs Org-roam | Comparable feature set; MAE adds AI-queryable KB as differentiator |
| Logseq | No daily outline / journal view (flat file per day, not block-structured) |

---

## 4. Team — Shared KB + AI

**Status:** Complete (v0.11.0)

### Persona

A small engineering team (2–5 people) who want to co-edit architecture notes, share a living KB during a design sprint, and let the AI query the shared knowledge base during code review.

### Goal

Stand up a shared KB session, have team members join, co-edit nodes in real time, and invoke the AI against the shared KB without each person needing a local copy.

### Workflow

1. **Host:** start the MAE state server (if not already running):
   ```bash
   mae-state-server --bind 0.0.0.0:9473
   ```
   Or as a systemd user unit: `systemctl --user start mae-state-server`.

2. **Host:** launch MAE and start a collab session:
   ```
   mae ~/team-kb
   SPC C s       → collab-start
   ```
   MAE connects to the local state server and registers the session.

3. **Host:** share the KB:
   ```
   :kb-share "team-arch"
   ```
   The KB collection is registered on the state server. MAE prints the join address (`mae://hostname:9473/team-arch`).

4. **Guest:** connect to the host's state server:
   ```
   SPC C c       → collab-connect
   ```
   Enter the address when prompted. MAE establishes a TCP connection and performs PSK mutual auth (HMAC-SHA256).

5. **Guest:** join the shared KB:
   ```
   :kb-join "team-arch"
   ```
   MAE downloads the collection manifest and syncs all nodes via CRDT (yrs).

6. **Both:** discover sessions on the local network (mDNS):
   ```
   :collab-discover
   ```
   MAE broadcasts `_mae-sync._tcp.local` and lists discovered peers in a picker.

7. **Both:** open any KB node and edit concurrently. CRDT sync (YATA algorithm via yrs) merges concurrent edits without conflicts.

8. Check collab status:
   ```
   SPC C i       → collab-status
   ```
   Shows connected peers, sync state (`synced` / `pending` / `offline`), and per-buffer CRDT version.

9. The status line shows a live KB sync indicator: `[KB:3|synced]` (3 shared nodes, synced).

10. **AI on shared KB:** any connected user can ask the AI to query the shared KB:
    ```
    SPC a c
    ```
    In the AI chat:
    ```
    Summarize the ADRs in the team-arch KB and highlight unresolved decisions.
    ```
    The AI uses `kb_search` and `kb_get` against the shared (server-side) KB, not just the local copy.

11. Disconnect when done:
    ```
    SPC C d       → collab-disconnect
    ```

### What to Verify

- PSK auth prevents unauthenticated connections to the state server.
- Concurrent edits to the same node by two users converge to the same content (CRDT invariant).
- mDNS discovery lists the host without manual address entry on the same LAN.
- The status line KB indicator transitions from `pending` to `synced` after the sync completes.
- AI `kb_search` queries return results from the shared KB, not only locally indexed nodes.
- `:kb-leave` stops sync and removes the shared collection from the local graph without deleting locally authored nodes.

### Key Features Used

- `mae-state-server` standalone binary (TCP, WAL SQLite, per-doc locking)
- PSK mutual auth (HMAC-SHA256) in TCP accept path
- `collab-start`, `collab-connect`, `collab-disconnect` (SPC C s/c/d)
- `:kb-share`, `:kb-join`, `:kb-leave`
- `:collab-discover` (mDNS via `mdns-sd`, `_mae-sync._tcp.local`)
- yrs CRDT sync for KB nodes (YATA algorithm, per-user UndoManager)
- Status line KB sync indicator (`[KB:N|synced/offline/pending]`)
- `collab_kb_sync_mode` option: `"manual"` | `"on_save"`
- AI `kb_search` / `kb_get` tools operating on shared KB

### Gaps vs Competitors

| Competitor | Gap |
|---|---|
| Notion | No per-node access control or permission levels yet (planned) |
| Confluence | No rich-text WYSIWYG; MAE is org-mode plain text |
| Zed collab | Zed collab is buffer-level; MAE adds KB-level sharing as a distinct layer |
| Roam Multiplayer | MAE requires self-hosted server; no managed cloud offering |

---

## 5. Developer — LSP Code Navigation

**Status:** Complete

### Persona

A developer working in a large Rust codebase (50k+ lines) who relies on semantic navigation — go-to-definition, find-all-references, hover docs, inline diagnostics — rather than grep.

### Goal

Navigate from a function call site to its definition, inspect its type signature and doc comment, find all callers, and fix a compiler warning — without leaving the editor or running a terminal command.

### Workflow

1. Open the project (rust-analyzer attaches automatically if in PATH):
   ```
   mae ~/src/big-project
   ```
   The status bar shows `[LSP:rust-analyzer|ready]` once the server has indexed the workspace.

2. Position the cursor on a function call and go to its definition:
   ```
   gd            → lsp-goto-definition
   ```
   MAE jumps to the definition file and line. The jump is added to the jump list (`C-o` to go back).

3. Peek the definition without leaving the current file:
   ```
   SPC c d       → lsp-peek-definition
   ```
   A floating overlay shows the definition inline.

4. Hover to see the type signature and doc comment:
   ```
   K             → lsp-hover
   ```
   A popup appears with the rendered doc string and type. `C-f` / `C-b` scroll the popup.

5. Find all references:
   ```
   gr            → lsp-find-references
   ```
   Results appear in a references panel (file, line, preview). `C-n` / `C-p` navigate; Enter jumps.

6. Open the symbol outline for the current file:
   ```
   SPC c o       → lsp-symbol-outline
   ```
   A side panel lists all functions, structs, and impls. `C-n` / `C-p` navigate; Enter jumps to the symbol.

7. Rename a symbol across the workspace:
   ```
   SPC c r       → lsp-rename
   ```
   Type the new name; MAE shows a preview of all affected files. Confirm to apply.

8. Navigate diagnostics (errors / warnings):
   ```
   ]d            → lsp-next-diagnostic
   [d            → lsp-prev-diagnostic
   ```
   Each jump lands on the offending line and opens a hover popup with the diagnostic message.

9. Show all diagnostics for the current file:
   ```
   SPC c e       → lsp-show-diagnostics
   ```

10. Trigger completion in insert mode:
    ```
    C-n           → lsp-complete-next (cycles through LSP completions)
    C-p           → lsp-complete-prev
    ```
    The completion popup shows kind icons (function, variable, module) and doc previews.

11. Apply a code action (e.g., "add missing import", "fill match arms"):
    ```
    SPC c a       → lsp-code-action
    ```
    A picker lists available actions; Enter applies the selected one.

12. Format the buffer via LSP:
    ```
    SPC c f       → lsp-format
    ```

### What to Verify

- `gd` on a trait method resolves to the concrete implementation, not the trait declaration, when the type is unambiguous.
- `gr` references panel correctly de-duplicates results from macro expansions.
- Diagnostics render as gutter markers (error/warning/info icons) and update live as the file is edited.
- Hover popup dismisses cleanly on cursor movement and does not interfere with normal-mode commands.
- Completion popup closes on `Esc` and does not corrupt the buffer if dismissed mid-token.

### Key Features Used

- `lsp-goto-definition` (`gd`), `lsp-peek-definition`
- `lsp-find-references` (`gr`), `lsp-peek-references`
- `lsp-hover` (`K`) with scrollable popup
- `lsp-symbol-outline`, `lsp-document-symbols`
- `lsp-rename` with workspace-wide preview
- `lsp-next-diagnostic` (`]d`) / `lsp-prev-diagnostic` (`[d`)
- `lsp-show-diagnostics`
- LSP completion popup (`C-n` / `C-p`)
- `lsp-code-action`
- `lsp-format`
- Gutter diagnostic severity markers

### Gaps vs Competitors

| Competitor | Gap |
|---|---|
| VSCode / rust-analyzer extension | No inlay hints (parameter names, return types shown inline) yet |
| Emacs / eglot | Comparable; MAE adds AI-aware LSP (AI can call `lsp_hover` as a tool) |
| Zed | No semantic diff integration (LSP-aware diff views) |
| IntelliJ | No structural search-and-replace (SSR) |

---

## 6. Developer — DAP Debug Session

**Status:** Complete

### Persona

A developer debugging a Rust binary or Python script who wants breakpoints, step-through execution, variable inspection, and expression evaluation — integrated with the same buffer they are editing.

### Goal

Set a breakpoint, launch a debug session, step through the suspect code path, inspect the call stack and variables, and evaluate an expression to test a hypothesis — all without leaving MAE.

### Workflow

1. Open the file to debug:
   ```
   mae ~/src/my-program
   SPC f f       → open src/main.rs
   ```

2. Set a breakpoint on the current line:
   ```
   SPC d b       → debug-toggle-breakpoint
   ```
   A red circle appears in the gutter. Toggle again to remove.

3. Set a conditional breakpoint:
   ```
   SPC d b       → debug-toggle-breakpoint (with prefix C-u for conditional)
   ```
   MAE prompts for a condition expression (e.g., `count > 100`).

4. Start the debug session (lldb-dap for Rust, debugpy for Python):
   ```
   SPC d s       → debug-start
   ```
   MAE launches the DAP adapter, compiles if needed, and attaches. The debug panel opens at the bottom.

5. The debug panel shows: call stack, local variables, watch expressions, and DAP output.

6. Continue execution to the next breakpoint:
   ```
   SPC d c       → debug-continue
   ```

7. Step over the current line:
   ```
   SPC d n       → debug-step-over
   ```

8. Step into a function call:
   ```
   SPC d i       → debug-step-into
   ```

9. Step out of the current function:
   ```
   SPC d o       → debug-step-out
   ```

10. Inspect a variable (cursor on the variable name in the call stack panel):
    ```
    SPC d v       → debug-inspect
    ```
    Nested structs and enums expand interactively with `TAB`.

11. Add a watch expression:
    ```
    SPC d w       → debug-add-watch
    ```
    Type an expression; MAE evaluates it at each step and shows the result in the watch panel.

12. Evaluate an arbitrary expression in the current frame:
    ```
    SPC d e       → debug-eval
    ```
    The result appears in a popup. Useful for testing a fix hypothesis without restarting.

13. Stop the debug session:
    ```
    SPC d q       → debug-stop
    ```

14. The AI can also participate in debugging:
    ```
    SPC a c
    ```
    In the AI chat:
    ```
    The program is stopped at line 42 in main.rs. What does the value of `conn_pool` tell us about the connection leak?
    ```
    The AI calls `debug_state` to read the current stack and variables and reasons over them.

### What to Verify

- Breakpoint gutter markers survive buffer saves and file reloads.
- Conditional breakpoints only pause execution when the condition evaluates to true.
- The call stack panel shows all frames; clicking a frame switches the variable view to that frame's scope.
- `debug-eval` evaluates in the selected frame's scope, not the top frame.
- Stopping the session clears all debug overlays (execution line marker, variable inline annotations).

### Key Features Used

- `debug-toggle-breakpoint` (SPC d b) with optional condition
- `debug-start` (SPC d s) — lldb-dap (Rust/C/C++) and debugpy (Python)
- `debug-continue` (SPC d c), `debug-step-over` (SPC d n), `debug-step-into` (SPC d i), `debug-step-out` (SPC d o)
- `debug-inspect` (SPC d v) with interactive variable expansion
- `debug-add-watch` (SPC d w), `debug-eval` (SPC d e)
- Debug panel UI: call stack, locals, watches, DAP output
- Gutter markers: breakpoints (red circle), execution line (yellow arrow), diagnostic severity
- AI `debug_state` tool for AI-assisted debugging

### Gaps vs Competitors

| Competitor | Gap |
|---|---|
| VSCode debugger | No inline variable value display next to code (hover-on-hover shows value; inline gutter annotations not yet implemented) |
| JetBrains IDEA | No "evaluate and set" (modify variable value during debug session) |
| Emacs / dap-mode | Comparable; MAE adds AI tool access to debug state as differentiator |
| Zed | Zed has no DAP integration at time of writing |

---

## 7. Developer — Org + Babel Notebooks

**Status:** Complete

### Persona

A developer or data scientist who writes literate documents in org-mode — mixing prose, code blocks in multiple languages, and their outputs — and needs to execute blocks, tangle source to files, and export finished documents.

### Goal

Author a literate programming document, execute embedded code blocks to verify results, tangle the source code to production files, and export the document to HTML for sharing.

### Workflow

1. Open or create an org-mode document:
   ```
   SPC f f       → open analysis.org
   ```

2. Write a prose section and insert a code block:
   ```org
   * Data Processing

   We load the dataset and compute summary statistics.

   #+begin_src python :results output
   import statistics
   data = [1, 4, 9, 16, 25]
   print(f"mean={statistics.mean(data)}, stdev={statistics.stdev(data):.2f}")
   #+end_src
   ```

3. Execute the block under the cursor:
   ```
   SPC m b e     → babel-execute (single block)
   ```
   MAE runs the block in an embedded shell session (or language REPL) and inserts a `#+RESULTS:` section below with the output.

4. Execute all blocks in the document in order:
   ```
   SPC m b a     → babel-execute-all
   ```

5. Kill all active babel sessions (to reset REPL state):
   ```
   SPC m b k     → babel-kill-sessions
   ```

6. Insert a Rust block with a tangle target:
   ```org
   #+begin_src rust :tangle src/lib.rs
   pub fn add(a: i32, b: i32) -> i32 { a + b }
   #+end_src
   ```

7. Tangle all blocks to their target files:
   ```
   SPC m b t     → babel-tangle
   ```
   MAE writes each block to its `:tangle` path. Output confirms each file written.

8. Export the document to HTML:
   ```
   SPC m e h     → org-export-html
   ```
   The HTML file is written to the same directory. Code blocks are syntax-highlighted; results are rendered inline.

9. Export a single subtree (not the full document):
   ```
   SPC m e s     → org-export-subtree
   ```
   MAE exports only the subtree containing the cursor.

10. Convert an org file to Markdown (for GitHub / docs site):
    ```
    :org-to-markdown
    ```

11. The AI can assist with notebook authoring:
    ```
    SPC a p
    ```
    Task: `Add error handling to the Python block in section 2 and re-execute it.`
    The AI reads the block, rewrites it, and calls `babel-execute` via the `babel_execute` tool.

### What to Verify

- `babel-execute` on a Python block with `:results output` inserts correct output under `#+RESULTS:` and replaces stale results on re-execution.
- `babel-tangle` writes only the blocks with `:tangle` headers and does not overwrite files not listed.
- After `babel-kill-sessions`, the next `babel-execute` starts a fresh interpreter (no stale REPL state).
- HTML export preserves org headings as `<h1>`–`<h6>`, code blocks as `<pre><code>` with language class, and inline markup (bold, italic, code spans).
- `org-export-subtree` respects the subtree boundary and does not include sibling headings.

### Key Features Used

- Org-mode syntax: headings, blocks, markup, links
- `babel-execute` (SPC m b e), `babel-execute-all` (SPC m b a), `babel-kill-sessions` (SPC m b k)
- `babel-tangle` (SPC m b t) with `:tangle` header arguments
- Org export: `org-export-html`, `org-export-markdown`, `org-export-subtree`, `org-to-markdown`
- AI `babel_execute` tool for AI-driven block execution
- Embedded shell sessions / language REPLs via `mae-shell` + `mae-babel`

### Gaps vs Competitors

| Competitor | Gap |
|---|---|
| Jupyter Notebook | No inline plot rendering in TUI backend; GUI backend renders PNG/SVG images but not live matplotlib output yet |
| Emacs org-babel | Comparable feature set; MAE adds AI as a first-class notebook participant |
| Observable | No reactive cell dependency graph; blocks execute imperatively |
| VS Code Jupyter | No org-mode source format; MAE is org-native, not `.ipynb` |

---

## 8. Power User — Scheme Extension

**Status:** Complete

### Persona

An experienced user (or MAE contributor) who wants to extend MAE's behavior beyond what the defaults provide — adding custom keybindings, writing hooks, defining new commands, and experimenting live in a Scheme REPL.

### Goal

Customize MAE's behavior at runtime via Scheme: bind new keys, add hooks that trigger on save or mode change, define a custom command, and hot-reload the config — all without restarting the editor.

### Workflow

1. Open the init.scm configuration file:
   ```
   :edit-config
   ```
   Opens `~/.config/mae/init.scm` in a buffer.

2. Inspect the available Scheme API (KB nodes for all ~50 primitives):
   ```
   :help scheme:define-key
   :help scheme:add-hook!
   :help scheme:buffer-insert
   ```

3. Add a custom keybinding (in init.scm or live in the REPL):
   ```scheme
   (define-key normal-map "SPC x d"
     (lambda ()
       (message "Today is %s" (format-time "%Y-%m-%d"))))
   ```

4. Add a named command and bind it:
   ```scheme
   (define-command "insert-date"
     "Insert today's date at point."
     (lambda ()
       (buffer-insert (format-time "%Y-%m-%d"))))

   (define-key insert-map (kbd "C-c d") 'insert-date)
   ```

5. Add a hook that runs after every save:
   ```scheme
   (add-hook! 'after-save-hook
     (lambda ()
       (when (string-suffix? ".rs" (buffer-file-name))
         (message "Rust file saved — remember to run tests!"))))
   ```

6. Add a hook that fires on mode transitions:
   ```scheme
   (add-hook! 'mode-change-hook
     (lambda (new-mode)
       (when (eq? new-mode 'insert)
         (set-option! "cursor_style" "bar"))))
   ```

7. Open the live Scheme REPL to experiment:
   ```
   :open-scheme-repl
   ```
   A REPL buffer opens. Expressions entered here execute in the live editor environment.

8. Evaluate a Scheme expression from the current buffer (or a selected region):
   ```
   SPC m e e     → eval-region   (visual selection)
   SPC m e b     → eval-buffer   (entire buffer)
   :eval (buffer-string)
   ```

9. Hot-reload the configuration without restarting:
   ```
   :reload-config
   ```
   MAE re-evaluates `init.scm`; newly defined keys and hooks take effect immediately.

10. Verify the configuration health:
    ```
    :describe-configuration
    ```
    A structured report lists all registered options, loaded modules, active hooks, and any warnings.

11. Use the `(mae!)` declarative block to select editor modules:
    ```scheme
    (mae!
      :keymap "doom"
      :modules '(git-status dailies collab))
    ```
    Only declared modules load; undeclared module bindings do not exist.

12. Inspect option values and set them at runtime:
    ```scheme
    (get-option "scrolloff")           ; => 8
    (set-option! "scrolloff" 4)        ; immediate effect
    ```
    Or via the ex command:
    ```
    :set scrolloff=4
    :set-save scrolloff=4   ; persists to config.toml
    ```

### What to Verify

- `(define-key normal-map ...)` takes effect immediately; the new binding works in the running editor without restart.
- `(add-hook! 'after-save-hook ...)` fires after every `:w` save and receives no arguments (the buffer context is implicit).
- `(add-hook! 'mode-change-hook ...)` fires with the new mode symbol on every mode transition.
- `:reload-config` re-evaluates init.scm and applies new bindings; previously defined hooks are not duplicated (hook registry is idempotent for named hooks).
- `:set-save scrolloff=4` writes to `config.toml` and survives a restart.
- `:describe-configuration` reports any option with an invalid value as a warning, not a silent failure.

### Key Features Used

- `init.scm` configuration (XDG-compliant: `~/.config/mae/init.scm`)
- `(define-key MAP KEY CMD)` — runtime-redefinable keybindings
- `(define-command NAME DOC LAMBDA)` — custom command registration
- `(add-hook! HOOK LAMBDA)` — 25 hook points
- `:open-scheme-repl` — live R7RS-small REPL in a MAE buffer
- `eval-region` / `eval-buffer` / `:eval` — evaluate Scheme in context
- `:reload-config` — hot-reload without restart
- `:describe-configuration` — structured config health report
- `(mae!)` declarative module block
- `(get-option ...)` / `(set-option! ...)` — OptionRegistry Scheme API
- `:set` / `:set-save` — ex command interface to options

### Gaps vs Competitors

| Competitor | Gap |
|---|---|
| Emacs / elisp | Emacs has decades of existing packages; MAE's module ecosystem is nascent |
| Neovim / Lua | Lua is more familiar to most developers than Scheme |
| Zed / extensions | Zed uses WASM extensions with a narrow API; MAE gives full Scheme access to all primitives |
| Helix | Helix has no extension language at all; MAE is strictly more capable here |

---

## Summary Table

| # | Story | Persona | Status | Key Differentiator |
|---|---|---|---|---|
| 1 | Multi-Project Git | Solo Rust dev | Complete | Magit-style hunk staging + project switching |
| 2 | AI-Assisted Feature Dev | Developer + AI | Complete | Tool-calling AI with diff-overlay review |
| 3 | Personal KB | Researcher / writer | Complete | AI-queryable org-mode graph with FTS5 |
| 4 | Shared KB + AI | Engineering team | Complete (v0.11.0) | CRDT KB sync + mDNS discovery + AI over shared KB |
| 5 | LSP Code Navigation | Large codebase dev | Complete | AI-aware LSP (AI can call LSP tools as a peer) |
| 6 | DAP Debug Session | Rust / Python dev | Complete | AI reads live debug state for hypothesis testing |
| 7 | Org + Babel Notebooks | Literate programmer | Complete | AI as notebook participant (execute / rewrite blocks) |
| 8 | Scheme Extension | Power user / contributor | Complete | Full R7RS-small runtime, live REPL, hot reload |
