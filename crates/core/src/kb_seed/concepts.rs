pub(super) const CONCEPT_GIT_STATUS: &str = "\
The **Git Status** buffer (`*git-status*`) is a high-fidelity \"porcelain\" UI \
inspired by Emacs Magit. It allows you to manage your repository state \
without leaving the editor.\n\n\
## Multi-Level Fold\n\
Press `TAB` on a section header, file entry, or hunk header to fold/unfold \
that level independently. Collapse indicators (`▸`/`▾`) show fold state.\n\n\
## Keybindings\n\
| Key | Action | Command |\n\
|-----|--------|---------|\n\
| `s` | Stage (context-aware) | [[cmd:git-stage]] |\n\
| `u` | Unstage (context-aware) | [[cmd:git-unstage]] |\n\
| `x` | Discard (context-aware) | [[cmd:git-discard]] |\n\
| `S` | Stage ALL | [[cmd:git-stage-all]] |\n\
| `U` | Unstage ALL | [[cmd:git-unstage-all]] |\n\
| `c c` | Commit | [[cmd:git-commit]] |\n\
| `c a` | Amend | [[cmd:git-amend]] |\n\
| `l l` | Log view | [[cmd:git-log]] |\n\
| `g r` | Refresh | [[cmd:git-status]] |\n\
| `TAB` | Toggle fold (section/file/hunk) | [[cmd:git-toggle-fold]] |\n\
| `n` / `p` | Next/prev hunk | [[cmd:git-next-hunk]] / [[cmd:git-prev-hunk]] |\n\
| `P p` | Push | [[cmd:git-push]] |\n\
| `F p` | Pull | [[cmd:git-pull]] |\n\
| `f f` | Fetch | [[cmd:git-fetch]] |\n\
| `b b` | Switch branch | [[cmd:git-branch-switch]] |\n\
| `b n` | Create branch | [[cmd:git-branch-create]] |\n\
| `b d` | Delete branch | [[cmd:git-branch-delete]] |\n\
| `z z` | Stash push | [[cmd:git-stash-push]] |\n\
| `z p` | Stash pop | [[cmd:git-stash-pop]] |\n\
| `z a` | Stash apply | [[cmd:git-stash-apply]] |\n\
| `z d` | Stash drop | [[cmd:git-stash-drop]] |\n\
| `Enter` | Open file | [[cmd:git-status-open]] |\n\
| `q` | Exit | [[cmd:enter-normal-mode]] |\n\n\
## Context-Aware Dispatch\n\
`s`/`u`/`x` operate based on cursor position:\n\
- **On a diff hunk/line**: stage/unstage/discard that hunk.\n\
- **On a file entry**: stage/unstage/discard the whole file.\n\
- **On a section header**: stage/unstage all files in that section.\n\n\
## Inline Diff\n\
Press `TAB` on a file entry to expand/collapse its inline diff. Each hunk \
can be further folded independently.\n\n\
## Workflow\n\
1. Open status via `SPC g s`.\n\
2. Navigate with `j`/`k`, jump hunks with `n`/`p`.\n\
3. Stage files/hunks with `s`.\n\
4. Commit with `c c` (opens a commit message buffer).\n\n\
See also: [[concept:project]], [[concept:terminal]], [[concept:buffer-mode]]\n";

pub(super) const CONCEPT_ORG_MODE: &str = "\
**Org-mode** in MAE provides structural editing and task management \
capabilities for `.org` files, inspired by Doom Emacs evil-org.\n\n\
## Core Features\n\n\
### 1. Three-State Heading Cycle (TAB)\n\
Press `TAB` on a heading to cycle its visibility through three states:\n\
**Subtree** (everything visible) -> **Folded** (heading only) -> \
**Children** (body + child headings visible, child bodies folded) -> **Subtree**.\n\
Leaf headings (no children) toggle between **Subtree** and **Folded**.\n\n\
### 2. Fold All / Unfold All\n\
- `zM` (`close-all-folds`): Fold all headings in the buffer.\n\
- `zR` (`open-all-folds`): Unfold all headings.\n\
- `za`: Toggle fold at cursor (tree-sitter or heading).\n\n\
### 3. Structural Editing\n\
- `M-h` / `M-Left`: **Promote** heading (remove one `*` prefix).\n\
- `M-l` / `M-Right`: **Demote** heading (add one `*` prefix).\n\
- `M-k` / `M-Up`: **Move subtree up** past previous sibling.\n\
- `M-j` / `M-Down`: **Move subtree down** past next sibling.\n\
Moving a subtree automatically clears any folds in the affected range.\n\n\
### 4. Narrow / Widen\n\
- `SPC m s n` (`org-narrow-subtree`): Narrow buffer to current heading's subtree. \
Only lines in that subtree are visible; cursor is clamped to the range. \
Status bar shows `[Narrowed]`.\n\
- `SPC m s w` (`org-widen`): Restore full buffer visibility.\n\n\
### 5. Heading Font Scaling\n\
Org headings render at scaled font sizes for visual hierarchy:\n\
`*` = 1.5x, `**` = 1.3x, `***` = 1.15x.\n\
Disable with `:set heading_scale false`.\n\n\
### 6. Insert Heading (M-Enter)\n\
- On a heading line: Insert a new heading at the **same level** after the current subtree.\n\
- Not on a heading: Insert a level-1 heading below the current line.\n\
- Automatically enters insert mode with cursor after the heading prefix.\n\n\
### 7. Task Management\n\
- `S-Left` / `S-Right`: Cycle TODO states (`TODO` -> `DONE` -> `None`).\n\
- `S-Up` / `S-Down`: Cycle priorities (`[#A]` -> `[#B]` -> `[#C]`).\n\n\
### 8. Links\n\
Press `Enter` on a `[[link]]` to follow it. Internal links jump to headings; \
external links open in your browser.\n\n\
### 9. Rich Rendering\n\
- `*bold*` text is rendered in bold.\n\
- `/italic/` text is rendered in italics.\n\
- **Emphasis Markers**: Use `:set org_hide_emphasis_markers true` to hide \
the surrounding `*` and `/` characters.\n\n\
See also: [[concept:markdown]], [[concept:knowledge-base]], [[concept:options]]\n";

