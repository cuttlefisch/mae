pub(super) const CONCEPT_GIT_STATUS: &str = "\
The **Git Status** buffer (`*git-status*`) is a high-fidelity \"porcelain\" UI \
inspired by Emacs Magit. It allows you to manage your repository state \
without leaving the editor.\n\n\
** Multi-Level Fold\n\
Press `TAB` on a section header, file entry, or hunk header to fold/unfold \
that level independently. Collapse indicators (`▸`/`▾`) show fold state.\n\n\
** Keybindings\n\
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
** Context-Aware Dispatch\n\
`s`/`u`/`x` operate based on cursor position:\n\
- **On a diff hunk/line**: stage/unstage/discard that hunk.\n\
- **On a file entry**: stage/unstage/discard the whole file.\n\
- **On a section header**: stage/unstage all files in that section.\n\n\
** Inline Diff\n\
Press `TAB` on a file entry to expand/collapse its inline diff. Each hunk \
can be further folded independently.\n\n\
** Workflow\n\
1. Open status via `SPC g s`.\n\
2. Navigate with `j`/`k`, jump hunks with `n`/`p`.\n\
3. Stage files/hunks with `s`.\n\
4. Commit with `c c` (opens a commit message buffer).\n\n\
See also: [[concept:project]], [[concept:terminal]], [[concept:buffer-mode]]\n";

pub(super) const CONCEPT_ORG_MODE: &str = "\
**Org-mode** in MAE provides structural editing and task management \
capabilities for `.org` files, inspired by Doom Emacs evil-org.\n\n\
** Core Features\n\n\
*** 1. Three-State Heading Cycle (TAB)\n\
Press `TAB` on a heading to cycle its visibility through three states:\n\
**Subtree** (everything visible) -> **Folded** (heading only) -> \
**Children** (body + child headings visible, child bodies folded) -> **Subtree**.\n\
Leaf headings (no children) toggle between **Subtree** and **Folded**.\n\n\
*** 2. Fold All / Unfold All\n\
- `zM` (`close-all-folds`): Fold all headings in the buffer.\n\
- `zR` (`open-all-folds`): Unfold all headings.\n\
- `za`: Toggle fold at cursor (tree-sitter or heading).\n\n\
*** 3. Structural Editing\n\
- `M-h` / `M-Left`: **Promote** heading (remove one `*` prefix).\n\
- `M-l` / `M-Right`: **Demote** heading (add one `*` prefix).\n\
- `M-k` / `M-Up`: **Move subtree up** past previous sibling.\n\
- `M-j` / `M-Down`: **Move subtree down** past next sibling.\n\
Moving a subtree automatically clears any folds in the affected range.\n\n\
*** 4. Narrow / Widen\n\
- `SPC m s n` (`org-narrow-subtree`): Narrow buffer to current heading's subtree. \
Only lines in that subtree are visible; cursor is clamped to the range. \
Status bar shows `[Narrowed]`.\n\
- `SPC m s w` (`org-widen`): Restore full buffer visibility.\n\n\
*** 5. Heading Font Scaling\n\
Org headings render at scaled font sizes for visual hierarchy:\n\
`*` = 1.5x, `**` = 1.3x, `***` = 1.15x.\n\
Disable with `:set heading_scale false`.\n\n\
*** 6. Insert Heading (M-Enter)\n\
- On a heading line: Insert a new heading at the **same level** after the current subtree.\n\
- Not on a heading: Insert a level-1 heading below the current line.\n\
- Automatically enters insert mode with cursor after the heading prefix.\n\n\
*** 7. Task Management\n\
- `S-Left` / `S-Right`: Cycle TODO states (`TODO` -> `DONE` -> `None`).\n\
- `S-Up` / `S-Down`: Cycle priorities (`[#A]` -> `[#B]` -> `[#C]`).\n\n\
*** 8. Links\n\
Press `Enter` on a link to follow it. Internal links jump to headings; \
external links open in your browser.\n\n\
*** 9. Rich Rendering\n\
- `*bold*` text is rendered in bold.\n\
- `/italic/` text is rendered in italics.\n\
- **Emphasis Markers**: Use `:set org_hide_emphasis_markers true` to hide \
the surrounding `*` and `/` characters.\n\n\
See also: [[concept:markdown]], [[concept:knowledge-base]], [[concept:options]]\n";

pub(super) const CONCEPT_MARKDOWN: &str = "\
**Markdown** in MAE provides structural editing for `.md` files, \
with the same UX as [[concept:org-mode|org-mode]] adapted for `#` headings.\n\n\
** Core Features\n\n\
*** 1. Three-State Heading Cycle (TAB)\n\
Press `TAB` on a heading to cycle its visibility:\n\
**Subtree** (everything visible) -> **Folded** (heading only) -> \
**Children** (body + child headings visible, child bodies folded) -> **Subtree**.\n\
Leaf headings (no children) toggle between **Subtree** and **Folded**.\n\n\
*** 2. Fold All / Unfold All\n\
- `zM` (`close-all-folds`): Fold all headings in the buffer.\n\
- `zR` (`open-all-folds`): Unfold all headings.\n\n\
*** 3. Structural Editing\n\
- `M-h` / `M-Left`: **Promote** heading (remove one `#` prefix).\n\
- `M-l` / `M-Right`: **Demote** heading (add one `#` prefix).\n\
- `M-k` / `M-Up`: **Move subtree up** past previous sibling.\n\
- `M-j` / `M-Down`: **Move subtree down** past next sibling.\n\n\
*** 4. Narrow / Widen\n\
- `SPC m s n` (`md-narrow-subtree`): Narrow buffer to current heading's subtree.\n\
- `SPC m s w` (`md-widen`): Restore full buffer visibility.\n\n\
*** 5. Insert Heading (M-Enter)\n\
- On a heading line: Insert a new heading at the **same level** after the current subtree.\n\
- Not on a heading: Insert a level-1 heading below the current line.\n\
- Automatically enters insert mode with cursor after the heading prefix.\n\n\
*** 6. Heading Font Scaling\n\
Markdown headings render at scaled font sizes:\n\
`#` = 1.5x, `##` = 1.3x, `###` = 1.15x.\n\
Disable with `:set heading_scale false`.\n\n\
*** 7. Markdown Keymap\n\
The `markdown` keymap activates automatically for `.md` files and falls back \
to the `normal` keymap for unbound keys. All structural editing keys mirror \
the org-mode keymap.\n\n\
See also: [[concept:org-mode]], [[concept:options]]\n";

pub(super) const CONCEPT_EX_COMMANDS: &str = "\
**Ex-command grammar** for write/quit compound commands.\n\n\
MAE parses `:w`, `:q`, `:x` commands using a token grammar rather than \
hardcoded match arms. This means all valid vim compound forms work \
automatically.\n\n\
** Grammar\n\n\
**Verbs:** `w` (write), `q` (quit), `x` (write-if-modified + quit)\n\
**Modifiers:** `a` (all — applies to preceding verb), `!` (force, must be terminal)\n\n\
** Valid Combinations\n\n\
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
** Implementation\n\
The tokenizer lives in `crates/core/src/editor/ex_parse.rs`. \
`parse_write_quit()` returns `Option<Vec<ExWriteQuit>>` — None for non-matching \
commands, Some for valid compound commands.\n\n\
See also: [[concept:command]], [[concept:options]]\n";

pub(super) const CONCEPT_SET_SYNTAX: &str = "\
**`:set` option syntax** — vim-style option configuration.\n\n\
** Syntax Forms\n\n\
| Syntax | Effect |\n\
|--------|--------|\n\
| `:set option` | Enable (bool) or query (non-bool) |\n\
| `:set nooption` | Disable bool option |\n\
| `:set option!` | Toggle bool option |\n\
| `:set option?` | Query current value |\n\
| `:set option value` | Assign value |\n\
| `:set option \"value with spaces\"` | Quoted value |\n\n\
** Tab Completion\n\n\
- `:set <Tab>` completes option names\n\
- `:set option <Tab>` completes values:\n\
  - Bool options: `true`, `false`\n\
  - Enum options: cycles through valid values\n\
  - Theme options: lists bundled themes\n\n\
** Implementation\n\
The parser lives in `crates/core/src/editor/ex_parse.rs` (`parse_set_args()`). \
Value completion is in `crates/core/src/editor/file_ops.rs` (`complete_set_value()`).\n\n\
See also: [[concept:options]], [[concept:command]]\n";

pub(super) const CONCEPT_SCROLLBAR: &str = "\
**Vertical scrollbar** for the GUI rendering backend.\n\n\
** Configuration\n\
- `:set scrollbar true` (default: enabled)\n\
- `:set scrollbar false` to disable\n\n\
** Layout\n\
The scrollbar occupies 1 column at the right edge of the text area. \
Space is allocated in `FrameLayout::compute_layout()` *before* wrap/layout \
computation, so text wrapping respects the reduced width.\n\n\
** Rendering\n\
- **Track**: full content-area height, theme color `ui.scrollbar.track`\n\
- **Thumb**: proportional to viewport/total ratio, theme color `ui.scrollbar.thumb`\n\
- Minimum thumb height: 1 cell\n\n\
** Mouse Interaction\n\
Click in the scrollbar column to jump to that scroll position.\n\n\
** Nyan Mode\n\
`:set nyan_mode true` enables a rainbow progress bar in the status line, \
showing scroll position as a filled bar with a cat marker.\n\n\
See also: [[concept:gui]], [[concept:options]]\n";

pub(super) const CONCEPT_AUTOSAVE: &str = "\
**Autosave** periodically saves all modified file-backed buffers in the background.\n\n\
** Configuration\n\
Configure in `init.scm` — MAE's primary config surface:\n\
- `:set autosave_interval 300` — save every 5 minutes (0 = disabled)\n\
- Scheme: `(set-option! \"autosave-interval\" \"300\")` (persists via `:set-save`)\n\n\
** Idle Debounce\n\
Autosave waits at least **5 seconds** after the last edit before saving. \
This prevents saving mid-typing. The timer resets with each keystroke.\n\n\
** Behavior\n\
- Only file-backed buffers (not scratch, conversation, or shell) are saved.\n\
- Status bar shows \"Autosaved N buffer(s)\" on each save.\n\
- Errors are reported but don't interrupt editing.\n\n\
See also: [[concept:options]], [[cmd:save]]\n";

pub(super) const CONCEPT_FILE_TREE: &str = "\
**File Tree** is a sidebar showing the project directory structure with file-type icons.\n\n\
** Keybindings\n\
| Key | Action |\n\
|---|---|\n\
| `SPC f t` | Toggle file tree sidebar |\n\
| `j` / `k` | Navigate entries |\n\
| `Enter` | Open file / toggle directory |\n\
| `o` | Toggle expand/collapse directory |\n\
| `R` | Refresh tree from disk |\n\
| `q` | Close file tree |\n\n\
** Project Root\n\
The tree roots at the detected project root (`.git`, `Cargo.toml`, etc.). \
Falls back to the current working directory.\n\n\
** Icons\n\
File type icons are Unicode emoji by default (no font dependency):\n\
- Directories: open/closed folder\n\
- `.rs` → crab, `.py` → snake, `.js` → lightning, `.toml`/`.json` → gear\n\n\
** Filtering\n\
Build artifacts and VCS directories (`target/`, `node_modules/`, `.git/`) \
are hidden automatically.\n\n\
See also: [[cmd:find-file]], [[concept:buffer]], [[concept:project]]\n";

pub(super) const CONCEPT_DIFF_DISPLAY: &str = "\
**Diff Display** renders unified diffs with syntax-highlighted lines.\n\n\
** Flow\n\
1. AI calls `propose_changes` tool with edits\n\
2. MAE computes a unified diff (LCS-based) between old and new content\n\
3. The diff is displayed in the `*AI-Diff*` buffer\n\
4. Lines are colored by type:\n\
   - `+` lines → `diff.added` (green)\n\
   - `-` lines → `diff.removed` (red)\n\
   - `@@` headers → `diff.hunk` (magenta)\n\
   - `---`/`+++` headers → `diff.header` (cyan, bold)\n\n\
** Commands\n\
- `:ai-accept` — apply the proposed changes\n\
- `:ai-reject` — discard the proposed changes\n\n\
** Theme Keys\n\
All 8 bundled themes include `diff.added`, `diff.removed`, `diff.hunk`, \
and `diff.header` style definitions.\n\n\
See also: [[concept:ai-as-peer]], [[concept:options]]\n";