pub(super) const CONCEPT_MARKDOWN: &str = "\
**Markdown** in MAE provides structural editing for `.md` files, \
with the same UX as [[concept:org-mode|org-mode]] adapted for `#` headings.\n\n\
## Core Features\n\n\
### 1. Three-State Heading Cycle (TAB)\n\
Press `TAB` on a heading to cycle its visibility:\n\
**Subtree** (everything visible) -> **Folded** (heading only) -> \
**Children** (body + child headings visible, child bodies folded) -> **Subtree**.\n\
Leaf headings (no children) toggle between **Subtree** and **Folded**.\n\n\
### 2. Fold All / Unfold All\n\
- `zM` (`close-all-folds`): Fold all headings in the buffer.\n\
- `zR` (`open-all-folds`): Unfold all headings.\n\n\
### 3. Structural Editing\n\
- `M-h` / `M-Left`: **Promote** heading (remove one `#` prefix).\n\
- `M-l` / `M-Right`: **Demote** heading (add one `#` prefix).\n\
- `M-k` / `M-Up`: **Move subtree up** past previous sibling.\n\
- `M-j` / `M-Down`: **Move subtree down** past next sibling.\n\n\
### 4. Narrow / Widen\n\
- `SPC m s n` (`md-narrow-subtree`): Narrow buffer to current heading's subtree.\n\
- `SPC m s w` (`md-widen`): Restore full buffer visibility.\n\n\
### 5. Insert Heading (M-Enter)\n\
- On a heading line: Insert a new heading at the **same level** after the current subtree.\n\
- Not on a heading: Insert a level-1 heading below the current line.\n\
- Automatically enters insert mode with cursor after the heading prefix.\n\n\
### 6. Heading Font Scaling\n\
Markdown headings render at scaled font sizes:\n\
`#` = 1.5x, `##` = 1.3x, `###` = 1.15x.\n\
Disable with `:set heading_scale false`.\n\n\
### 7. Markdown Keymap\n\
The `markdown` keymap activates automatically for `.md` files and falls back \
to the `normal` keymap for unbound keys. All structural editing keys mirror \
the org-mode keymap.\n\n\
See also: [[concept:org-mode]], [[concept:options]]\n";

pub(super) const CONCEPT_EX_COMMANDS: &str = "\
**Ex-command grammar** for write/quit compound commands.\n\n\
MAE parses `:w`, `:q`, `:x` commands using a token grammar rather than \
hardcoded match arms. This means all valid vim compound forms work \
automatically.\n\n\
## Grammar\n\n\
**Verbs:** `w` (write), `q` (quit), `x` (write-if-modified + quit)\n\
**Modifiers:** `a` (all — applies to preceding verb), `!` (force, must be terminal)\n\n\
## Valid Combinations\n\n\
| Command | Effect |\n\
|---------|--------|\n\
| `:w`    | Write current buffer |\n\
| `:wa`   | Write all buffers |\n\
| `:q`    | Quit (check modified) |\n\
| `:q!`   | Quit (force, discard changes) |\n\
| `:qa`   | Quit all |\n\
| `:qa!`  | Force quit all |\n\
| `:wq`   | Write + quit |\n\
| `:wq!`  | Write + force quit |\n\
| `:wqa`  | Write all + quit all |\n\
| `:wqa!` | Write all + force quit all |\n\
| `:x`    | Write-if-modified + quit |\n\
| `:xa`   | Write-if-modified all + quit all |\n\
| `:xa!`  | Write-if-modified all + force quit all |\n\n\
## Implementation\n\
The tokenizer lives in `crates/core/src/editor/ex_parse.rs`. \
`parse_write_quit()` returns `Option<Vec<ExWriteQuit>>` — None for non-matching \
commands, Some for valid compound commands.\n\n\
See also: [[concept:command]], [[concept:options]]\n";

pub(super) const CONCEPT_SET_SYNTAX: &str = "\
**`:set` option syntax** — vim-style option configuration.\n\n\
## Syntax Forms\n\n\
| Syntax | Effect |\n\
|--------|--------|\n\
| `:set option` | Enable (bool) or query (non-bool) |\n\
| `:set nooption` | Disable bool option |\n\
| `:set option!` | Toggle bool option |\n\
| `:set option?` | Query current value |\n\
| `:set option value` | Assign value |\n\
| `:set option \"value with spaces\"` | Quoted value |\n\n\
## Tab Completion\n\n\
- `:set <Tab>` completes option names\n\
- `:set option <Tab>` completes values:\n\
  - Bool options: `true`, `false`\n\
  - Enum options: cycles through valid values\n\
  - Theme options: lists bundled themes\n\n\
## Implementation\n\
The parser lives in `crates/core/src/editor/ex_parse.rs` (`parse_set_args()`). \
Value completion is in `crates/core/src/editor/file_ops.rs` (`complete_set_value()`).\n\n\
See also: [[concept:options]], [[concept:command]]\n";

pub(super) const CONCEPT_SCROLLBAR: &str = "\
**Vertical scrollbar** for the GUI rendering backend.\n\n\
## Configuration\n\
- `:set scrollbar true` (default: enabled)\n\
- `:set scrollbar false` to disable\n\n\
## Layout\n\
The scrollbar occupies 1 column at the right edge of the text area. \
Space is allocated in `FrameLayout::compute_layout()` *before* wrap/layout \
computation, so text wrapping respects the reduced width.\n\n\
## Rendering\n\
- **Track**: full content-area height, theme color `ui.scrollbar.track`\n\
- **Thumb**: proportional to viewport/total ratio, theme color `ui.scrollbar.thumb`\n\
- Minimum thumb height: 1 cell\n\n\
## Mouse Interaction\n\
Click in the scrollbar column to jump to that scroll position.\n\n\
## Nyan Mode\n\
`:set nyan_mode true` enables a rainbow progress bar in the status line, \
showing scroll position as a filled bar with a cat marker.\n\n\
See also: [[concept:gui]], [[concept:options]]\n";

pub(super) const CONCEPT_AUTOSAVE: &str = "\
**Autosave** periodically saves all modified file-backed buffers in the background.\n\n\
## Configuration\n\
- `:set autosave_interval 300` — save every 5 minutes (0 = disabled)\n\
- `config.toml`: `autosave_interval = 300` under `[editor]`\n\
- Scheme: `(set-option! \"autosave-interval\" \"300\")`\n\n\
## Idle Debounce\n\
Autosave waits at least **5 seconds** after the last edit before saving. \
This prevents saving mid-typing. The timer resets with each keystroke.\n\n\
## Behavior\n\
- Only file-backed buffers (not scratch, conversation, or shell) are saved.\n\
- Status bar shows \"Autosaved N buffer(s)\" on each save.\n\
- Errors are reported but don't interrupt editing.\n\n\
See also: [[concept:options]], [[cmd:save]]\n";

pub(super) const CONCEPT_FILE_TREE: &str = "\
**File Tree** is a sidebar showing the project directory structure with file-type icons.\n\n\
## Keybindings\n\
| Key | Action |\n\
|---|---|\n\
| `SPC f t` | Toggle file tree sidebar |\n\
| `j` / `k` | Navigate entries |\n\
| `Enter` | Open file / toggle directory |\n\
| `o` | Toggle expand/collapse directory |\n\
| `R` | Refresh tree from disk |\n\
| `q` | Close file tree |\n\n\
## Project Root\n\
The tree roots at the detected project root (`.git`, `Cargo.toml`, etc.). \
Falls back to the current working directory.\n\n\
## Icons\n\
File type icons are Unicode emoji by default (no font dependency):\n\
- Directories: open/closed folder\n\
- `.rs` → crab, `.py` → snake, `.js` → lightning, `.toml`/`.json` → gear\n\n\
## Filtering\n\
Build artifacts and VCS directories (`target/`, `node_modules/`, `.git/`) \
are hidden automatically.\n\n\
See also: [[cmd:find-file]], [[concept:buffer]], [[concept:project]]\n";

pub(super) const CONCEPT_DIFF_DISPLAY: &str = "\
**Diff Display** renders unified diffs with syntax-highlighted lines.\n\n\
## Flow\n\
1. AI calls `propose_changes` tool with edits\n\
2. MAE computes a unified diff (LCS-based) between old and new content\n\
3. The diff is displayed in the `*AI-Diff*` buffer\n\
4. Lines are colored by type:\n\
   - `+` lines → `diff.added` (green)\n\
   - `-` lines → `diff.removed` (red)\n\
   - `@@` headers → `diff.hunk` (magenta)\n\
   - `---`/`+++` headers → `diff.header` (cyan, bold)\n\n\
## Commands\n\
- `:ai-accept` — apply the proposed changes\n\
- `:ai-reject` — discard the proposed changes\n\n\
## Theme Keys\n\
All 8 bundled themes include `diff.added`, `diff.removed`, `diff.hunk`, \
and `diff.header` style definitions.\n\n\
See also: [[concept:ai-as-peer]], [[concept:options]]\n";

pub(super) const CONCEPT_BUFFER_MODE: &str = "\
The **BufferMode** trait (`buffer_mode.rs`) is the contract every buffer kind implements. \
It replaces scattered `match buf.kind` blocks with polymorphic dispatch.\n\n\
## Methods\n\
| Method | Purpose |\n\
|--------|---------|\n\
| `mode_name()` | Display name for the status bar |\n\
| `keymap_name()` | Overlay keymap name (e.g. `git-status`, `help`) |\n\
| `read_only()` | Whether inserts are blocked |\n\
| `default_word_wrap()` | Whether word-wrap defaults to on |\n\
| `has_gutter()` | Whether line numbers render |\n\
| `status_hint()` | One-line discoverability text on mode entry |\n\
| `mode_theme_key()` | Status-bar mode indicator color |\n\
| `insert_mode()` | Which insert mode to enter (Insert vs ShellInsert) |\n\n\
`BufferKind` implements `BufferMode`. New buffer types add trait arms, not scattered matches.\n\n\
See also: [[concept:buffer]], [[concept:mode]], [[concept:keymap-inheritance]]\n";

pub(super) const CONCEPT_BUFFER_VIEW: &str = "\
The **BufferView** enum (`buffer_view.rs`) stores mode-specific state on `Buffer`. \
Variants: `Conversation`, `Help`, `Debug`, `GitStatus`, `Visual`, `FileTree`, `None`.\n\n\
Accessor methods: `buf.conversation()`, `buf.help_view()`, `buf.git_status_view()`, etc. \
Each returns `Option<&T>` (or `Option<&mut T>` for the `_mut` variant).\n\n\
This replaced 6 `Option<T>` fields that were always mutually exclusive.\n\n\
See also: [[concept:buffer]], [[concept:buffer-mode]]\n";

pub(super) const CONCEPT_KEYMAP_INHERITANCE: &str = "\
**Keymap inheritance** lets buffer-kind overlay keymaps (git-status, help, debug, file-tree) \
inherit bindings from a parent keymap.\n\n\
## Mechanism\n\
- `Keymap` has a `parent: Option<String>` field.\n\
- Key lookup: overlay keymap -> parent -> fallback.\n\
- `which_key_entries_for_current_keymap()` merges overlay + parent entries for the which-key popup.\n\n\
## Scheme API\n\
`(define-keymap \"name\" \"parent\")` creates a keymap with inheritance.\n\n\
## Current Overlay Keymaps\n\
| Keymap | Parent | Buffer Kind |\n\
|--------|--------|-------------|\n\
| `git-status` | `normal` | GitStatus |\n\
| `help` | `normal` | Help |\n\
| `debug` | `normal` | Debug |\n\
| `file-tree` | `normal` | FileTree |\n\n\
See also: [[concept:mode]], [[concept:buffer-mode]]\n";

pub(super) const CONCEPT_PROMPT_TIERS: &str = "\
## Prompt Tiers\n\n\
MAE uses tiered system prompts to optimize AI agent behavior for different models.\n\n\
### Full Tier\n\
Concise prompt (~25 lines) for frontier models with strong implicit reasoning:\n\
- Claude Opus, Claude Sonnet, GPT-4o, GPT-4 Turbo, Gemini 2.5 Pro, o1\n\n\
### Compact Tier\n\
Explicit guardrails (~70 lines) for smaller/cheaper models:\n\
- DeepSeek, Claude Haiku, GPT-4o-mini, Gemini Flash, o1-mini\n\
- Includes: tool preferences, fallback chains, anti-looping rules, common recipes\n\n\
### Default Assignments\n\
Unknown models default to **compact** (safe: over-prompting wastes a few tokens; \
under-prompting wastes millions in looping).\n\n\
### Override\n\
Set `[ai] prompt_tier = \"full\"` or `\"compact\"` in `config.toml` to force a tier \
regardless of model.\n\n\
### Custom Prompts\n\
Place `pair-programmer.xml` or `pair-programmer-compact.xml` in:\n\
- `~/.config/mae/prompts/` (user override)\n\
- `.mae/prompts/` (project-local override)\n\n\
See also: [[concept:ai-modes|AI Agent vs Chat]], [[concept:ai-as-peer|AI as Peer]]\n";

pub(super) const CONCEPT_DISPLAY_POLICY: &str = "\
## Display Policy\n\n\
Controls how buffers become visible in windows — the O(1) enum-dispatch replacement \
for Emacs's 29 `display-buffer-*` functions and regex alist.\n\n\
### The Problem\n\
Five direct `focused_window_mut().buffer_idx` calls (help, messages, debug, git-status, \
file-tree) had zero conversation awareness. If the AI agent called `help_open` while \
focused on the tiny AI input pane, the help buffer got crammed in and the conversation \
layout was destroyed.\n\n\
### The 4 Actions (vs Emacs's 29)\n\
- **ReplaceFocused** — replace the focused window, but fall through to AvoidConversation \
if focused on a conversation buffer (git-status, dashboard)\n\
- **AvoidConversation** — route via `switch_to_buffer_non_conversation` which has a \
3-step strategy protecting conversation pairs (text, diff)\n\
- **ReuseOrSplit** — reuse an existing window of the same BufferKind, or create a split \
with the given direction and ratio (help → 50% vsplit, messages → 30% hsplit)\n\
- **Hidden** — buffer exists for programmatic access only, never shown (conversation — \
managed by `open_conversation_buffer`)\n\n\
### Default Rules\n\
| Kind        | Action               | Rationale                   |\n\
| Text        | AvoidConversation    | Normal files never invade   |\n\
| Help        | ReuseOrSplit V 50%   | Reuse help window or vsplit |\n\
| Messages    | ReuseOrSplit H 30%   | Bottom 30%, reuse if open   |\n\
| Debug       | ReuseOrSplit H 40%   | Bottom 40%                  |\n\
| GitStatus   | ReplaceFocused       | Full window (Magit style)   |\n\
| Conversation| Hidden               | Managed internally          |\n\n\
### Customization\n\
From init.scm: `(set-display-rule! \"help\" \"reuse-or-split:vertical:0.5\")`\n\
Inspect: `(display-buffer-policy \"help\")` or `SPC h D` ([[cmd:describe-display-policy]])\n\n\
### Emacs Comparison\n\
Emacs: `display-buffer-alist` (29 action functions, regex matching, order-dependent). \
Doom: `set-popup-rules!` (simpler but still regex). MAE: enum dispatch by BufferKind — \
O(1), no order bugs, no regex.\n\n\
See also: [[concept:buffer]], [[concept:window]], [[concept:buffer-mode]]\n";