pub(super) const CONCEPT_BUFFER_MODE: &str = "\
The **BufferMode** trait (`buffer_mode.rs`) is the contract every buffer kind implements. \
It replaces scattered `match buf.kind` blocks with polymorphic dispatch.\n\n\
** Methods\n\
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
Accessor methods: `buf.conversation()`, `buf.kb_view()`, `buf.git_status_view()`, etc. \
Each returns `Option<&T>` (or `Option<&mut T>` for the `_mut` variant).\n\n\
This replaced 6 `Option<T>` fields that were always mutually exclusive.\n\n\
See also: [[concept:buffer]], [[concept:buffer-mode]]\n";

pub(super) const CONCEPT_KEYMAP_INHERITANCE: &str = "\
**Keymap inheritance** lets buffer-kind overlay keymaps (git-status, help, debug, file-tree) \
inherit bindings from a parent keymap.\n\n\
** Mechanism\n\
- `Keymap` has a `parent: Option<String>` field.\n\
- Key lookup: overlay keymap -> parent -> fallback.\n\
- `which_key_entries_for_current_keymap()` merges overlay + parent entries for the which-key popup.\n\n\
** Scheme API\n\
`(define-keymap \"name\" \"parent\")` creates a keymap with inheritance.\n\n\
** Current Overlay Keymaps\n\
| Keymap | Parent | Buffer Kind |\n\
|--------|--------|-------------|\n\
| `git-status` | `normal` | GitStatus |\n\
| `help` | `normal` | Help |\n\
| `debug` | `normal` | Debug |\n\
| `file-tree` | `normal` | FileTree |\n\n\
See also: [[concept:mode]], [[concept:buffer-mode]]\n";

pub(super) const CONCEPT_PROMPT_TIERS: &str = "\
** Prompt Tiers\n\n\
MAE uses tiered system prompts to optimize AI agent behavior for different models.\n\n\
*** Full Tier\n\
Concise prompt (~25 lines) for frontier models with strong implicit reasoning:\n\
- Claude Opus, Claude Sonnet, GPT-4o, GPT-4 Turbo, Gemini 2.5 Pro, o1\n\n\
*** Compact Tier\n\
Explicit guardrails (~70 lines) for smaller/cheaper models:\n\
- DeepSeek, Claude Haiku, GPT-4o-mini, Gemini Flash, o1-mini\n\
- Includes: tool preferences, fallback chains, anti-looping rules, common recipes\n\n\
*** Default Assignments\n\
Unknown models default to **compact** (safe: over-prompting wastes a few tokens; \
under-prompting wastes millions in looping).\n\n\
*** Override\n\
Force a tier regardless of model with `[ai] prompt_tier = \"full\"` (or \
`\"compact\"`) in `config.toml`. Prompt-tier selection is part of the narrow AI-provider \
bootstrap read at startup (alongside provider/model), so it lives in `config.toml` rather \
than the Scheme option surface.\n\n\
*** Custom Prompts\n\
Place `pair-programmer.xml` or `pair-programmer-compact.xml` in:\n\
- `~/.config/mae/prompts/` (user override)\n\
- `.mae/prompts/` (project-local override)\n\n\
See also: [[concept:ai-modes|AI Agent vs Chat]], [[concept:ai-as-peer|AI as Peer]]\n";

pub(super) const CONCEPT_DISPLAY_POLICY: &str = "\
** Display Policy\n\n\
Controls how buffers become visible in windows — the O(1) enum-dispatch replacement \
for Emacs's 29 `display-buffer-*` functions and regex alist.\n\n\
*** The Problem\n\
Five direct `focused_window_mut().buffer_idx` calls (help, messages, debug, git-status, \
file-tree) had zero conversation awareness. If the AI agent called `help_open` while \
focused on the tiny AI input pane, the KB buffer got crammed in and the conversation \
layout was destroyed.\n\n\
*** The 4 Actions (vs Emacs's 29)\n\
- **ReplaceFocused** — replace the focused window, but fall through to AvoidConversation \
if focused on a conversation buffer (git-status, dashboard)\n\
- **AvoidConversation** — route via `display_buffer_for_agent` which has a \
3-step strategy protecting conversation pairs (text, diff)\n\
- **ReuseOrSplit** — reuse an existing window of the same BufferKind, or create a split \
with the given direction and ratio (help → 50% vsplit, messages → 30% hsplit)\n\
- **Hidden** — buffer exists for programmatic access only, never shown (conversation — \
managed by `open_conversation_buffer`)\n\n\
*** Default Rules\n\
| Kind        | Action               | Rationale                   |\n\
| Text        | AvoidConversation    | Normal files never invade   |\n\
| Help        | ReuseOrSplit V 50%   | Reuse help window or vsplit |\n\
| Messages    | ReuseOrSplit H 30%   | Bottom 30%, reuse if open   |\n\
| Debug       | ReuseOrSplit H 40%   | Bottom 40%                  |\n\
| GitStatus   | ReplaceFocused       | Full window (Magit style)   |\n\
| Conversation| Hidden               | Managed internally          |\n\n\
*** Customization\n\
From init.scm: `(set-display-rule! \"help\" \"reuse-or-split:vertical:0.5\")`\n\
Inspect: `(display-buffer-policy \"help\")` or `SPC h D` ([[cmd:describe-display-policy]])\n\n\
*** Emacs Comparison\n\
Emacs: `display-buffer-alist` (29 action functions, regex matching, order-dependent). \
Doom: `set-popup-rules!` (simpler but still regex). MAE: enum dispatch by BufferKind — \
O(1), no order bugs, no regex.\n\n\
See also: [[concept:buffer]], [[concept:window]], [[concept:buffer-mode]]\n";

pub(super) const CONCEPT_MCP_DEVELOPMENT: &str = "\
** MCP Development Workflow\n\n\
All 700+ MAE tools are exposed via MCP with full parity. When developing MAE \
inside MAE (e.g. with Claude Code via `mae-mcp-shim`), these tools provide \
structured access to LSP, DAP, KB, and editor state.\n\n\
*** Tool Categories\n\
- **Code Navigation (LSP):** `lsp_definition`, `lsp_references`, `lsp_hover`, \
`lsp_workspace_symbol`, `lsp_document_symbols`, `lsp_diagnostics`\n\
- **Debugging (DAP):** `dap_start`, `dap_set_breakpoint`, `dap_continue`, \
`dap_step`, `debug_state`\n\
- **Knowledge Base:** `kb_search`, `kb_get`, `kb_links_from`, `kb_links_to`, `kb_graph`\n\
- **Buffer/Editor:** `buffer_read`, `buffer_write`, `project_search`, \
`command_list`, `execute_command`, `eval_scheme`, `audit_configuration`, `introspect`\n\
- **Validation:** `self_test_suite` — structured JSON test plan\n\n\
*** Connection\n\
Socket: `/tmp/mae-{PID}.sock`\n\
Shim: `mae-mcp-shim` (stdio ↔ Unix socket bridge)\n\n\
*** When to Use\n\
- **Navigating MAE code:** `lsp_definition` / `lsp_references` over grep — structured, no false positives\n\
- **Understanding architecture:** `kb_search` or `kb_get` — curated docs\n\
- **Debugging MAE:** `dap_start` with `lldb-dap`, `debug_state` for inspection\n\
- **Testing changes:** `execute_command`, `self_test_suite` for E2E\n\n\
See also: [[concept:ai-as-peer]], [[concept:agent-bootstrap]], [[concept:self-test]]\n";

pub(super) const INDEX_BODY: &str = "Welcome to MAE's built-in help. This knowledge base is the same data \
surface the AI agent queries via its `kb_*` tools — you and the AI read the same pages.

** Core concepts
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
- [[concept:modules|Module System]] — structured packages with manifests, flags, and CLI\n\
- [[concept:flags|Module Flags]] — optional sub-features with +flag syntax\n\
- [[concept:design-philosophy|Design Philosophy]] — composition over inheritance, stable API\n\
- [[guide:extension-authoring|Extension Authoring Guide]] — how to create MAE modules\n\
- [[concept:option-registry|Option Registry]] — single source of truth for editor settings\n\
- [[concept:scheme-api|Scheme API]] — ~50 functions for buffer/window/command/keymap access\n\
- [[concept:ai-modes|AI Agent vs Chat]] — when to use each AI interface\n\
- [[concept:prompt-tiers|Prompt Tiers]] — model-aware prompt selection (full vs compact)\n\
- [[concept:display-policy|Display Policy]] — how buffers are placed in windows (4 actions, O(1) dispatch)\n\
- [[concept:sync-engine|Sync Engine]] — yrs (Yjs Rust) CRDT for collaborative state\n\
- [[concept:collaborative-state|Collaborative State]] — vision: text + visual + KB sync\n\
- [[concept:adr-text-sync|ADR-002: Text Sync]] — decision: yrs/YATA (accepted)\n\
- [[concept:adr-kb-crdt|ADR-005: KB CRDT]] — KB nodes as yrs documents

** Reference
- [[key:normal-mode|Normal-mode keys]]
- [[key:leader-keys|SPC leader bindings]] (14 groups, Doom Emacs style)
- [[concept:project|Project management]]
- Commands: run `:command-list` for the full list, or visit any `cmd:<name>` node.
- Browse by category: `category:movement`, `category:editing`, `category:git`, etc.

** Tutorial
- [[tutorial:getting-started|Getting Started]] — progressive guide (Vim track / Beginner track / AI setup)
- [[tutor:index|Lesson-style Tutorial]] — 12 focused lessons covering all essentials

** Getting around
- **Enter** on a link follows it.
- **C-o** goes back, **C-i** goes forward (history, like vim jumps).
- **q** closes the KB viewer.
";

pub(super) const CONCEPT_BUFFER: &str = "A **buffer** is the unit of editable content in MAE.\n\
It has an optional file path, a kind (BufferKind), modification \
state, and either a rope (for text) or a structured payload (for conversations, help, etc).\n\n\
** Contrast with other editors\n\
- **Emacs buffer** ≈ MAE buffer (same lineage).\n\
- **Vim buffer** ≈ MAE buffer, but MAE does not have Vim's separate *tabs* or *windows-per-tab* concept.\n\
- **VSCode tab** is a UI affordance — MAE exposes no such primitive.\n\n\
** What buffers do NOT own\n\
Cursor position lives on [[concept:window|Window]], not on the buffer. Two windows can \
view the same buffer at different points — the design is deliberately Emacs-shaped here.\n\n\
** Display Policy\n\
How a buffer becomes visible is governed by the [[concept:display-policy|Display Policy]], \
which maps each BufferKind to a DisplayAction (replace, avoid conversation, reuse/split, hidden).\n\n\
See also: [[concept:window]], [[concept:command]], [[concept:display-policy]]\n";

pub(super) const CONCEPT_WINDOW: &str =
    "A **window** is a rectangular view onto a [[concept:buffer|buffer]]. \
MAE's tiling WindowManager owns the layout tree (splits, sizes) \
and exactly one window is focused at a time.\n\n\
** Why cursor state lives here, not on the buffer\n\
Emacs has taught us that two windows can legitimately view the same buffer at different \
points. If cursor state lived on the buffer, this would be impossible without extra hacks. \
MAE inherits this shape.\n\n\
** What MAE windows are NOT\n\
- NOT an OS-level window (Emacs's terminology for that is a \"frame\" — MAE has no frames).\n\
- NOT a tab (MAE has no tabs).\n\n\
See also: [[concept:buffer]], [[concept:mode]]\n";

pub(super) const CONCEPT_MODE: &str = "MAE is **modal** like Vim. The current [[concept:mode|Mode]] \
determines which keymap is active.\n\n\
** Modes\n\
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
See also: [[concept:ai-as-peer]], [[concept:command]]\n";