pub(super) const CONCEPT_MCP_DEVELOPMENT: &str = "\
## MCP Development Workflow\n\n\
All 130+ MAE tools are exposed via MCP with full parity. When developing MAE \
inside MAE (e.g. with Claude Code via `mae-mcp-shim`), these tools provide \
structured access to LSP, DAP, KB, and editor state.\n\n\
### Tool Categories\n\
- **Code Navigation (LSP):** `lsp_definition`, `lsp_references`, `lsp_hover`, \
`lsp_workspace_symbol`, `lsp_document_symbols`, `lsp_diagnostics`\n\
- **Debugging (DAP):** `dap_start`, `dap_set_breakpoint`, `dap_continue`, \
`dap_step`, `debug_state`\n\
- **Knowledge Base:** `kb_search`, `kb_get`, `kb_links_from`, `kb_links_to`, `kb_graph`\n\
- **Buffer/Editor:** `buffer_read`, `buffer_write`, `project_search`, \
`command_list`, `execute_command`, `eval_scheme`, `audit_configuration`, `introspect`\n\
- **Validation:** `self_test_suite` — structured JSON test plan\n\n\
### Connection\n\
Socket: `/tmp/mae-{PID}.sock`\n\
Shim: `mae-mcp-shim` (stdio ↔ Unix socket bridge)\n\n\
### When to Use\n\
- **Navigating MAE code:** `lsp_definition` / `lsp_references` over grep — structured, no false positives\n\
- **Understanding architecture:** `kb_search` or `kb_get` — curated docs\n\
- **Debugging MAE:** `dap_start` with `lldb-dap`, `debug_state` for inspection\n\
- **Testing changes:** `execute_command`, `self_test_suite` for E2E\n\n\
See also: [[concept:ai-as-peer]], [[concept:agent-bootstrap]], [[concept:self-test]]\n";

pub(super) const INDEX_BODY: &str = "Welcome to MAE's built-in help. This knowledge base is the same data \
surface the AI agent queries via its `kb_*` tools — you and the AI read the same pages.

## Core concepts
- [[concept:buffer|Buffer]] — the unit of editable content
- [[concept:window|Window]] — a view onto a buffer
- [[concept:mode|Mode]] — which keymap is active
- [[concept:command|Command]] — the shared API between human, Scheme, and AI
- [[concept:ai-as-peer|The AI as Peer Actor]] — the fundamental design stance
- [[concept:knowledge-base|Knowledge Base]] — this page, and why it exists
- [[concept:terminal|Embedded Terminal]] — full terminal emulator inside MAE + MCP bridge
- [[concept:hooks|Hooks]] — Scheme extension points for editor events
- [[concept:options|Editor Options]] — configuring MAE from Scheme
- [[concept:agent-bootstrap|Agent Bootstrap]] — zero-config MCP tool discovery for AI agents
- [[concept:self-test|AI Self-Test]] — validate editor tools and integrations via `:self-test`
- [[concept:debugging|Debugging (DAP)]] — DAP client, debug panel, breakpoints, AI debug tools
- [[concept:watchdog|Watchdog]] — event loop stall detection and thread dumps\n\
- [[concept:event-recording|Event Recording]] — session capture and JSON export\n\
- [[concept:dap-attach|DAP Attach]] — cross-instance debugging with PID\n\
- [[concept:introspect|Introspect]] — AI diagnostic snapshot (threads/perf/locks/buffers)
- [[concept:gui|GUI Backend]] — dual rendering (terminal + GUI), mouse, font config
- [[concept:git-status|Git Status]] — Magit-lite porcelain UI
- [[concept:org-mode|Org-mode]] — Structural editing, folding, narrowing, and task management\n\
- [[concept:markdown|Markdown]] — Structural editing parity with org-mode for `#` headings\n\
- [[concept:ex-commands|Ex-Command Grammar]] — Tokenizer for w/q/x compound commands\n\
- [[concept:set-syntax|:set Syntax]] — Vim-style option configuration (no-prefix, toggle, query)\n\
- [[concept:autosave|Autosave]] — interval-based background save with idle debounce\n\
- [[concept:file-tree|File Tree]] — project sidebar with icons and directory expansion\n\
- [[concept:diff-display|Diff Display]] — syntax-highlighted unified diffs for AI changes\n\
- [[concept:scrollbar|Scrollbar]] — Vertical scrollbar and nyan mode\n\
- [[concept:conceal|Link & Markup Rendering]] — Descriptive links and inline styling\n\
- [[concept:buffer-mode|BufferMode Trait]] — the contract every buffer kind implements\n\
- [[concept:buffer-view|BufferView Enum]] — mode-specific state on Buffer\n\
- [[concept:keymap-inheritance|Keymap Inheritance]] — overlay keymaps with parent fallback\n\
- [[concept:package-system|Package System]] — require/provide for Scheme extensions\n\
- [[concept:option-registry|Option Registry]] — single source of truth for editor settings\n\
- [[concept:scheme-api|Scheme API]] — ~50 functions for buffer/window/command/keymap access\n\
- [[concept:ai-modes|AI Agent vs Chat]] — when to use each AI interface\n\
- [[concept:prompt-tiers|Prompt Tiers]] — model-aware prompt selection (full vs compact)\n\
- [[concept:display-policy|Display Policy]] — how buffers are placed in windows (4 actions, O(1) dispatch)

## Reference
- [[key:normal-mode|Normal-mode keys]]
- [[key:leader-keys|SPC leader bindings]] (14 groups, Doom Emacs style)
- [[concept:project|Project management]]
- Commands: run `:command-list` for the full list, or visit any `cmd:<name>` node.
- Browse by category: `category:movement`, `category:editing`, `category:git`, etc.

## Tutorial
- [[tutorial:getting-started|Getting Started]] — progressive guide (Vim track / Beginner track / AI setup)
- [[tutor:index|Lesson-style Tutorial]] — 12 focused lessons covering all essentials

## Getting around
- **Enter** on a link follows it.
- **C-o** goes back, **C-i** goes forward (history, like vim jumps).
- **q** closes the help buffer.
";

pub(super) const CONCEPT_BUFFER: &str = "A **buffer** is the unit of editable content in MAE.\n\
It has an optional file path, a kind (BufferKind), modification \
state, and either a rope (for text) or a structured payload (for conversations, help, etc).\n\n\
## Contrast with other editors\n\
- **Emacs buffer** ≈ MAE buffer (same lineage).\n\
- **Vim buffer** ≈ MAE buffer, but MAE does not have Vim's separate *tabs* or *windows-per-tab* concept.\n\
- **VSCode tab** is a UI affordance — MAE exposes no such primitive.\n\n\
## What buffers do NOT own\n\
Cursor position lives on [[concept:window|Window]], not on the buffer. Two windows can \
view the same buffer at different points — the design is deliberately Emacs-shaped here.\n\n\
## Display Policy\n\
How a buffer becomes visible is governed by the [[concept:display-policy|Display Policy]], \
which maps each BufferKind to a DisplayAction (replace, avoid conversation, reuse/split, hidden).\n\n\
See also: [[concept:window]], [[concept:command]], [[cmd:list-buffers]], [[concept:display-policy]]\n";

pub(super) const CONCEPT_WINDOW: &str =
    "A **window** is a rectangular view onto a [[concept:buffer|buffer]]. \
MAE's tiling WindowManager owns the layout tree (splits, sizes) \
and exactly one window is focused at a time.\n\n\
## Why cursor state lives here, not on the buffer\n\
Emacs has taught us that two windows can legitimately view the same buffer at different \
points. If cursor state lived on the buffer, this would be impossible without extra hacks. \
MAE inherits this shape.\n\n\
## What MAE windows are NOT\n\
- NOT an OS-level window (Emacs's terminology for that is a \"frame\" — MAE has no frames).\n\
- NOT a tab (MAE has no tabs).\n\n\
See also: [[concept:buffer]], [[concept:mode]]\n";

pub(super) const CONCEPT_MODE: &str = "MAE is **modal** like Vim. The current [[concept:mode|Mode]] \
determines which keymap is active.\n\n\
## Modes\n\
- **Normal** — movement and commands (default).\n\
- **Insert** — literal text entry.\n\
- **Visual(Char|Line)** — selection.\n\
- **Command** — `:command` line.\n\
- **Search** — `/` incremental search.\n\
- **ConversationInput** — typing into the AI prompt.\n\
- **FilePicker** — fuzzy file open overlay.\n\
- **ShellInsert** — raw keyboard passthrough to [[concept:terminal|embedded terminal]].\n\n\
Mode transitions are commands — see [[cmd:enter-normal-mode]], [[cmd:enter-insert-mode]], \
[[cmd:enter-command-mode]]. The AI agent can trigger them too (that's the point of [[concept:ai-as-peer]]).\n\n\
See also: [[key:normal-mode]]\n";

pub(super) const CONCEPT_COMMAND: &str =
    "A **command** is a named, documented operation with a stable string identifier. \
Commands are registered in a shared CommandRegistry and can \
be triggered from three peer surfaces:\n\n\
1. **Human** — via keybindings (`:command-list` or `SPC SPC`).\n\
2. **Scheme** — via `(execute-command \"name\")` from config or packages.\n\
3. **AI agent** — each command is exposed as a tool-call; the agent sees the same doc \
the human sees on this page.\n\n\
This is the *entire* reason MAE has the ergonomics it has — there is exactly one API and \
it has three callers.\n\n\
See also: [[concept:ai-as-peer]], [[cmd:command-list]]\n";

pub(super) const CONCEPT_AI_AS_PEER: &str = "MAE's single-most-important design stance: **the AI agent is a peer actor, not a plugin.**\n\n\
A keybinding and an AI tool-call both resolve to the same [[concept:command|Command]] \
via the same dispatcher. There is no separate \"AI mode\", no simulated keystrokes, no \
shadow API. When you type `dd` to delete a line, the agent can invoke `cmd:delete-line` \
with the same effect, and vice versa.\n\n\
## What the agent can see\n\
- [[cmd:buffer-read|Buffer contents]] ([[cmd:list-buffers|across all buffers]]).\n\
- [[cmd:cursor-info|Cursor state]] and [[cmd:editor-state|editor state]].\n\
- [[cmd:lsp-diagnostics|LSP diagnostics]] and [[cmd:syntax-tree|tree-sitter parse trees]].\n\
- [[cmd:debug-state|DAP debug state]] when a session is active.\n\
- This knowledge base (`kb_get`, `kb_search`, `kb_list`).\n\
- [[concept:project|Project state]] via `project_info`, `project_files`, `project_search`.\n\
- **[[concept:introspect|Introspection]]**: The agent can see thread stacks, performance counters, and lock contention.\n\n\
## Interaction Surfaces\n\
1. **Internal Peer**: Embedded directly in MAE, sharing your active workspace context. Trigger via `SPC a p`.\n\
2. **External Agent**: Any MCP-capable client (like Gemini CLI or Claude Code) can connect to MAE via the `mae-mcp-shim`. The external agent gains full control of the editor's tool surface.\n\n\
## Permission tiers\n\
Every tool has a permission tier: ReadOnly, Write, Shell, \
Privileged. Users control how far the agent can act autonomously.\n\n\
See also: [[concept:knowledge-base]], [[concept:command]], [[concept:agent-bootstrap]]\n";

pub(super) const CONCEPT_KB: &str = "MAE's **knowledge base** is a typed graph of nodes with \
bidirectional link markers. It started as the help system's backing store and is \
designed to grow into an org-roam-equivalent personal knowledge graph.\n\n\
## Why one system for both?\n\
Help pages, keybinding docs, architectural essays, user notes, and AI-authored findings \
all want the same three properties:\n\
1. Addressable (stable id).\n\
2. Linkable (`[[other-node]]`).\n\
3. Queryable by a peer (the AI gets the same query surface the human does).\n\n\
## Node namespaces\n\
- `index` — the entry page.\n\
- `cmd:<name>` — one per registered [[concept:command|Command]] (auto-generated).\n\
- `concept:<slug>` — architectural concepts (hand-authored).\n\
- `key:<context>` — keybinding summaries.\n\
- (Future) `note:<slug>` — user notes; `file:<path>` — per-file AI notes.\n\n\
## AI surface\n\
The agent reaches the KB via the `kb_get`, `kb_search`, `kb_list`, `kb_links_from`, and \
`kb_links_to` tools. Same nodes the human reads via `:help`.\n\n\
See also: [[concept:ai-as-peer]], [[index]]\n";

pub(super) const CONCEPT_PROJECT: &str =
    "A **project** in MAE is a directory with optional `.project` TOML configuration.\n\n\
## Detection\n\
When you open a file, MAE walks upward from its directory looking for marker files:\n\
`.project` > `.git` > `Cargo.toml` > `package.json` > `go.mod` > `pyproject.toml` > `Makefile`.\n\
The first match becomes the project root.\n\n\
## .project TOML\n\
Optional declarative config:\n\
```toml\n\
name = \"My Project\"\n\
root-directory = \"~/src/my-project\"\n\
required-resources = [\"README.org\", \"Cargo.toml\"]\n\
```\n\n\
## SPC p commands\n\
- `SPC p f` — find file in project ([[cmd:project-find-file]])\n\
- `SPC p s` — search in project ([[cmd:project-search]])\n\
- `SPC p d` — browse project directory ([[cmd:project-browse]])\n\
- `SPC p r` — recent project files ([[cmd:project-recent-files]])\n\n\
## AI integration\n\
The AI agent can query project state via the `project_info` tool and \
search project files via `project_files` and `project_search`.\n\n\
See also: [[index]], [[concept:ai-as-peer]]\n";