pub(super) const CONCEPT_AI_AS_PEER: &str = "MAE's single-most-important design stance: **the AI agent is a peer actor, not a plugin.**\n\n\
A keybinding and an AI tool-call both resolve to the same [[concept:command|Command]] \
via the same dispatcher. There is no separate \"AI mode\", no simulated keystrokes, no \
shadow API. When you type `dd` to delete a line, the agent can invoke `cmd:delete-line` \
with the same effect, and vice versa.\n\n\
** What the agent can see\n\
- Buffer contents (`buffer_read`) across all buffers (`list_buffers`).\n\
- Cursor state (`cursor_info`) and editor state (`editor_state`).\n\
- LSP diagnostics (`lsp_diagnostics`) and tree-sitter parse trees (`syntax_tree`).\n\
- DAP debug state (`debug_state`) when a session is active.\n\
- This knowledge base (`kb_get`, `kb_search`, `kb_list`).\n\
- [[concept:project|Project state]] via `project_info`, `project_files`, `project_search`.\n\
- **[[concept:introspect|Introspection]]**: The agent can see thread stacks, performance counters, and lock contention.\n\n\
** Interaction Surfaces\n\
1. **Internal Peer**: Embedded directly in MAE, sharing your active workspace context. Trigger via `SPC a p`.\n\
2. **External Agent**: Any MCP-capable client (like Gemini CLI or Claude Code) can connect to MAE via the `mae-mcp-shim`. The external agent gains full control of the editor's tool surface.\n\n\
** Permission tiers\n\
Every tool has a permission tier: ReadOnly, Write, Shell, \
Privileged. Users control how far the agent can act autonomously.\n\n\
See also: [[concept:knowledge-base]], [[concept:command]], [[concept:agent-bootstrap]]\n";

pub(super) const CONCEPT_KB: &str = "\
MAE's **knowledge base** is a typed graph database backed by CozoDB (Datalog). \
It serves as both the built-in manual and a personal knowledge graph \
(org-roam-equivalent). CozoDB is the sole graph store (sled storage engine in the \
editor; the daemon uses CozoDB-over-SQLite for persistence).\n\n\
** Graph model\n\
- **CozoDB** (Datalog) primary backend with typed nodes and relationships.\n\
- 14 `NodeKind` variants: Index, Command, Concept, Key, Note, Project, Category, \
Lesson, Tutorial, Meta, Block, SchemeApi, Task, View.\n\
- 20 typed relationship types with declared inverses (`implements`/`implemented_by`, \
`teaches`/`taught_by`, `requires`/`required_by`, etc.).\n\
- Full-text search via CozoDB FTS (Tantivy).\n\
- `NodeSource` provenance: Seed, UserOrg, Manual, Federation.\n\n\
** Node namespaces\n\
- `index` — the entry page.\n\
- `cmd:<name>` — one per registered [[concept:command|Command]] (auto-generated).\n\
- `concept:<slug>` — architectural concepts (hand-authored).\n\
- `key:<context>` — keybinding summaries.\n\
- `option:<name>` — editor options.\n\
- `category:<name>` — command category indices.\n\
- `scheme:<name>` — Scheme API functions.\n\
- `lesson:<slug>` — interactive tutorials.\n\
- `tutorial:<slug>` — getting-started tracks.\n\
- `term:<name>` — terminology definitions.\n\
- `view:<name>` — stored Datalog view queries.\n\n\
** Graph features\n\
- **Typed relationships**: 95+ seed relationships with semantic types.\n\
- **Node versioning**: Snapshot on update, version history, point-in-time restore.\n\
- **Meta-nodes**: Compose body from member nodes with cached refresh.\n\
- **Block addressing**: `parent_id#N` for paragraph-level references.\n\
- **Agenda queries**: Filter by TODO state, priority, tags, staleness, orphans.\n\
- **HNSW vector index**: 384-dim embeddings for semantic search (GraphRAG-ready).\n\
- **Views**: 6 pre-built flavors (kanban, backlog, sprint, timeline, agenda, custom).\n\n\
** Federation\n\
The KB supports multiple instances: a local KB (seed + user help) plus N external \
instances registered from org-roam directories. See [[concept:kb-federation]].\n\n\
** AI surface\n\
Core tools: `kb_get`, `kb_search`, `kb_list`, `kb_links_from`, `kb_links_to`, \
`kb_graph`, `kb_health`. Graph tools: `kb_agenda`, `kb_history`, `kb_restore`, \
`kb_view_query`, `kb_vector_search`, `kb_raw_query`, `kb_shortest_path`, \
`kb_neighborhood`, `kb_add_link`, `kb_search_context`. \
Federation: `kb_register`, `kb_unregister`, `kb_reimport`, `kb_instances`.\n\n\
See also: [[concept:kb-federation]], [[concept:kb-workflows]], [[concept:ai-as-peer]], [[index]]\n";

pub(super) const CONCEPT_KB_FEDERATION: &str = "\
KB **federation** lets you register external org directories as searchable \
knowledge base instances alongside MAE's built-in help.\n\n\
** Architecture\n\
```\n\
┌─────────────────────────────────────────┐\n\
│           MAE Knowledge Base            │\n\
├─────────────┬───────────────────────────┤\n\
│  Local KB   │  Federated Instances      │\n\
│  (seed +    │  ┌─────────┐ ┌─────────┐ │\n\
│   user help │  │RoamNotes│ │ Work KB │ │\n\
│   + AI)     │  │ 2,500   │ │ 87      │ │\n\
│  200+ nodes │  │ nodes   │ │ nodes   │ │\n\
│             │  └─────────┘ └─────────┘ │\n\
└─────────────┴───────────────────────────┘\n\
         ↕ search / get / graph\n\
    ┌─────────────────────┐\n\
    │  :help  │  AI tools │\n\
    │  SPC h  │  kb_*     │\n\
    └─────────────────────┘\n\
```\n\n\
** Design principle\n\
**The org directory is READ-ONLY for the KB layer. The CozoDB graph is derived.**\n\
Your org files remain the canonical source of truth. MAE reads them, \
builds an in-memory graph, and never writes to your org directory \
(except one sentinel file: `eor-instance.org`).\n\n\
** Registry\n\
Stored at `~/.config/mae/kb-registry.toml`. Each instance has:\n\
- UUID (generated or read from sentinel file)\n\
- Name (user-chosen display label)\n\
- Org directory path\n\
- Enabled flag\n\
- Last import timestamp\n\n\
** Sentinel file\n\
When you register a directory, MAE creates `eor-instance.org` with the \
instance UUID. This file is safe to delete — MAE recreates it on next \
registration. It marks the directory as a MAE KB instance.\n\n\
** Link scheme\n\
- `eor:node-id` — local-first lookup (checks local KB, then instances).\n\
- `eor:uuid/node-id` — targeted lookup in a specific instance.\n\n\
** Import pipeline\n\
1. Recursive `walkdir` over the org directory.\n\
2. Parse each `.org` file for `:ID:` properties (file-level + heading-level).\n\
3. Files without `:ID:` are counted but skipped.\n\
4. File-path links (images, attachments) are NOT treated as KB links.\n\
5. Duplicate `:ID:` values are detected and reported.\n\
6. Health metrics (orphans, broken links, namespaces) computed automatically.\n\n\
** Commands\n\
- `:kb-register <name> <path>` — register and import.\n\
- `:kb-unregister <name>` — remove instance.\n\
- `:kb-reimport <name>` — refresh after editing org files.\n\
- `:kb-instances` — list registered instances.\n\
- `:kb-health` — health report (orphans, broken links, namespace counts).\n\
- `:kb-promote` — promote a node from a federated instance into the primary,\n\
  CozoDB-backed KB (SPC n p), severing its dependency on the origin org file.\n\n\
See also: [[concept:knowledge-base]], [[concept:kb-workflows]], [[lesson:kb-import-roam]]\n";

pub(super) const CONCEPT_KB_WORKFLOWS: &str = "\
Workflows for exploring, authoring, migrating, and maintaining your knowledge base.\n\n\
** Exploration\n\
- `:help <topic>` or `SPC h h` — fuzzy search all KB nodes.\n\
- `SPC h s` — full-text search.\n\
- `SPC n f` / `:kb-find` — search with fuzzy matching.\n\
- Tab/Enter in Help buffer — follow links, navigate graph.\n\
- `C-o` — jump back in help history.\n\n\
** Authoring\n\
- Create `.org` files in `~/.config/mae/help/` with `:ID:` properties.\n\
- `:help-edit <topic>` — open/create a user help node for editing.\n\
- User-authored nodes are loaded on startup alongside seed nodes.\n\n\
** Migration from org-roam\n\
See [[lesson:kb-import-roam]] for step-by-step instructions.\n\
Quick version: `:kb-register MyNotes ~/org-roam`\n\n\
** Backup and restore\n\
**Your org files ARE the backup.** They're plain text on disk — version \
them with git, sync them with any tool you like.\n\
- The CozoDB cache is disposable: delete it and reimport, zero data loss.\n\
- `:kb-save <path>` exports a CozoDB snapshot (useful for sharing the index).\n\
- `:kb-load <path>` imports a snapshot.\n\
- `:kb-reimport <name>` rebuilds from org source.\n\
- **There is no new data format to manage.** Your existing org files + \
git workflow = complete data lifecycle.\n\n\
** Health monitoring\n\
- `:kb-health` — orphan nodes, broken links, namespace distribution.\n\
- Health metrics are also reported automatically after `:kb-register` and `:kb-reimport`.\n\n\
** AI access\n\
The AI agent uses the same tools: `kb_search`, `kb_get`, `kb_graph`. \
It sees everything you see. Ask it: \"find notes about X\" or \
\"import my KB at ~/RoamNotes\".\n\n\
See also: [[concept:knowledge-base]], [[concept:kb-federation]]\n";

pub(super) const CONCEPT_KB_ALTERNATIVES: &str = "\
How MAE's knowledge base compares to Obsidian and Roam Research.\n\n\
** Feature comparison\n\
| Feature | MAE KB | Obsidian | Roam Research |\n\
|---------|--------|----------|---------------|\n\
| Data format | Org-mode (plain text) | Markdown | Proprietary JSON |\n\
| Storage | Local files + CozoDB graph index | Local vault | Cloud (proprietary) |\n\
| Link model | Typed graph, reverse index | Wiki-links, backlinks | Block references |\n\
| Search | CozoDB FTS + fuzzy + substring | Basic full-text | Full-text + filter |\n\
| AI integration | Peer actor (same API) | Plugin (Copilot) | None native |\n\
| Federation | Multi-directory, cross-KB | Single vault | Single graph |\n\
| Open source | GPL-3.0 | Freemium, closed core | Proprietary |\n\
| Offline | Full offline | Full offline | Requires sync |\n\
| Extensibility | Scheme + module system | JS plugins | CSS themes only |\n\n\
** Migration paths\n\n\
*** From Obsidian\n\
- Obsidian vaults are Markdown files with wiki-style links.\n\
- Convert to org via pandoc: `pandoc -f markdown -t org input.md -o output.org`\n\
- Add `:ID:` properties (org-roam's `org-roam-migrate-wizard` helps).\n\
- Register: `:kb-register MyVault ~/converted-vault`\n\n\
*** From Roam Research\n\
- Export as Markdown or JSON from Roam settings.\n\
- Convert to org, assign `:ID:` properties.\n\
- `((block-ref))` maps lossy to heading-level `:ID:` nodes.\n\
- Register: `:kb-register RoamExport ~/roam-export`\n\n\
** Philosophy: why local-first + AI-peer + plain-text-canonical\n\n\
1. **Plain text is the only immortal format.** The CozoDB graph is derived. Cloud sync \
is a dependency. Org files survive every tool transition.\n\
2. **AI as peer, not plugin.** MAE's AI calls `kb_search`, `kb_get`, `kb_graph` — \
the same query surface the human uses. No impedance mismatch.\n\
3. **Federation > monolithic vault.** Life has multiple knowledge domains. \
Obsidian forces one vault or vault-switching. MAE federates: each domain \
is a registered instance, searchable together.\n\
4. **Ownership means exit.** Your org files are yours. No account, no sync \
service, no API key required to read your own notes.\n\
5. **Performance at the editor layer.** In-memory graph with pre-lowercased \
search cache. CozoDB Tantivy FTS. Sub-millisecond search across \
thousands of nodes. No Electron, no browser runtime.\n\n\
See also: [[concept:knowledge-base]], [[concept:kb-federation]], [[concept:kb-workflows]]\n";