pub(super) const CONCEPT_TERMINAL: &str =
    "MAE embeds a full **terminal emulator** backed by `alacritty_terminal`, the same \
engine that powers the Alacritty terminal. Programs like vim, less, top, fzf, and tmux \
work correctly — this is not a line-oriented shell like eshell.\n\n\
## Opening a terminal\n\
- `:terminal` or `SPC o t` — opens a new `*Terminal*` buffer in ShellInsert mode.\n\
- The terminal runs the user's `$SHELL` in a PTY.\n\n\
## Modes\n\
- **ShellInsert** — all keys go directly to the PTY. The terminal is fully interactive.\n\
- **Normal** — `Ctrl-\\ Ctrl-n` exits ShellInsert → Normal mode (Neovim convention). \
You can then use leader keys (`SPC`), window commands, etc.\n\
- Press `i` or `a` to re-enter ShellInsert from Normal mode on a terminal buffer.\n\n\
## Commands\n\
- [[cmd:terminal]] — open a new terminal buffer.\n\
- [[cmd:terminal-reset]] (`SPC o r`) — reset/clear the terminal (fixes residual \
characters from programs like cmatrix that don't clean up on exit).\n\
- [[cmd:terminal-close]] (`SPC o c`) — close the terminal and kill the shell process.\n\
- [[cmd:send-to-shell]] (`SPC e s`) — send current line to a terminal.\n\
- [[cmd:send-region-to-shell]] (`SPC e S`) — send visual selection to a terminal.\n\n\
## Scheme integration\n\
- `(shell-cwd BUF-IDX)` — returns the CWD of a shell buffer (via `/proc/PID/cwd`).\n\
- `(shell-read-output BUF-IDX MAX-LINES)` — reads the last N lines of terminal output.\n\
- `*shell-buffers*` — list of buffer indices that are Shell-kind.\n\n\
## MCP bridge\n\
MAE runs an MCP (Model Context Protocol) server on a Unix socket (`/tmp/mae-PID.sock`). \
The `MAE_MCP_SOCKET` env var is injected into every spawned terminal. This lets Claude Code \
(running inside the terminal) call back into the editor via the same tool API the built-in \
AI uses. The `mae-mcp-shim` binary bridges stdio to the socket.\n\n\
## File auto-reload\n\
When switching to a buffer whose backing file has changed on disk:\n\
- **Clean buffer** (no unsaved edits): reloaded automatically.\n\
- **Dirty buffer**: warning shown, no clobber.\n\
The `file-changed-on-disk` hook fires in both cases.\n\n\
## Process lifecycle\n\
When the shell process exits (e.g. `exit` or `Ctrl-D`), MAE automatically:\n\
1. Switches back to Normal mode.\n\
2. Shuts down the PTY.\n\
3. Marks the buffer name with `[exited]`.\n\
Close the buffer manually with `SPC o c` or `:kill-buffer`.\n\n\
## Architecture\n\
The `mae-shell` crate wraps `alacritty_terminal::Term` with PTY management. The renderer \
reads the terminal grid and converts cells to ratatui spans with full color and attribute \
support. A 30fps render tick ensures smooth output.\n\n\
See also: [[concept:mode]], [[concept:ai-as-peer]], [[index]]\n";

pub(super) const CONCEPT_HOOKS: &str =
    "**Hooks** are MAE's primary extensibility mechanism — they let Scheme code react to \
editor events without the core knowing anything about Scheme.\n\n\
## Available hooks\n\
| Hook name | Fires when |\n\
|-----------|------------|\n\
| `before-save` | Just before a buffer is written to disk |\n\
| `after-save` | After a successful save |\n\
| `buffer-open` | After a file is opened into a buffer |\n\
| `buffer-close` | Before a buffer is killed |\n\
| `mode-change` | When the editing mode changes |\n\
| `command-pre` | Before a command is dispatched (planned) |\n\
| `command-post` | After a command completes (planned) |\n\
| `file-changed-on-disk` | When a buffer's backing file changes externally |\n\n\
## Usage from Scheme\n\
```scheme\n\
;; Register a function to run on save:\n\
(add-hook! \"after-save\" \"my-after-save\")\n\
\n\
;; Define the function:\n\
(define (my-after-save)\n\
  (display \"File saved!\"))\n\
\n\
;; Remove a hook:\n\
(remove-hook! \"after-save\" \"my-after-save\")\n\
```\n\n\
## Design\n\
Core fires hooks by pushing `(hook-name, fn-name)` entries into \
`Editor::pending_hook_evals`. The binary drains them and calls the Scheme runtime — \
the same intent pattern used for LSP and DAP. This keeps the core crate free of \
Scheme dependencies.\n\n\
See also: [[concept:command]], [[concept:options]], [[index]]\n";

pub(super) const CONCEPT_OPTIONS: &str =
    "MAE's editor options can be configured from Scheme using `(set-option! KEY VALUE)`.\n\n\
## Available options\n\
| Option | Values | Description |\n\
|--------|--------|-------------|\n\
| `line-numbers` | `true`/`false` | Show line numbers in gutter |\n\
| `relative-line-numbers` | `true`/`false` | Relative line numbering |\n\
| `word-wrap` | `true`/`false` | Soft-wrap long lines |\n\
| `break-indent` | `true`/`false` | Indent wrapped continuation lines |\n\
| `show-break` | string | Character prefix for wrapped lines (e.g. `↪`) |\n\
| `theme` | theme name | Set the color theme |\n\
| `show-fps` | `true`/`false` | Show FPS overlay in status bar |\n\
| `font-size` | float (6-72) | GUI font size in points |\n\n\
## Usage from Scheme\n\
```scheme\n\
;; In init.scm:\n\
(set-option! \"line-numbers\" \"true\")\n\
(set-option! \"relative-line-numbers\" \"true\")\n\
(set-option! \"theme\" \"dracula\")\n\
(set-option! \"word-wrap\" \"true\")\n\
(set-option! \"show-break\" \"↪ \")\n\
```\n\n\
## Toggle commands\n\
Options can also be toggled interactively via `SPC t`:\n\
- `SPC t l` — [[cmd:toggle-line-numbers]]\n\
- `SPC t r` — [[cmd:toggle-relative-line-numbers]]\n\
- `SPC t w` — [[cmd:toggle-word-wrap]]\n\
- `SPC t t` — [[cmd:cycle-theme]]\n\n\
See also: [[concept:hooks]], [[concept:command]], [[index]]\n";

pub(super) const CONCEPT_AGENT_BOOTSTRAP: &str =
    "MAE auto-configures AI agents running inside its embedded terminal so they \
can discover the editor's MCP tools with zero manual setup.\n\n\
## How it works\n\
1. MAE starts an MCP socket server at `/tmp/mae-{pid}.sock`.\n\
2. The `MAE_MCP_SOCKET` env var is injected into every PTY.\n\
3. On first `:terminal` spawn, MAE writes `.mcp.json` to the project root:\n\
   ```json\n\
   { \"mcpServers\": { \"mae-editor\": { \"command\": \"/path/to/mae-mcp-shim\" } } }\n\
   ```\n\
4. MAE also writes agent-specific settings to auto-approve tools \
(e.g. `.claude/settings.local.json` for Claude Code).\n\
5. The agent reads `.mcp.json`, spawns the shim, and gets full tool access.\n\
6. The shim inherits `MAE_MCP_SOCKET` from the shell env and connects.\n\n\
## Commands\n\
- `:agent-setup <name>` — write `.mcp.json` and approval settings for an agent\n\
- `:agent-list` — show all agents MAE can bootstrap\n\
- `mae --setup-agents [DIR]` — CLI: write configs without starting the editor\n\n\
## Configuration\n\
In `~/.config/mae/config.toml`:\n\
```toml\n\
[agents]\n\
auto_mcp_json = true       # write .mcp.json on terminal spawn\n\
auto_approve_tools = true  # write agent settings for tool approval\n\
```\n\
Env var overrides: `MAE_AGENTS_AUTO_MCP=0`, `MAE_AGENTS_AUTO_APPROVE=0`\n\n\
## Adding a new agent\n\
The bootstrap system is agent-agnostic. See the doc comments in `agents.rs` \
for how to add support for new AI agents. Claude Code is the reference \
implementation.\n\n\
## AI permission tiers (internal)\n\
MAE's own tool permissions are separate from agent approval. Use the \
`ai_permissions` tool or `MAE_AI_PERMISSIONS` env var to control what \
tier the AI auto-approves up to.\n\n\
See also: [[concept:terminal]], [[concept:ai-as-peer]], [[index]]\n";

pub(super) const CONCEPT_SELF_TEST: &str =
    "The **self-test** command (`:self-test`) tells the AI agent to exercise its own tool \
surface and report what works, what's broken, and what's unavailable.\n\n\
## Usage\n\
- `:self-test` — run all test categories.\n\
- `:self-test introspection` — run only the introspection category.\n\
- `:self-test editing,help` — run multiple specific categories.\n\n\
## Categories\n\
| Category | What it tests |\n\
|----------|---------------|\n\
| **introspection** | `cursor_info`, `editor_state`, `list_buffers`, `window_layout`, `command_list`, `ai_permissions` |\n\
| **editing** | `create_file`, `buffer_write`, `buffer_read`, `open_file`, `switch_buffer`, `close_buffer` |\n\
| **help** | `kb_search`, `kb_get`, `kb_list`, `kb_graph`, `kb_links_from`, `kb_links_to`, `help_open` |\n\
| **project** | `project_info`, `project_files`, `project_search` (needs git repo) |\n\
| **lsp** | `lsp_diagnostics`, `lsp_document_symbols` (needs LSP server) |\n\
| **dap** | `dap_start`, `dap_set_breakpoint`, `dap_step` (needs lldb-dap or debugpy) |\n\
| **git** | `git_status`, `git_diff`, `git_log`, `git_stash_list` (needs git repo) |\n\
| **performance** | `introspect` timing metrics, lock contention, anomaly detection |\n\n\
## State management\n\
The self-test uses `editor_save_state` before tests and `editor_restore_state` after \
to leave the editor in a clean state regardless of pass/fail outcomes.\n\n\
## Reading results\n\
Results appear in the `*AI*` conversation buffer:\n\
- **[PASS]** — tool returned expected data.\n\
- **[FAIL]** — tool returned unexpected data or errored.\n\
- **[SKIP]** — prerequisite not met (e.g. no LSP server).\n\n\
The self-test also validates the command palette (key commands must exist) and \
runs a connected help-navigation walkthrough (search → get → graph → open).\n\n\
## Why this exists\n\
Unit tests validate individual components. The self-test validates the full \
AI↔editor integration: tool dispatch, permission checks, KB graph integrity, \
and command registration. It catches wiring bugs that unit tests can't reach.\n\n\
See also: [[concept:ai-as-peer]], [[concept:command]], [[concept:knowledge-base]], [[index]]\n";

pub(super) const CONCEPT_DEBUGGING: &str =
    "MAE integrates with the **Debug Adapter Protocol (DAP)** to provide a full \
debugging experience accessible to both the human user and the AI agent.\n\n\
## DAP client\n\
The DAP client connects to debug adapters via stdin/stdout. Built-in adapter \
presets: `lldb` (LLVM), `debugpy` (Python), `codelldb` (CodeLLDB / Rust+C++).\n\n\
## Debug panel\n\
The `*Debug*` buffer (`SPC d p` or `:debug-panel`) shows threads, stack frames, \
scopes, and variables in a navigable tree view.\n\n\
| Key | Action |\n\
|-----|--------|\n\
| `j`/`k` | Navigate up/down |\n\
| `Enter` | Expand/collapse node |\n\
| `o` | Open source at selected frame |\n\
| `q` | Close debug panel |\n\n\
## AI debug tools (13 tools)\n\
| Tool | Permission | Description |\n\
|------|-----------|-------------|\n\
| `dap_start` | Privileged | Launch adapter + debuggee |\n\
| `dap_set_breakpoint` | Write | Set a breakpoint at file:line |\n\
| `dap_remove_breakpoint` | Write | Remove a breakpoint |\n\
| `dap_continue` | Write | Resume execution |\n\
| `dap_step` | Write | Step over/into/out |\n\
| `dap_list_variables` | ReadOnly | List variables in current scope |\n\
| `dap_inspect_variable` | ReadOnly | Inspect a variable's value |\n\
| `dap_expand_variable` | ReadOnly | Expand a structured variable |\n\
| `dap_select_frame` | Write | Select a stack frame |\n\
| `dap_select_thread` | Write | Select a thread |\n\
| `dap_output` | ReadOnly | Debug adapter output |\n\
| `dap_evaluate` | Write | Evaluate expression in debuggee |\n\
| `dap_disconnect` | Write | Disconnect from debug session |\n\n\
Use `debug_state` to inspect the current session state (threads, frames, breakpoints).\n\n\
## Permission tiers\n\
- **Privileged** — `dap_start` (spawns processes).\n\
- **Write** — execution control (`dap_continue`, `dap_step`, `dap_set_breakpoint`, `dap_remove_breakpoint`, `dap_select_frame`, `dap_select_thread`, `dap_evaluate`, `dap_disconnect`).\n\
- **ReadOnly** — inspection (`dap_list_variables`, `dap_inspect_variable`, `dap_expand_variable`, `dap_output`).\n\n\
See also: [[concept:ai-as-peer]], [[cmd:debug-panel]], [[cmd:debug-start]], [[key:leader-keys]], [[index]]\n";

pub(super) const CONCEPT_GUI: &str =
    "MAE has a **dual rendering backend** — terminal (ratatui/crossterm) and GUI \
(winit + Skia 2D). Both backends share the same editor core, commands, and AI integration.\n\n\
## Launching\n\
- `mae --gui file.rs` — hardware-accelerated GUI window.\n\
- `mae file.rs` — terminal mode (default).\n\
- Desktop launcher: installed via `make install` to `~/.local/share/applications/mae.desktop`.\n\n\
## GUI features\n\
- **Mouse support:** click to place cursor, wheel scroll.\n\
- **Font configuration:** `config.toml` `[editor] font_size = 14.0` or `:set font_size 16`.\n\
- **Dirty-flag rendering:** GPU idle when nothing changes (~0% CPU).\n\
- **Shell colors:** terminal emulator respects editor theme.\n\
- **Shell scrollback:** Shift-PageUp/PageDown.\n\
- **FPS overlay:** `SPC t F` or `:set show_fps true`.\n\n\
## Architecture\n\
The `Renderer` trait (in `mae-renderer`) defines the backend-agnostic HAL. The `mae-gui` \
crate implements it using winit for windowing and skia-safe for 2D rendering. The terminal \
backend uses ratatui/crossterm. The binary selects the backend at startup based on `--gui`.\n\n\
## Event loop\n\
- **Terminal:** `crossterm::EventStream` + tokio `select!`.\n\
- **GUI:** `winit::pump_app_events()` + tokio `select!` with dirty-flag gating.\n\n\
See also: [[concept:terminal]], [[concept:mode]], [[index]]\n";