pub(super) const CONCEPT_DAILIES: &str = "\
**Org-dailies** provides daily journal notes with backward chain-linking, \
inspired by `org-roam-dailies` in Emacs.\n\n\
** How It Works\n\
Each daily note lives at `<dailies-dir>/YYYY-MM-DD.org` with a unique ID \
(`daily:YYYY-MM-DD`). When you open today's daily, MAE creates the file if \
needed and **chain-fills** backward — creating stub files for any gaps and \
inserting Previous links (e.g. `Previous: YYYY-MM-DD`) to form a \
continuous backward chain.\n\n\
** Keybindings (SPC n d)\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `SPC n d t` | [[cmd:daily-goto-today]] | Open today's daily (chain-fill) |\n\
| `SPC n d y` | [[cmd:daily-goto-yesterday]] | Open yesterday's daily |\n\
| `SPC n d d` | [[cmd:daily-goto-date]] | Open daily for a specific date |\n\
| `SPC n d p` | [[cmd:daily-prev]] | Navigate to previous daily |\n\
| `SPC n d n` | [[cmd:daily-next]] | Navigate to next daily |\n\n\
** Configuration\n\
- `kb_dailies_dir` — explicit path (default: `<kb_notes_dir>/daily`)\n\
- `kb_daily_chain_gap_max` — max days to chain-fill backward (default: 90)\n\n\
** Chain-Fill Algorithm\n\
1. Ensure target date file exists (create stub if needed)\n\
2. Walk backward day-by-day from target\n\
3. For each missing day, create a stub `.org` file\n\
4. Insert `Previous:` link in each stub pointing to the prior day\n\
5. Stop when hitting a pre-existing daily or exhausting `kb_daily_chain_gap_max`\n\n\
All file writes use a write-guard to prevent the filesystem watcher from \
triggering duplicate reimports.\n\n\
See also: [[concept:knowledge-base]], [[concept:kb-workflows]], [[concept:modules]]\n";

pub(super) const CONCEPT_PROJECT: &str =
    "A **project** in MAE is a directory with optional `.project` TOML configuration.\n\n\
** Detection\n\
When you open a file, MAE walks upward from its directory looking for marker files:\n\
`.project` > `.git` > `Cargo.toml` > `package.json` > `go.mod` > `pyproject.toml` > `Makefile`.\n\
The first match becomes the project root.\n\n\
** .project TOML\n\
Optional declarative config:\n\
```toml\n\
name = \"My Project\"\n\
root-directory = \"~/src/my-project\"\n\
required-resources = [\"README.org\", \"Cargo.toml\"]\n\
```\n\n\
** SPC p commands\n\
- `SPC p f` — find file in project ([[cmd:project-find-file]])\n\
- `SPC p s` — search in project ([[cmd:project-search]])\n\
- `SPC p d` — browse project directory ([[cmd:project-browse]])\n\
- `SPC p r` — recent project files ([[cmd:project-recent-files]])\n\n\
** AI integration\n\
The AI agent can query project state via the `project_info` tool and \
search project files via `project_files` and `project_search`.\n\n\
See also: [[index]], [[concept:ai-as-peer]]\n";

pub(super) const CONCEPT_TERMINAL: &str =
    "MAE embeds a full **terminal emulator** backed by `alacritty_terminal`, the same \
engine that powers the Alacritty terminal. Programs like vim, less, top, fzf, and tmux \
work correctly — this is not a line-oriented shell like eshell.\n\n\
** Opening a terminal\n\
- `:terminal` or `SPC o t` — opens a new `*Terminal*` buffer in ShellInsert mode.\n\
- The terminal runs the user's `$SHELL` in a PTY.\n\n\
** Modes\n\
- **ShellInsert** — all keys go directly to the PTY. The terminal is fully interactive.\n\
- **Normal** — `Ctrl-\\ Ctrl-n` exits ShellInsert → Normal mode (Neovim convention). \
You can then use leader keys (`SPC`), window commands, etc.\n\
- Press `i` or `a` to re-enter ShellInsert from Normal mode on a terminal buffer.\n\n\
** Commands\n\
- [[cmd:terminal]] — open a new terminal buffer.\n\
- [[cmd:terminal-reset]] (`SPC o r`) — reset/clear the terminal (fixes residual \
characters from programs like cmatrix that don't clean up on exit).\n\
- [[cmd:terminal-close]] (`SPC o c`) — close the terminal and kill the shell process.\n\
- [[cmd:send-to-shell]] (`SPC e s`) — send current line to a terminal.\n\
- [[cmd:send-region-to-shell]] (`SPC e S`) — send visual selection to a terminal.\n\n\
** Scheme integration\n\
- `(shell-cwd BUF-IDX)` — returns the CWD of a shell buffer (via `/proc/PID/cwd`).\n\
- `(shell-read-output BUF-IDX MAX-LINES)` — reads the last N lines of terminal output.\n\
- `*shell-buffers*` — list of buffer indices that are Shell-kind.\n\n\
** MCP bridge\n\
MAE runs an MCP (Model Context Protocol) server on a Unix socket (`/tmp/mae-PID.sock`). \
The `MAE_MCP_SOCKET` env var is injected into every spawned terminal. This lets Claude Code \
(running inside the terminal) call back into the editor via the same tool API the built-in \
AI uses. The `mae-mcp-shim` binary bridges stdio to the socket.\n\n\
** File auto-reload\n\
When switching to a buffer whose backing file has changed on disk:\n\
- **Clean buffer** (no unsaved edits): reloaded automatically.\n\
- **Dirty buffer**: warning shown, no clobber.\n\
The `file-changed-on-disk` hook fires in both cases.\n\n\
** Process lifecycle\n\
When the shell process exits (e.g. `exit` or `Ctrl-D`), MAE automatically:\n\
1. Switches back to Normal mode.\n\
2. Shuts down the PTY.\n\
3. Marks the buffer name with `[exited]`.\n\
Close the buffer manually with `SPC o c` or `:kill-buffer`.\n\n\
** Architecture\n\
The `mae-shell` crate wraps `alacritty_terminal::Term` with PTY management. The renderer \
reads the terminal grid and converts cells to ratatui spans with full color and attribute \
support. A 30fps render tick ensures smooth output.\n\n\
See also: [[concept:mode]], [[concept:ai-as-peer]], [[index]]\n";

pub(super) const CONCEPT_BABEL: &str =
    "MAE executes **org-mode babel** source blocks — code inside \
`#+begin_src <lang> ... #+end_src` — and inserts the captured output as a \
`#+RESULTS:` block. Run the block at the cursor with `:babel-execute` \
(`SPC c e`); tangle a file's blocks to disk with `:babel-tangle`.\n\n\
** Supported languages\n\
- **Interpreted** (piped to the interpreter): python, ruby, perl, bash/sh, zsh, \
fish, javascript (node), lua, R.\n\
- **Compiled** (compile → cache → run): rust, go, c, c++/cpp. The compiled \
binary is cached by a hash of the source under `$XDG_CACHE_HOME/mae/babel`, so \
an unchanged block re-runs without recompiling. A runaway binary is killed at \
the babel timeout.\n\
- **In-editor:** scheme/elisp (evaluated in the Scheme runtime) and \
datalog/cozodb (run against the KB store).\n\n\
** Header arguments\n\
- `:cmd <path>` — override the interpreter/compiler for this block.\n\
- `:var name=value` — bind variables (interpreted languages only; compiled \
blocks use the raw body).\n\
- `:session <name>` — persistent REPL session (interpreted languages).\n\
- `:tangle <file>` — write the block to a file on `:babel-tangle`.\n\
- `:eval never` — block execution (safety policy).\n\n\
** Configuration\n\
- [[option:babel_timeout]] — per-block execution timeout (seconds).\n\
- [[option:babel_confirm]] — prompt before executing a block.\n\
- [[option:babel_cxx_compiler]] / [[option:babel_c_compiler]] — the C++/C \
compiler (default `c++` / `cc`); also overridable by the `MAE_BABEL_CXX` / \
`MAE_BABEL_CC` env vars or a per-block `:cmd`.\n\
- [[option:babel_cxx_std]] — C++ standard, passed as `-std=<value>` (default \
`c++17`).\n\n\
See also: [[concept:scheme-api]], [[concept:knowledge-base]], [[index]]\n";