pub(super) const CONCEPT_PACKAGE_SYSTEM: &str = "\
The **package system** enables Scheme-based extensions via `require`/`provide`.\n\n\
## Loading\n\
- `(require \"feature\")` — searches `load-path` for `feature.scm` and evaluates it.\n\
- `(provide \"feature\")` — marks the current file as providing a feature.\n\
- `(featurep \"feature\")` — returns `#t` if the feature is loaded.\n\n\
## Load path\n\
Default: `~/.config/mae/packages/`, `~/.config/mae/lisp/`.\n\
- `(load-path)` — returns current search path as a list.\n\
- `(add-to-load-path! \"/path/to/dir\")` — prepends to search path.\n\n\
## Autoload\n\
`CommandSource::Autoload { feature }` enables deferred loading: when a command is first \
dispatched, `(require feature)` is triggered, then the command re-dispatches.\n\n\
See also: [[concept:hooks]], [[concept:options]], [[index]]\n";

pub(super) const CONCEPT_OPTION_REGISTRY: &str = "\
The **option registry** (`options.rs`) is the single source of truth for all editor settings.\n\n\
Each `OptionDef` has: name, aliases, kind, default, config_key, doc, valid_values.\n\
Kinds: `Bool`, `String`, `Float`, `Int`, `Theme`.\n\n\
## Flow\n\
1. `:set foo bar` → `Editor::set_option(\"foo\", \"bar\")`\n\
2. Validates kind + range → sets field on `Editor`\n\
3. `get_option(name)` reads back the current value\n\n\
## Scheme\n\
- `(set-option! \"name\" \"value\")` — from Scheme\n\
- `(get-option \"name\")` — returns current value as string\n\
- `*option-list*` — all options as `(name kind default doc)` tuples\n\n\
## Range clamping\n\
Options with numeric types are clamped to valid ranges in `set_option()` to prevent \
rendering corruption (e.g. heading_scale ≤0 → infinite loop).\n\n\
See also: [[concept:command]], [[concept:hooks]], [[index]]\n";

pub(super) const CONCEPT_SCHEME_API: &str = "\
MAE exposes ~50 Scheme functions to extension code. They fall into categories:\n\n\
## Buffer editing\n\
`buffer-insert`, `buffer-delete-range`, `buffer-replace-range`, `buffer-undo`, `buffer-redo`\n\n\
## Buffer read\n\
`*buffer-name*`, `*buffer-text*`, `*buffer-char-count*`, `buffer-text-range`, \
`*buffer-list*`, `get-buffer-by-name`\n\n\
## Cursor / navigation\n\
`cursor-goto`, `*cursor-row*`, `*cursor-col*`, `open-file`, `switch-to-buffer`\n\n\
## Windows\n\
`*window-count*`, `*window-list*`\n\n\
## Options / commands\n\
`set-option!`, `set-local-option!`, `get-option`, `*option-list*`, \
`define-command`, `run-command`, `command-exists?`, `*command-list*`\n\n\
## Keymaps\n\
`define-key`, `define-keymap`, `undefine-key!`, `*keymap-list*`, `keymap-bindings`\n\n\
## File I/O\n\
`read-file`, `file-exists?`, `list-directory`\n\n\
## Architecture\n\
Write-side: `SharedState` (Arc<Mutex>) accumulates `pending_*` fields during eval.\n\
Read-side: `inject_editor_state()` snapshots editor state as globals before eval.\n\
Apply: `apply_to_editor()` drains pending changes after eval.\n\n\
See also: [[concept:hooks]], [[concept:options]], [[index]]\n";

pub(super) const CONCEPT_AI_MODES: &str = "\
MAE provides two distinct AI interfaces, each suited to different workflows.\n\n\
## AI Agent (`SPC a a`)\n\
An **external tool** (Claude Code, gemini-cli, etc.) running in MAE's embedded terminal.\n\n\
**When to use:**\n\
- Autonomous coding: writing features, fixing bugs, multi-file refactors\n\
- Tasks that need shell access: running tests, installing packages, git operations\n\
- When you want the AI to drive and you review the results\n\n\
**Configuration:**\n\
```toml\n\
[ai]\n\
editor = \"claude\"  # command to run in terminal\n\
```\n\n\
The agent communicates with MAE via the MCP bridge — it can call editor tools \
just like the built-in AI.\n\n\
## AI Chat (`SPC a p`)\n\
MAE's **native conversation** interface with full editor context.\n\n\
**When to use:**\n\
- Quick questions about code in your current buffer\n\
- LSP-aware code review (the AI sees diagnostics, types, references)\n\
- Editor-integrated tasks: explain this function, suggest a refactor, write a docstring\n\
- When you want to stay in the editor flow without context-switching\n\n\
**Configuration:**\n\
```toml\n\
[ai]\n\
provider = \"claude\"  # or openai, gemini, deepseek\n\
model = \"claude-sonnet-4-20250514\"\n\
```\n\n\
## Shared configuration\n\
Both interfaces respect:\n\
- **Permission tiers:** `readonly`, `standard`, `trusted`, `privileged`\n\
- **Budget limits:** `budget_warn_tokens`, `budget_limit_tokens`\n\
- **API keys:** env vars (ANTHROPIC_API_KEY, etc.) or `api_key_command`\n\n\
See also: [[tutorial:ai-setup]], [[concept:ai-as-peer]], [[index]]\n";

pub(super) const CONCEPT_CONCEAL: &str = "\
**Link & Markup Rendering** controls how inline markup is displayed — \
showing styled labels instead of raw syntax.\n\n\
## Options\n\
| Option | Default | Description |\n\
|--------|---------|-------------|\n\
| `link_descriptive` | `true` | Strip `[label](url)` markup, show styled label only |\n\
| `render_markup` | `true` | Render `**bold**`, `` `code` ``, `*bold*`, `/italic/`, `=code=`, `~verbatim~` with styling |\n\n\
## Configuration\n\
- `:set link_descriptive false` — show raw `[label](url)` text\n\
- `:set render_markup false` — disable inline styling in conversation buffers\n\
- `:setlocal nolink_descriptive` — per-buffer override\n\
- `config.toml`: `link_descriptive = true` under `[editor]`\n\
- Scheme: `(set-option! \"link-descriptive\" \"true\")`\n\n\
## Scope\n\
- **Conversation buffers:** markdown links are stripped to labels; org and markdown \
inline markup (bold, italic, code) get styling spans\n\
- **Help buffers:** both markdown and org inline markup are styled\n\
- Links are clickable via `gx` (`open-link-at-cursor`)\n\n\
## Safety\n\
Inline markup spans intentionally exclude `markup.heading` — heading spans \
would trigger `line_heading_scale()` in `compute_layout()`, breaking uniform \
line heights in conversation buffers.\n\n\
See also: [[concept:options]], [[concept:buffer]], [[concept:ai-as-peer]]\n";