pub(super) const CONCEPT_HOOKS: &str =
    "**Hooks** are MAE's primary extensibility mechanism — they let Scheme code react to \
editor events without the core knowing anything about Scheme.\n\n\
** Available hooks (25)\n\n\
*** Buffer lifecycle\n\
| Hook name | Fires when |\n\
|-----------|------------|\n\
| `before-save` | Just before a buffer is written to disk |\n\
| `after-save` | After a successful save |\n\
| `buffer-open` | After a file is opened into a buffer |\n\
| `buffer-close` | Before a buffer is killed |\n\
| `buffer-switch` | When active buffer changes |\n\
| `before-revert` | Before reverting a buffer to disk contents |\n\
| `after-revert` | After a successful revert |\n\
| `file-changed-on-disk` | When a buffer's backing file changes externally |\n\n\
*** Editing\n\
| Hook name | Fires when |\n\
|-----------|------------|\n\
| `after-insert` | After text is inserted via Scheme |\n\
| `after-delete` | After text is deleted via Scheme |\n\
| `mode-change` | When the editing mode changes |\n\n\
*** Commands\n\
| Hook name | Fires when |\n\
|-----------|------------|\n\
| `command-pre` | Before a command is dispatched |\n\
| `command-post` | After a command completes |\n\n\
*** Window & Focus\n\
| Hook name | Fires when |\n\
|-----------|------------|\n\
| `window-split` | After a window split |\n\
| `window-close` | After a window is closed |\n\
| `focus-in` | When editor gains focus |\n\
| `focus-out` | When editor loses focus |\n\n\
*** Application lifecycle\n\
| Hook name | Fires when |\n\
|-----------|------------|\n\
| `app-start` | During editor startup |\n\
| `app-exit` | During editor shutdown |\n\
| `after-load` | After a Scheme file is loaded (parameterized: `after-load:filename`) |\n\
| `module-loaded` | After a module loads (parameterized: `module-loaded:name`) |\n\
| `module-unloaded` | After a module unloads (reserved — no unload mechanism yet) |\n\n\
*** Configuration & state\n\
| Hook name | Fires when |\n\
|-----------|------------|\n\
| `option-change` | After an option is set (parameterized: `option-change:name`) |\n\
| `after-kb-change` | After knowledge base content changes |\n\
| `sync-update` | After a remote CRDT sync update is applied |\n\
| `idle` | After a period of user inactivity (reserved — needs timer infrastructure) |\n\n\
** Usage from Scheme\n\
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
\n\
;; Parameterized hooks — react to specific events:\n\
(add-hook! \"option-change:theme\" \"on-theme-change\")\n\
(add-hook! \"module-loaded:keymap-doom\" \"after-doom-loaded\")\n\
(add-hook! \"after-load:init.scm\" \"post-init-setup\")\n\
```\n\n\
** Event-driven primitives\n\
For Scheme code that needs to wait for hooks (e.g. in tests or async workflows):\n\
```scheme\n\
(import (mae async))\n\
(yield-tick)                    ; drain one event loop iteration\n\
(await-hook \"after-save\" 5000) ; suspend until hook fires (5s timeout)\n\
(await-condition pred 3000)     ; wait for predicate to become true\n\
```\n\n\
** Design\n\
Core fires hooks by pushing `(hook-name, fn-name)` entries into \
`Editor::pending_hook_evals`. The binary drains them and calls the Scheme runtime — \
the same intent pattern used for LSP and DAP. This keeps the core crate free of \
Scheme dependencies. The hook namespace is open — modules can register any hook name.\n\n\
See also: [[concept:command]], [[concept:options]], [[index]]\n";

pub(super) const CONCEPT_OPTIONS: &str =
    "MAE's editor options can be configured from Scheme using `(set-option! KEY VALUE)`.\n\n\
** Available options\n\
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
** Usage from Scheme\n\
```scheme\n\
;; In init.scm:\n\
(set-option! \"line-numbers\" \"true\")\n\
(set-option! \"relative-line-numbers\" \"true\")\n\
(set-option! \"theme\" \"dracula\")\n\
(set-option! \"word-wrap\" \"true\")\n\
(set-option! \"show-break\" \"↪ \")\n\
```\n\n\
** Toggle commands\n\
Options can also be toggled interactively via `SPC t`:\n\
- `SPC t l` — [[cmd:toggle-line-numbers]]\n\
- `SPC t r` — [[cmd:toggle-relative-line-numbers]]\n\
- `SPC t w` — [[cmd:toggle-word-wrap]]\n\
- `SPC t t` — [[cmd:cycle-theme]]\n\n\
See also: [[concept:hooks]], [[concept:command]], [[index]]\n";

pub(super) const CONCEPT_AGENT_BOOTSTRAP: &str =
    "MAE auto-configures AI agents running inside its embedded terminal so they \
can discover the editor's MCP tools with zero manual setup.\n\n\
** How it works\n\
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
** Commands\n\
- `:agent-setup <name>` — write `.mcp.json` and approval settings for an agent\n\
- `:agent-list` — show all agents MAE can bootstrap\n\
- `mae --setup-agents [DIR]` — CLI: write configs without starting the editor\n\n\
** Configuration\n\
Agent bootstrap is a startup-time concern (it runs at terminal-spawn), so it lives \
in the narrow legacy `config.toml` bootstrap, with env-var overrides:\n\
```toml\n\
[agents]\n\
auto_mcp_json = true       # write .mcp.json on terminal spawn\n\
auto_approve_tools = true  # write agent settings for tool approval\n\
```\n\
Env var overrides: `MAE_AGENTS_AUTO_MCP=0`, `MAE_AGENTS_AUTO_APPROVE=0`\n\n\
** Adding a new agent\n\
The bootstrap system is agent-agnostic. See the doc comments in `agents.rs` \
for how to add support for new AI agents. Claude Code is the reference \
implementation.\n\n\
** AI permission tiers (internal)\n\
MAE's own tool permissions are separate from agent approval. Use the \
`ai_permissions` tool or `MAE_AI_PERMISSIONS` env var to control what \
tier the AI auto-approves up to.\n\n\
See also: [[concept:terminal]], [[concept:ai-as-peer]], [[index]]\n";

pub(super) const CONCEPT_SELF_TEST: &str =
    "The **self-test** command (`:self-test`) tells the AI agent to exercise its own tool \
surface and report what works, what's broken, and what's unavailable.\n\n\
** Usage\n\
- `:self-test` — run all test categories.\n\
- `:self-test introspection` — run only the introspection category.\n\
- `:self-test editing,help` — run multiple specific categories.\n\n\
** Categories\n\
| Category | What it tests |\n\
|----------|---------------|\n\
| **introspection** | `cursor_info`, `editor_state`, `list_buffers`, `window_layout`, `command_list`, `ai_permissions` |\n\
| **editing** | `create_file`, `buffer_write`, `buffer_read`, `open_file`, `switch_buffer`, `close_buffer` |\n\
| **help** | `kb_search`, `kb_get`, `kb_list`, `kb_graph`, `kb_links_from`, `kb_links_to`, `help_open` |\n\
| **project** | `project_info`, `project_files`, `project_search` (needs git repo) |\n\
| **lsp** | `lsp_diagnostics`, `lsp_document_symbols` (needs LSP server) |\n\
| **dap** | `dap_start`, `dap_set_breakpoint`, `dap_step` (needs lldb-dap or debugpy) |\n\
| **git** | `git_status`, `git_diff`, `git_log`, `git_stash_list` (needs git repo) |\n\
| **modules** | `list_modules`, `describe-module`, module KB nodes |\n\
| **federation** | `kb_instances`, `kb_health`, `concept:kb-federation`, `concept:kb-workflows`, `concept:kb-vs-alternatives` |\n\
| **performance** | `introspect` timing metrics, lock contention, anomaly detection |\n\n\
** State management\n\
The self-test uses `editor_save_state` before tests and `editor_restore_state` after \
to leave the editor in a clean state regardless of pass/fail outcomes.\n\n\
** Reading results\n\
Results appear in the `*AI*` conversation buffer:\n\
- **[PASS]** — tool returned expected data.\n\
- **[FAIL]** — tool returned unexpected data or errored.\n\
- **[SKIP]** — prerequisite not met (e.g. no LSP server).\n\n\
The self-test also validates the command palette (key commands must exist) and \
runs a connected help-navigation walkthrough (search → get → graph → open).\n\n\
** Why this exists\n\
Unit tests validate individual components. The self-test validates the full \
AI↔editor integration: tool dispatch, permission checks, KB graph integrity, \
and command registration. It catches wiring bugs that unit tests can't reach.\n\n\
See also: [[concept:ai-as-peer]], [[concept:command]], [[concept:knowledge-base]], [[index]]\n";

pub(super) const CONCEPT_DEBUGGING: &str =
    "MAE integrates with the **Debug Adapter Protocol (DAP)** to provide a full \
debugging experience accessible to both the human user and the AI agent.\n\n\
** DAP client\n\
The DAP client connects to debug adapters via stdin/stdout. Built-in adapter \
presets: `lldb` (LLVM), `debugpy` (Python), `codelldb` (CodeLLDB / Rust+C++).\n\n\
** Debug panel\n\
The `*Debug*` buffer (`SPC d p` or `:debug-panel`) shows threads, stack frames, \
scopes, and variables in a navigable tree view.\n\n\
| Key | Action |\n\
|-----|--------|\n\
| `j`/`k` | Navigate up/down |\n\
| `Enter` | Expand/collapse node |\n\
| `o` | Open source at selected frame |\n\
| `q` | Close debug panel |\n\n\
** AI debug tools (13 tools)\n\
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
** Permission tiers\n\
- **Privileged** — `dap_start` (spawns processes).\n\
- **Write** — execution control (`dap_continue`, `dap_step`, `dap_set_breakpoint`, `dap_remove_breakpoint`, `dap_select_frame`, `dap_select_thread`, `dap_evaluate`, `dap_disconnect`).\n\
- **ReadOnly** — inspection (`dap_list_variables`, `dap_inspect_variable`, `dap_expand_variable`, `dap_output`).\n\n\
See also: [[concept:ai-as-peer]], [[cmd:debug-panel]], [[cmd:debug-start]], [[key:leader-keys]], [[index]]\n";

pub(super) const CONCEPT_GUI: &str =
    "MAE has a **dual rendering backend** — terminal (ratatui/crossterm) and GUI \
(winit + Skia 2D). Both backends share the same editor core, commands, and AI integration.\n\n\
** Launching\n\
- `mae --gui file.rs` — hardware-accelerated GUI window.\n\
- `mae file.rs` — terminal mode (default).\n\
- Desktop launcher: installed via `make install` to `~/.local/share/applications/mae.desktop`.\n\n\
** GUI features\n\
- **Mouse support:** click to place cursor, wheel scroll.\n\
- **Font configuration:** `:set font_size 16` (or `(set-option! \"font-size\" \"14.0\")` in init.scm).\n\
- **Dirty-flag rendering:** GPU idle when nothing changes (~0% CPU).\n\
- **Shell colors:** terminal emulator respects editor theme.\n\
- **Shell scrollback:** Shift-PageUp/PageDown.\n\
- **FPS overlay:** `SPC t F` or `:set show_fps true`.\n\n\
** Architecture\n\
The `Renderer` trait (in `mae-renderer`) defines the backend-agnostic HAL. The `mae-gui` \
crate implements it using winit for windowing and skia-safe for 2D rendering. The terminal \
backend uses ratatui/crossterm. The binary selects the backend at startup based on `--gui`.\n\n\
** Event loop\n\
- **Terminal:** `crossterm::EventStream` + tokio `select!`.\n\
- **GUI:** `winit::pump_app_events()` + tokio `select!` with dirty-flag gating.\n\n\
See also: [[concept:terminal]], [[concept:mode]], [[index]]\n";

pub(super) const CONCEPT_PACKAGE_SYSTEM: &str = "\
The **package system** enables Scheme-based extensions via `require`/`provide` \
and the **module system** for structured, discoverable packages.\n\n\
** Loading primitives\n\
- `(require \"feature\")` — searches `load-path` for `feature.scm` and evaluates it.\n\
- `(provide \"feature\")` — marks the current file as providing a feature.\n\
- `(featurep \"feature\")` — returns `#t` if the feature is loaded.\n\n\
** Load path\n\
Default: `~/.config/mae/packages/`, `~/.config/mae/lisp/`.\n\
- `(load-path)` — returns current search path as a list.\n\
- `(add-to-load-path! \"/path/to/dir\")` — prepends to search path.\n\n\
** Autoload\n\
`CommandSource::Autoload { feature }` enables deferred loading: when a command is first \
dispatched, `(require feature)` is triggered, then the command re-dispatches.\n\n\
** Module system (v0.9.0)\n\
Modules are self-contained packages with a `module.toml` manifest. They follow a \
Doom Emacs-inspired three-file model:\n\
- `module.toml` — identity, version, dependencies, flags\n\
- `autoloads.scm` — keybindings, command registration (eager, runs before user config)\n\
- `init.scm` — feature logic (lazy, loaded on first command use)\n\n\
See also: [[concept:modules]], [[concept:flags]], [[guide:extension-authoring]], [[index]]\n";

pub(super) const CONCEPT_OPTION_REGISTRY: &str = "\
The **option registry** (`options.rs`) is the single source of truth for all editor settings.\n\n\
Each `OptionDef` has: name, aliases, kind, default, config_key, doc, valid_values.\n\
Kinds: `Bool`, `String`, `Float`, `Int`, `Theme`.\n\n\
** Flow\n\
1. `:set foo bar` → `Editor::set_option(\"foo\", \"bar\")`\n\
2. Validates kind + range → sets field on `Editor`\n\
3. `get_option(name)` reads back the current value\n\n\
** Scheme\n\
- `(set-option! \"name\" \"value\")` — from Scheme\n\
- `(get-option \"name\")` — returns current value as string\n\
- `*option-list*` — all options as `(name kind default doc)` tuples\n\n\
** Range clamping\n\
Options with numeric types are clamped to valid ranges in `set_option()` to prevent \
rendering corruption (e.g. heading_scale ≤0 → infinite loop).\n\n\
See also: [[concept:command]], [[concept:hooks]], [[index]]\n";

pub(super) const CONCEPT_SCHEME_API: &str = "\
MAE exposes ~50 Scheme functions to extension code. They fall into categories:\n\n\
** Buffer editing\n\
`buffer-insert`, `buffer-delete-range`, `buffer-replace-range`, `buffer-undo`, `buffer-redo`\n\n\
** Buffer read\n\
`*buffer-name*`, `*buffer-text*`, `*buffer-char-count*`, `buffer-text-range`, \
`*buffer-list*`, `get-buffer-by-name`\n\n\
** Cursor / navigation\n\
`cursor-goto`, `*cursor-row*`, `*cursor-col*`, `open-file`, `switch-to-buffer`\n\n\
** Windows\n\
`*window-count*`, `*window-list*`\n\n\
** Options / commands\n\
`set-option!`, `set-local-option!`, `get-option`, `*option-list*`, \
`define-command`, `run-command`, `command-exists?`, `*command-list*`\n\n\
** Keymaps\n\
`define-key`, `define-keymap`, `undefine-key!`, `*keymap-list*`, `keymap-bindings`\n\n\
** File I/O\n\
`read-file`, `file-exists?`, `list-directory`\n\n\
** Architecture\n\
Write-side: `SharedState` (Arc<Mutex>) accumulates `pending_*` fields during eval.\n\
Read-side: `inject_editor_state()` snapshots editor state as globals before eval.\n\
Apply: `apply_to_editor()` drains pending changes after eval.\n\n\
See also: [[concept:hooks]], [[concept:options]], [[index]]\n";

pub(super) const CONCEPT_AI_MODES: &str = "\
MAE provides two distinct AI interfaces, each suited to different workflows.\n\n\
** AI Agent (`SPC a a`)\n\
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
** AI Chat (`SPC a p`)\n\
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
** Shared configuration\n\
Both interfaces respect:\n\
- **Permission tiers:** `readonly`, `standard`, `trusted`, `privileged`\n\
- **Budget limits:** `budget_warn_tokens`, `budget_limit_tokens`\n\
- **API keys:** env vars (ANTHROPIC_API_KEY, etc.) or `api_key_command`\n\n\
See also: [[tutorial:ai-setup]], [[concept:ai-as-peer]], [[index]]\n";

pub(super) const CONCEPT_CONCEAL: &str = "\
**Link & Markup Rendering** controls how inline markup is displayed — \
showing styled labels instead of raw syntax.\n\n\
** Options\n\
| Option | Default | Description |\n\
|--------|---------|-------------|\n\
| `link_descriptive` | `true` | Strip `[label](url)` markup, show styled label only |\n\
| `render_markup` | `true` | Render `**bold**`, `` `code` ``, `*bold*`, `/italic/`, `=code=`, `~verbatim~` with styling |\n\n\
** Configuration\n\
- `:set link_descriptive false` — show raw `[label](url)` text\n\
- `:set render_markup false` — disable inline styling in conversation buffers\n\
- `:setlocal nolink_descriptive` — per-buffer override\n\
- Scheme (in init.scm, the primary config surface): `(set-option! \"link-descriptive\" \"true\")` \
(persists via `:set-save`)\n\n\
** Scope\n\
- **Conversation buffers:** markdown links are stripped to labels; org and markdown \
inline markup (bold, italic, code) get styling spans\n\
- **Help buffers:** both markdown and org inline markup are styled\n\
- Links are clickable via `gx` (`open-link-at-cursor`)\n\n\
** Safety\n\
Inline markup spans intentionally exclude `markup.heading` — heading spans \
would trigger `line_heading_scale()` in `compute_layout()`, breaking uniform \
line heights in conversation buffers.\n\n\
See also: [[concept:options]], [[concept:buffer]], [[concept:ai-as-peer]]\n";

pub(super) const CONCEPT_MODULES: &str = "\
The **module system** is MAE's structured package infrastructure, introduced in v0.9.0. \
It follows the Doom Emacs model: modules are self-contained Scheme packages that register \
commands, keybindings, options, and hooks — composing on the kernel rather than inheriting from it.\n\n\
** Structure\n\
Every module has three files:\n\
| File | Purpose | When loaded |\n\
|------|---------|-------------|\n\
| `module.toml` | Manifest: name, version, deps, flags | Before Scheme init (TOML parsing) |\n\
| `autoloads.scm` | Keybindings, command stubs, options | Eager: after init.scm, before config.scm |\n\
| `init.scm` | Feature logic, provides feature | Lazy: on first command use |\n\n\
** Loading order\n\
```\n\
config.toml → SchemeRuntime::new()\n\
  → init.scm (module declarations)\n\
  → discover_modules() → resolve_deps() → load_module_autoloads()\n\
  → config.scm (user customization — runs AFTER module autoloads)\n\
```\n\n\
**Key invariant:** Module autoloads run BEFORE `config.scm`. Users can override any \
module keybinding or option in their config.\n\n\
** Built-in modules\n\
MAE ships 9 built-in modules: dashboard, surround, marks-jumps, search, registers, \
macros, tables, multicursor, file-tree. Each extracts keybindings from hardcoded Rust \
into Scheme autoloads while commands remain in the kernel.\n\n\
** Manifest (`module.toml`)\n\
```toml\n\
[module]\n\
name = \"search\"\n\
version = \"0.1.0\"\n\
description = \"Search and highlight\"\n\
category = \"editor\"\n\
mae_version = \">=0.9.0\"\n\
```\n\n\
** CLI\n\
- `mae pkg list` — show all discovered modules\n\
- `mae pkg doctor [NAME]` — validate manifests and entry points\n\
- `mae pkg info <NAME>` — detailed module information\n\
- `mae pkg create <NAME>` — scaffold a new module\n\n\
** Introspection\n\
- `:describe-module <name>` — manifest, commands, options, status\n\
- `:describe-bindings` — full keymap table for current mode\n\
- `:describe-mode` — current buffer's mode, keymap, and options\n\
- `(module-loaded? \"name\")` — check if a module is loaded\n\
- `(module-list)` — list all active modules\n\n\
** Live reload\n\
`:module-reload <name>` unregisters and re-evaluates a module's autoloads \
without restarting the editor.\n\n\
See also: [[concept:package-system]], [[concept:flags]], [[concept:design-philosophy]], \
[[guide:extension-authoring]], [[index]]\n";

pub(super) const CONCEPT_FLAGS: &str = "\
**Flags** are optional sub-features that modules can declare. They follow Doom Emacs's \
`+flag` syntax and enable conditional loading of module functionality.\n\n\
** Declaration\n\
Flags are declared in `module.toml`:\n\
```toml\n\
[flags]\n\
agenda = { doc = \"Task/schedule agenda view across org files\" }\n\
babel = { doc = \"Code block execution and tangling\" }\n\
```\n\n\
** Usage in Scheme\n\
```scheme\n\
(when-flag \"+agenda\"\n\
  (autoload \"open-agenda\" \"org-agenda\" \"Open agenda view\")\n\
  (define-key \"normal\" \"SPC o a\" \"open-agenda\"))\n\
```\n\n\
** Enabling flags\n\
Flags are enabled in `init.scm` via the `mae!` macro. Each `:category` marker is\n\
cosmetic (it just organizes the list); a bare string enables a module with no\n\
flags, and `(list \"module\" \"+flag\" ...)` enables one with flags:\n\
```scheme\n\
(mae!\n\
  :lang\n\
    (list \"org\" \"+agenda\" \"+babel\")\n\
  :editor\n\
    (list \"multicursor\" \"+align\"))\n\
```\n\n\
** Validation\n\
`mae pkg doctor` warns about:\n\
- Unknown flags (typos)\n\
- Flags declared but never checked\n\
- Missing flag documentation\n\n\
See also: [[concept:modules]], [[concept:package-system]], [[index]]\n";

pub(super) const CONCEPT_DESIGN_PHILOSOPHY: &str = "\
MAE's **design philosophy** for modules follows four principles derived from \
35 years of Emacs history.\n\n\
** 1. Composition over inheritance\n\
- Register commands, not subclasses\n\
- Compose via hooks, not method overrides\n\
- Extend via keymaps with parent chains, not by patching existing maps\n\
- Configure via options, not by mutating global state\n\n\
** 2. Single source of truth\n\
The Scheme code is both the declaration AND the implementation. \
`module.toml` declares identity only (name, version, deps, flags). \
Everything the module provides (commands, options, keybindings) is registered \
exclusively in `autoloads.scm`.\n\n\
** 3. Stable API contract\n\
The module API is the set of Scheme functions, hooks, options, commands, and keymaps \
that modules can rely on:\n\
- **Additions** = minor version bump, never breaking\n\
- **Removals/renames** = major version bump, with deprecation cycle\n\
- `mae_version` constraint in `module.toml` declares minimum MAE version\n\n\
** 4. No framework, no SDK\n\
Modules import nothing — they call registered Scheme functions:\n\
```scheme\n\
(define-command \"my-cmd\" (lambda () (buffer-insert \"hello\")) \"Insert hello\")\n\
(define-key \"normal\" \"SPC x h\" \"my-cmd\")\n\
(define-option! \"my_option\" \"string\" \"default\" \"My option doc\")\n\
(add-hook! \"after-save\" \"my-after-save-fn\")\n\
(provide-feature \"my-module\")\n\
```\n\n\
** Kernel boundary\n\
The line: **if it needs `tokio`, PTY, or FFI, it's kernel. If it's commands + \
keybindings + hooks + options, it's a module.** LSP, DAP, Shell, and AI stay \
as Rust crates.\n\n\
** Pitfall avoidance\n\
| Pitfall | Source | MAE avoidance |\n\
|---------|--------|---------------|\n\
| Namespace pollution | Emacs | Convention prefix + `mae pkg doctor` warnings |\n\
| Load-order hell | Emacs | Topo-sorted autoloads → config.scm |\n\
| Silent command shadowing | Emacs | Conflict warnings on duplicate registration |\n\
| Metadata in comments | Emacs | Structured `module.toml` |\n\
| Implicit flags | Doom | Flags declared in `module.toml [flags]` |\n\n\
See also: [[concept:modules]], [[concept:ai-as-peer]], [[index]]\n";

pub(super) const GUIDE_EXTENSION_AUTHORING: &str = "\
# Extension Authoring Guide\n\n\
This guide covers creating, testing, and publishing MAE modules.\n\n\
** Quick start\n\
```bash\n\
mae pkg create my-module\n\
cd modules/my-module\n\
# edit module.toml, autoloads.scm, init.scm\n\
mae pkg doctor my-module\n\
```\n\n\
** Module structure\n\n\
Every module has three files:\n\n\
*** `module.toml` — identity\n\
```toml\n\
[module]\n\
name = \"my-module\"\n\
version = \"0.1.0\"\n\
description = \"What the module does\"\n\
category = \"editor\"    # editor | lang | tools | ui | ai | completion\n\
mae_version = \">=0.9.0\"\n\
\n\
[flags]\n\
extra = { doc = \"Enable extra feature\" }\n\
\n\
[dependencies]\n\
# other-module = \">=0.1.0\"\n\
\n\
[entry]\n\
init = \"init.scm\"\n\
autoloads = \"autoloads.scm\"\n\
```\n\n\
*** `autoloads.scm` — eager registration\n\
Runs at startup before user config. Register commands, keybindings, options:\n\
```scheme\n\
;; Commands\n\
(define-command \"my-greet\" (lambda () (buffer-insert \"Hello!\")) \"Greet\")\n\
\n\
;; Keybindings\n\
(define-key \"normal\" \"SPC x g\" \"my-greet\")\n\
\n\
;; Options\n\
(define-option! \"my_greeting\" \"string\" \"Hello!\" \"The greeting text\")\n\
\n\
;; Hooks\n\
(add-hook! \"after-save\" \"my-after-save-fn\")\n\
\n\
;; Conditional on flags\n\
(when-flag \"+extra\"\n\
  (define-key \"normal\" \"SPC x e\" \"my-extra-cmd\"))\n\
```\n\n\
*** `init.scm` — lazy initialization\n\
Loaded on first command use via autoload:\n\
```scheme\n\
;; Full feature logic here\n\
(provide-feature \"my-module\")\n\
```\n\n\
** Scheme API reference\n\n\
*** Buffer operations\n\
- `(buffer-insert text)` — insert text at cursor\n\
- `(buffer-delete-range start end)` — delete range\n\
- `(buffer-replace-range start end text)` — replace range\n\
- `(buffer-text-range start end)` — read range\n\
- `*buffer-name*`, `*buffer-text*`, `*buffer-char-count*` — buffer state\n\n\
*** Cursor & navigation\n\
- `(cursor-goto row col)` — move cursor\n\
- `*cursor-row*`, `*cursor-col*` — current position\n\
- `(open-file path)` — open a file\n\
- `(switch-to-buffer name)` — switch buffer\n\n\
*** Commands\n\
- `(define-command name fn doc)` — register a command\n\
- `(run-command name)` — execute a command\n\
- `(command-exists? name)` — check if registered\n\
- `(undefine-command! name)` — remove (for unload)\n\n\
*** Keymaps\n\
- `(define-key keymap key command)` — bind key to command\n\
- `(define-keymap name parent)` — create a new keymap\n\
- `(undefine-key! keymap key)` — remove binding\n\n\
*** Options\n\
- `(define-option! name type default doc)` — register option\n\
- `(set-option! name value)` — set value\n\
- `(get-option name)` — read value\n\
- `(undefine-option! name)` — remove (for unload)\n\n\
*** Hooks\n\
- `(add-hook! hook fn)` — subscribe to event\n\
- `(remove-hook! hook fn)` — unsubscribe\n\
- `(run-hook! hook)` — trigger event\n\n\
*** Module queries\n\
- `(module-loaded? name)` — is module active?\n\
- `(module-version name)` — version string or `#f`\n\
- `(module-list)` — all active module names\n\
- `(module-flags name)` — enabled flags for module\n\
- `(when-module name fn)` — conditional on module presence\n\
- `(when-flag flag fn)` — conditional on flag\n\n\
*** Deprecation\n\
- `(deprecate-function! old new since)` — register deprecation\n\
- `(check-deprecated name)` — check and warn once\n\n\
** Testing your module\n\n\
1. **Manifest validation:** `mae pkg doctor my-module`\n\
2. **Load validation:** `mae --check-config` (loads all modules)\n\
3. **Live development:** `:module-reload my-module` after editing\n\
4. **Disable test:** comment out module in init.scm, verify editor starts\n\
5. **Override test:** add overrides in config.scm, verify they take effect\n\n\
** Naming conventions\n\n\
Module authors MUST prefix definitions with the module name:\n\
- Commands: `my-module-command` (not `command`)\n\
- Options: `my_module_option` (underscores for TOML)\n\
- Scheme symbols: `my-module-helper` (not `helper`)\n\n\
`mae pkg doctor` warns about unprefixed definitions.\n\n\
** Reference implementation\n\n\
The `dashboard` module is the simplest complete example. See `modules/dashboard/` \
for the canonical three-file pattern.\n\n\
For a more complex example with module-owned keymaps, see `modules/file-tree/`.\n\n\
See also: [[concept:modules]], [[concept:flags]], [[concept:design-philosophy]], \
[[concept:package-system]], [[concept:scheme-api]], [[index]]\n";

pub(super) const CONCEPT_SYNC_ENGINE: &str = "\
The **Sync Engine** is MAE's collaborative state layer, built on \
yrs (the Rust port of Yjs, using the YATA algorithm). Crate: `yrs` on crates.io.\n\n\
** Why yrs\n\
- Handles text (`YText`), structured documents (`YMap`, `YArray`), and \
  knowledge base nodes in a single framework\n\
- Built-in `UndoManager` with per-user stacks\n\
- Awareness protocol for cursor/selection sharing\n\
- Proven at scale: Notion (200M+ users), Excalidraw, TLDraw\n\
- YATA algorithm: O(n) space, optimized for sequential typing\n\n\
** Dual Structure\n\
yrs `YText` is the source of truth for collaborative state. ropey remains \
the rendering engine (efficient line indexing via `Rope::line()`). A bridge \
rebuilds the rope from YText on remote changes (~1ms for 10K lines).\n\n\
** Document Types\n\
| Type | yrs Representation | Use Case |\n\
|------|-------------------|----------|\n\
| Text buffer | `YText` | Code editing |\n\
| Visual element | `YMap { position, style, children }` | Design system |\n\
| KB node | `YMap { title: YText, body: YText, tags: YArray }` | Knowledge base |\n\n\
See also: [[concept:collaborative-state]], [[concept:adr-text-sync]], [[concept:adr-kb-crdt]]\n";

pub(super) const CONCEPT_COLLABORATIVE_STATE: &str = "\
MAE is a **collaborative state engine** where AI and humans interact via text \
OR visual interfaces, backed by a federated knowledge base. The sync layer \
(powered by [[concept:sync-engine|yrs]]) is the universal substrate for ALL state:\n\n\
- **Editor buffers** — text and code (YText)\n\
- **Visual documents** — design components, scene graphs (YMap/YArray)\n\
- **Knowledge base nodes** — CRDT-synced across instances for offline editing\n\n\
** Requirements\n\
1. Real-time multi-user collaboration (text AND visual content)\n\
2. AI agents as collaborative peers (sequential tool calls → yrs transactions)\n\
3. Non-textual documents: scene graphs, component trees, design tokens\n\
4. KB nodes as CRDT documents — offline editing, conflict-free merge, P2P federation\n\
5. Sustainable maintenance for a small team (~1000 lines MAE-specific sync code)\n\
6. Performance: 100+ concurrent clients, 100K+ element documents\n\n\
** Transport\n\
JSON-RPC 2.0 over Unix sockets (extend existing MCP protocol). Upgrade path: \
msgpack wire format, then TCP for multi-machine. See [[concept:adr-text-sync]].\n\n\
See also: [[concept:sync-engine]], [[concept:knowledge-base]], [[concept:ai-as-peer]]\n";

pub(super) const CONCEPT_ADR_TEXT_SYNC: &str = "\
**ADR-002: Text Synchronization Model** — Status: **Accepted (yrs/YATA)**\n\n\
** Decision\n\
Use yrs (Yjs Rust port) as the sync engine for all collaborative state. \
Dual structure: yrs YText + ropey mirror for rendering.\n\n\
** Key Rationale\n\
- MAE needs to sync structured documents (visual elements, KB nodes), not just text\n\
- yrs provides YText, YMap, YArray — handles all content types\n\
- Built-in UndoManager eliminates custom undo work\n\
- Yjs ecosystem is the de-facto standard (Notion, Excalidraw, TLDraw)\n\n\
** Alternatives Rejected\n\
| Library | Why Not |\n\
|---------|--------|\n\
| automerge-rs | Performance cliff >100K ops, no built-in undo |\n\
| diamond-types | Text-only, bus factor = 1 |\n\
| Custom OT | Combinatorial explosion for visual operations |\n\n\
Full ADR: `docs/adr/002-text-sync-model.md`\n\n\
See also: [[concept:sync-engine]], [[concept:collaborative-state]], [[concept:adr-kb-crdt]]\n";

pub(super) const CONCEPT_ADR_KB_CRDT: &str = "\
**ADR-005: KB Nodes as CRDT Documents** — Status: **Accepted**\n\n\
** Decision\n\
Each KB node becomes a yrs document with schema:\n\
```\n\
YMap { id, title: YText, body: YText, tags: YArray, links: YArray, meta: YMap }\n\
```\n\n\
CozoDB remains the persistence backend — yrs document bytes stored as a column. \
The FTS index covers materialized text from `YText::to_string()`.\n\n\
** Benefits\n\
- **Offline editing**: Edit KB nodes without connectivity, merge on reconnect\n\
- **P2P federation**: Exchange yrs state vectors between MAE instances\n\
- **AI attribution**: Each transaction carries a client ID\n\
- **Per-user undo**: yrs UndoManager provides this automatically\n\n\
** Migration Path\n\
1. Phase A: plain CozoDB nodes only (current)\n\
2. Phase B: Optional `crdt_doc` column, new nodes get yrs docs\n\
3. Phase C: All nodes have yrs docs, CozoDB is read cache + FTS index\n\n\
Full ADR: `docs/adr/005-kb-crdt.md`\n\n\
See also: [[concept:sync-engine]], [[concept:knowledge-base]], [[concept:collaborative-state]]\n";

pub(super) const CONCEPT_ADR_KEYMAP_RESOLUTION: &str = "\
**ADR-015: Unified Layered Keymap Resolution Chain** — Status: **Accepted**\n\n\
** Decision\n\
Resolve keystrokes against ONE ordered keymap chain (most-specific layer first), \
consumed identically by dispatch, the which-key popup, and describe-bindings. \
`Editor::keymap_chain()` is the single source of truth; `Keymap::lookup` stays the \
single-map hot primitive and `Keymap.parent` is demoted to introspection sugar.\n\n\
** Why\n\
- Dispatch used a flat (primary, fallback) pair while describe-bindings walked the \
parent chain N levels — they agreed only by convention, so a 3-deep chain would \
dispatch and display differently (latent bug).\n\
- Buffer-context routing was a hardcoded Rust match (BufferMode::keymap_name + \
org/markdown), violating principle #7 (no kernel patch to extend).\n\n\
** Mechanism\n\
- A data-driven `KeymapRegistry` (kernel-seeded, re-seeded on \
reset_keymaps_to_kernel) maps BufferKind/Language -> context keymap; modules \
extend it from Scheme via `(bind-context-keymap …)`.\n\
- First consumer: the shared `navigation` context for read-only nav buffers \
(flavor-independent movement + both SPC and C-; open the leader).\n\n\
Full ADR: `docs/adr/015-keymap-resolution-chain.md`\n\n\
See also: [[concept:adr-artifact-interaction]], [[concept:scheme-api]], [[concept:keymap-inheritance]]\n";

pub(super) const CONCEPT_ADR_ARTIFACT_INTERACTION: &str = "\
**ADR-016: ArtifactType Axis & Interaction Model for Non-Text Artifacts** — \
Status: **Proposed**\n\n\
** Decision\n\
Decompose the conflated `Mode` enum into four orthogonal, registry-driven axes: \
modality, ArtifactType, buffer-kind context, and a stacked transient-overlay \
layer. Each artifact type (text/visual-canvas/kb-graph/terminal) declares its own \
modalities and interaction keymaps.\n\n\
** Why\n\
- `Mode` mixes input modality (Normal/Insert/Visual), transient UI (Command/Search/\
Palette), and artifact input (ShellInsert) — no clean seam for non-text CRDT \
artifacts (principle #11: canvas=YMap/YArray, KB nodes=yrs docs are first-class).\n\n\
** Phases (follow-up PRs, build on ADR-015)\n\
- Phase 2: extract transient overlays from `Mode` into a composable overlay stack \
(capture vs layer), enabling e.g. completion-in-search.\n\
- Phase 3: ArtifactType + per-artifact modalities; canvas/kb-graph interaction \
commands mutate the yrs doc (not the render mirror) and double as the AI peer's \
MCP tool-calls.\n\n\
Full ADR: `docs/adr/016-artifact-interaction-model.md`\n\n\
See also: [[concept:adr-keymap-resolution]], [[concept:adr-text-sync]], [[concept:collaborative-state]]\n";

pub(super) const CONCEPT_COLLAB_ARCHITECTURE: &str = "\
**Collaborative Editing Architecture** describes how MAE synchronises editor \
state across multiple clients — from solo AI agents on a single machine to \
multi-user sessions over a LAN or the internet.\n\n\
** Document Addressing\n\
Every collaborative document is identified by a URI with one of three namespaces:\n\
| Namespace | Example | Meaning |\n\
|-----------|---------|--------|\n\
| `file:` | `file:///home/user/project/main.rs` | Local or remote file buffer |\n\
| `kb:` | `kb://default/concept:collab-architecture` | Knowledge-base node |\n\
| `shared:` | `shared://session-id/scratchpad` | Anonymous shared document |\n\n\
** Data Flow\n\
```\n\
Local editor\n\
  └─ user/AI edit → yrs transaction (YText insert/delete)\n\
       └─ mae-sync encodes update bytes\n\
            └─ TCP framed write → mae-daemon (sync/update)\n\
                 └─ server applies to doc store, WAL flush\n\
                      └─ broadcast diff → connected peers\n\
                           └─ peer decodes → ropey mirror rebuild → redraw\n\
```\n\n\
** Save Protocol\n\
File saves use content-hash verification (SHA-256) to guard against silent \
mtime failures. Before writing, MAE reads the current on-disk bytes, computes \
their SHA-256, and compares it with the last-known hash. If they differ an \
external modification warning is raised. After writing, the new hash is \
stored as the baseline. Advisory lock files (`.{name}.mae.lock`) prevent \
simultaneous writes from two editor instances.\n\n\
** Daemon Role\n\
The `mae-daemon` binary is a **document hub**, not a source of truth. \
Documents are authoritative at the client; the server:\n\
- Holds the latest merged CRDT state (yrs doc bytes)\n\
- Appends every `sync/update` to a SQLite WAL before applying to memory\n\
- Broadcasts diffs to all connected peers (bounded queues, write timeout 5 s)\n\
- Compacts WAL into a snapshot once the WAL exceeds the configured threshold (default 500 entries)\n\
- Recovers by loading the latest snapshot then replaying the WAL tail on restart\n\n\
** Three Workflow Tiers\n\
| Tier | Server | Use case |\n\
|------|--------|----------|\n\
| **Solo** | none | Single user, no collaboration needed |\n\
| **Loopback** | `127.0.0.1:9473` | Multiple MAE instances or AI agents on one machine |\n\
| **Collaborative** | remote host | Multi-user editing across machines |\n\n\
In solo mode the sync layer is still active locally — edits are yrs \
transactions — but no TCP connection is opened. This means switching from \
solo to loopback requires only `(set-option! \"collab-server-address\" \"127.0.0.1:9473\")` \
and a reconnect; no data migration is needed.\n\n\
See also: [[concept:sync-engine]], [[concept:collab-workflows]], \
[[concept:collaborative-state]], [[concept:adr-text-sync]], [[index]]\n";

pub(super) const CONCEPT_COLLAB_WORKFLOWS: &str = "\
**Collaborative Editing Workflows** — practical recipes for the three tiers \
of MAE collaboration.\n\n\
** Solo Mode\n\
No mae-daemon is required. MAE operates entirely locally. All edits are \
still yrs transactions, which means:\n\
- Full undo/redo with per-user attribution\n\
- Zero configuration changes needed\n\
- Instant upgrade path to loopback or collaborative mode\n\n\
** Loopback Mode (Local Multi-Agent)\n\
Run `mae-daemon` on the same machine to coordinate multiple MAE \
instances or AI agents on the same project.\n\n\
```bash\n\
mae-daemon                      # listens on 127.0.0.1:9473\n\
```\n\n\
Then in each MAE instance:\n\
```scheme\n\
(set-option! \"collab-server-address\" \"127.0.0.1:9473\")\n\
(set-option! \"collab-auto-connect\" \"true\")\n\
```\n\n\
Or interactively: `SPC C s` to start a local server, `SPC C c` to connect.\n\n\
** Collaborative Mode (Multi-User)\n\
Point all clients at a shared server:\n\
```scheme\n\
(set-option! \"collab-server-address\" \"192.168.1.10:9473\")\n\
```\n\n\
The server can be started with:\n\
```bash\n\
mae-daemon --bind 0.0.0.0:9473\n\
```\n\n\
> **Security:** the auth mode is one of `none`, `psk`, or `key`. The recommended \
> mode is `key` — Ed25519 trusted-peer mutual TLS, where access keys on each peer's \
> verified key fingerprint (no shared secret to leak). `psk` (HMAC-SHA256 shared key) \
> remains available for simple setups. Never store secrets in plaintext `config.toml`: \
> use the trusted-peer keystore (`key` mode) or `collab_psk_command`. For untrusted \
> networks, use a VPN regardless.\n\n\
** Commands\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `SPC C s` | `:collab-start` | Start a local mae-daemon |\n\
| `SPC C c` | `:collab-connect` | Connect to configured server |\n\
| `SPC C d` | `:collab-disconnect` | Disconnect from server |\n\
| `SPC C S` | `:collab-share` | Share current buffer with peers |\n\
| `SPC C i` | `:collab-status` | Show connection + peer status |\n\n\
** Configuration Options\n\
| Option | Default | Description |\n\
|--------|---------|-------------|\n\
| `collab-server-address` | `\"\"` | Server host:port (empty = solo mode) |\n\
| `collab-auto-connect` | `\"false\"` | Connect on startup if address is set |\n\n\
** Diagnostics\n\
- `:collab-doctor` — comprehensive diagnostic: server reachability, WAL health, peer list\n\
- `:collab-status` — live connection state, document list, peer cursors\n\
- `mae doctor` (CLI) — checks mae-daemon process, port binding, WAL integrity\n\n\
See also: [[concept:collab-architecture]], [[lesson:collab-setup]], \
[[concept:sync-engine]], [[index]]\n";

pub(super) const CONCEPT_SCHEME_TESTING: &str = "\
MAE has a headless **Scheme test framework** inspired by Emacs ERT/Buttercup \
and Neovim Plenary. Tests boot a real editor (no mocks) and exercise the same \
Scheme API available to users.\n\n\
** BDD Structure\n\
Tests use `describe-group` / `it-test` blocks (like Buttercup's `describe`/`it`):\n\n\
```scheme\n\
(describe-group \"Feature name\"\n\
  (lambda ()\n\
    (it-test \"setup\"\n\
      (lambda () (create-buffer \"*test*\")))\n\
    (it-test \"insert text\"\n\
      (lambda () (buffer-insert \"hello\")))\n\
    (it-test \"verify\"\n\
      (lambda () (should-equal (buffer-string) \"hello\")))))\n\
```\n\n\
** Assertions\n\
| Function | Purpose |\n\
|----------|----------|\n\
| [[scheme:should]] | Assert truthy |\n\
| [[scheme:should-not]] | Assert falsy |\n\
| [[scheme:should-equal]] | Assert equality |\n\
| [[scheme:should-contain]] | Assert substring |\n\
| `(should-mode MODE)` | Assert editor mode |\n\n\
** Running Tests\n\
```\n\
mae --test tests/crdt/               # CRDT sync tests\n\
mae --test tests/editor/             # Editor feature tests\n\
mae --test tests/collab-e2e/test_smoke.scm  # Single file\n\
```\n\n\
** Key Principle: One Op Per Step\n\
Each `it-test` is one eval→apply cycle. Pending mutations (`buffer-insert`, \
`goto-char`, etc.) execute during `apply_to_editor` after eval completes. \
Multiple mutations in one step may execute in unexpected order — split them.\n\n\
See also: [[concept:test-runner]], [[scheme:describe-group]], \
[[scheme:it-test]], [[index]]\n";

pub(super) const CONCEPT_TEST_RUNNER: &str = "\
The **headless test runner** (`mae --test PATH`) orchestrates Scheme test \
execution from the Rust side. It is the canonical path for all tests.\n\n\
** Architecture (3 layers)\n\
1. **`scheme/lib/mae-test.scm`** — BDD library (describe/it/should/TAP output)\n\
2. **`crates/mae/src/test_runner.rs`** — Rust orchestrator\n\
3. **`crates/scheme/src/runtime.rs`** — Scheme primitives\n\n\
** Execution Flow\n\
1. Boot editor headless (no terminal/GUI)\n\
2. Load `mae-test.scm` library\n\
3. Load test file(s) → registers tests via `describe-group`/`it-test`\n\
4. Iterate tests from Rust: `eval(\"(run-nth-test N)\")` for each test\n\
5. Between each test: `apply_to_editor()` + `sync_scheme_state()`\n\
6. Print TAP v14 output, exit 0 (pass) or 1 (fail)\n\n\
** SharedState Pattern\n\
Mutable editor state is stored in `Arc<Mutex<SharedState>>` and registered \
Rust functions read from it. Functions like `buffer-string`, \
`buffer-sync-enabled?`, `current-mode`, and `get-buffer-by-name` always \
return fresh data from SharedState. `inject_editor_state()` updates both \
the VM globals and SharedState in a single call.\n\n\
** Adding New Test Primitives\n\
- **Read-only**: Add to SharedState → register `test-*` Rust fn → add \
  Scheme forwarding in `install_mutable_buffer_accessors` → update in \
  `sync_scheme_state`\n\
- **Mutations**: Add pending field to SharedState → register Scheme fn → \
  process in `apply_to_editor`\n\n\
See also: [[concept:scheme-testing]], [[concept:scheme-api]], [[index]]\n";

pub(super) const CONCEPT_KB_SHARING: &str = "\
**KB Sharing** enables collaborative editing of knowledge bases across MAE \
instances connected via the mae-daemon.\n\n\
** How It Works\n\
Each KB node is a yrs CRDT document (see [[concept:adr-kb-crdt]]). When you \
share a KB, all its nodes are registered as collaborative documents on the \
mae-daemon. Other clients can join the shared KB and receive real-time \
updates as nodes are edited.\n\n\
** Commands\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `SPC C K s` | `:kb-share` | Share the primary KB (or named instance) |\n\
| `SPC C K j` | `:kb-join` | Join a shared KB from the server |\n\
| `SPC C K l` | `:kb-leave` | Leave a shared KB (local copy preserved) |\n\
| `SPC C K r` | `:kb-list-remote` | List available shared KBs on the server |\n\n\
** Sync Modes\n\
The `collab_kb_sync_mode` option controls when local edits are pushed:\n\
- `\"on_save\"` (default): push CRDT updates when a node is saved\n\
- `\"manual\"`: only sync when explicitly requested via `:collab-sync`\n\n\
** Offline Support\n\
When disconnected, edits accumulate in a pending queue. On reconnect, \
the queue drains automatically — CRDT merge guarantees conflict-free \
convergence regardless of editing order.\n\n\
** Status Line\n\
When KB sharing is active, the status bar shows `[KB:N|status]` where \
N is the number of shared KBs and status is `synced`, `offline`, or \
`pending` (has unsent updates).\n\n\
** Data Storage\n\
- Local KBs: `$XDG_DATA_HOME/mae/kb/local/{slug}/kb.sqlite`\n\
- Shared KBs: `$XDG_DATA_HOME/mae/kb/shared/{slug}/kb.sqlite`\n\n\
** mDNS Discovery\n\
On a LAN, `:collab-discover` uses mDNS (`_mae-sync._tcp.local`) to find \
peers running `mae-daemon`. Connect to a discovered peer to browse \
their shared KBs.\n\n\
** Authentication\n\
Auth mode is `none`, `psk`, or `key`. The recommended mode is `key` — Ed25519 \
trusted-peer mutual TLS keyed on each peer's verified key fingerprint (see the \
access-control section below). `psk` mode (`collab_psk` / `collab_psk_command`, \
HMAC-SHA256, shared secret) remains available for simple setups. Never put secrets \
in plaintext `config.toml`.\n\n\
** Access Control: identity, roles, policy (ADR-018)\n\
Ownership and membership key on your **Ed25519 key fingerprint** (`SHA256:…`), \
NOT your label or `collab-user-name` (display only). On `:kb-share` the daemon \
binds the owner to your verified key; a self-claimed creator is ignored.\n\
- Roles (hierarchical, owner ⊇ editor ⊇ viewer): owner manages members + \
policy; editor reads + edits; **viewer is read-only**.\n\
- Per-KB join policy (default `invite`): `restrictive` (owner + added members \
only), `invite` (join → pending → owner approves), `permissive` (any \
authorized peer auto-joins as viewer).\n\
- `:kb-set-policy <kb> <restrictive|invite|permissive>`, `:kb-pending <kb>` (lists \
label + fingerprint), `:kb-approve <kb> <fingerprint> [role]`, \
`:kb-add-member <kb> <fingerprint> [role]`, `:kb-remove-member <kb> <fingerprint>`.\n\
- **Local self-protection (ADR-039 A2)**: `:kb-block-member <kb> <fingerprint>` / \
`:kb-unblock-member <kb> <fingerprint>` (or `b` in *KB Sharing*) add/remove a \
principal on **this daemon's LOCAL deny-list** — never propagated to peers (unlike \
`:kb-remove-member`, a global removal) and **not owner-gated** (you may block even \
the owner). Use it to stop trusting a principal you cannot get globally removed.\n\
- Members are managed by **fingerprint** (from `:kb-pending` or, for admins, \
`mae-daemon authorized`). Admin: `mae-daemon authorize <line> <unique-label>`, \
`mae-daemon revoke <label|SHA256:fp>`.\n\
**As an AI peer you act under the human's KB role and cannot exceed it** — the \
daemon enforces identically for keybindings and tool calls.\n\n\
** End-to-End Encryption (ADR-037)\n\
`:kb-set-encryption <kb> e2e` seals a KB's node content under a per-KB symmetric \
content key. The daemon/relay is **key-blind** — it stores only ciphertext and never \
holds the key. The owner wraps the key to each member's published X25519 key on \
approval; a member derives it from the signed membership op-log. Your Ed25519 \
identity seed (`~/.local/share/mae/collab/id_ed25519`) is the root of all access — \
**back it up**; without it (and absent a recovery key) encrypted content is \
unrecoverable. See [[concept:collab-architecture]] and `docs/E2E_ENCRYPTION.md`.\n\n\
** Identity rotation & recovery (ADR-040)\n\
- **Rotate** (planned key change, new device): `collab-rotate-identity` (`SPC C I r`) \
cross-signs a successor key into every KB you own AND belong to; the owner re-wraps \
E2e keys to your new key. Then authorize the new key on the daemon and reconnect.\n\
- **Prepare for loss**: `collab-register-recovery-key` (`SPC C I k`) registers an \
offline recovery key across your KBs and saves it to `<collab_dir>/recovery` — **move \
it OFFLINE**. Latest registration wins (revokes a leaked one).\n\
- **Recover a lost key**: with your new key authorized + connected, \
`:collab-recover-identity <recovery-key-dir> <old-fingerprint>` uses the offline key \
to rotate your seats onto the new key. Compromise (not just loss) → owner-mediated \
remove + re-key + local block (see COLLABORATION.md §8).\n\n\
** P2P mesh (ADR-025, beta)\n\
Two **daemons** can sync a KB directly over iroh QUIC — no central hub. `kb-share-p2p` \
establishes a mesh share on your daemon and mints a `mae://join/…` ticket; a peer runs \
`:kb-join-p2p <ticket>`, their daemon dials yours, you approve the peer daemon's \
fingerprint, and the KB converges peer-to-peer. Enable with `[collab.p2p]` + \
`mae setup-collab --p2p`; see `docs/DAEMON_ADMIN.md §3b`.\n\n\
** Step-by-step workflows (in-manual)\n\
Follow these lessons end-to-end (each ends with a Verify step):\n\
- [[lesson:kb-set-encryption|Enable E2E on a KB]] · [[lesson:kb-join-encrypted|Join an encrypted KB]]\n\
- [[lesson:kb-manage-members|Manage members & roles]]\n\
- [[lesson:collab-rotate-identity|Rotate identity]] · [[lesson:collab-register-recovery-key|Register a recovery key]] · [[lesson:collab-recover-identity|Recover a lost identity]]\n\
- [[lesson:kb-share-p2p|Share over P2P (beta)]]\n\
- [[concept:kb-e2e-security-boundaries|What E2E does NOT protect]] — read before relying on it.\n\n\
See also: [[concept:knowledge-base]], [[concept:collab-architecture]], \
[[concept:sync-engine]], [[concept:adr-kb-crdt]], \
[[concept:kb-e2e-security-boundaries]], [[index]]\n";
