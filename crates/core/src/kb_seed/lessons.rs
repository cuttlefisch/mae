pub(super) const TUTOR_INDEX: &str = "\
Welcome to the MAE Tutorial!\n\n\
MAE (Modern AI Editor) is an AI-native Lisp machine editor. \
Work through these lessons to learn the essentials.\n\n\
## Lessons\n\
1. [[lesson:navigation|Navigation]] — h/j/k/l, words, pages\n\
2. [[lesson:modes|Modes]] — Normal, Insert, Visual, Command\n\
3. [[lesson:editing|Editing]] — insert, delete, undo, repeat\n\
4. [[lesson:files|Files & Buffers]] — open, save, switch\n\
5. [[lesson:ai|AI Features]] — the AI as peer actor\n\
6. [[lesson:scheme|Scheme REPL]] — extend MAE with R7RS Scheme\n\
7. [[lesson:lsp|LSP]] — go-to-definition, references, hover\n\
8. [[lesson:terminal|Terminal]] — embedded terminal emulator\n\
9. [[lesson:help|Help System]] — navigating the knowledge base\n\
10. [[lesson:leader|Leader Keys]] — SPC-based command groups\n\
11. [[lesson:debugging|Debugging]] — DAP, breakpoints, stepping, inspect\n\
12. [[lesson:observability|Observability]] — watchdog, event recording, introspect\n\
13. [[lesson:collab-setup|Collaborative Editing]] — share buffers in real-time\n\n\
## Sharing a Knowledge Base (E2E)\n\
- [[lesson:kb-set-encryption|Enabling E2E Encryption]] — owner-only, per-KB\n\
- [[lesson:kb-join-encrypted|Joining an Encrypted KB]] — get admitted + decrypt\n\
- [[lesson:kb-manage-members|Managing Members & Roles]] — add/remove/approve, policy\n\
- [[lesson:collab-rotate-identity|Rotating Your Identity Key]] — move to a fresh key\n\
- [[lesson:collab-register-recovery-key|Registering a Recovery Key]] — before you need it\n\
- [[lesson:collab-recover-identity|Recovering a Lost Identity]] — restore access\n\
- [[lesson:kb-share-p2p|Sharing over P2P]] — mesh, no hub (beta)\n\
- [[concept:kb-e2e-security-boundaries|What E2E does NOT protect]] — the honest limits\n\n\
Navigate with **Tab** to move between links, **Enter** to follow.\n\
**C-o** goes back, **C-i** goes forward.\n\n\
See also: [[index|Help Index]]\n";

pub(super) const LESSON_NAVIGATION: &str = "\
## Lesson 1: Navigation\n\n\
MAE uses vi-style movement keys in [[concept:mode|Normal mode]].\n\n\
### Basic movement\n\
  `h` — move left    `j` — move down    `k` — move up    `l` — move right\n\n\
### Word movement\n\
  `w` — next word start    `b` — previous word start\n\
  `e` — next word end      `0` — line start\n\
  `$` — line end\n\n\
### File movement\n\
  `gg` — first line         `G` — last line\n\
  `Ctrl-d` — half page down  `Ctrl-u` — half page up\n\
  `Ctrl-f` — page down       `Ctrl-b` — page up\n\n\
Try opening a file and moving around with these keys!\n\n\
**Next:** [[lesson:modes|Lesson 2: Modes]]  |  **Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_MODES: &str = "\
## Lesson 2: Modes\n\n\
MAE uses [[concept:mode|modal editing]] like Vim:\n\n\
- **Normal mode** (default) — navigation and commands\n\
- **Insert mode** — type text freely\n\
- **Visual mode** — select text\n\
- **Command mode** — ex commands (`:` prefix)\n\n\
### Switching modes\n\
  `i` — enter Insert mode (before cursor)\n\
  `a` — enter Insert mode (after cursor)\n\
  `v` — enter Visual mode (character)\n\
  `V` — enter Visual mode (line)\n\
  `:` — enter Command mode\n\
  `Escape` — return to Normal mode\n\n\
**Prev:** [[lesson:navigation|Lesson 1]]  |  \
**Next:** [[lesson:editing|Lesson 3: Editing]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_EDITING: &str = "\
## Lesson 3: Editing\n\n\
### Inserting text\n\
  `i` — insert before cursor    `a` — insert after cursor\n\
  `o` — open line below         `O` — open line above\n\n\
### Deleting text\n\
  `x` — delete character         [[cmd:delete-line|dd]] — delete line\n\
  `dw` — delete word             `d$` — delete to end of line\n\n\
### Undo / Redo\n\
  [[cmd:undo|u]] — undo          `Ctrl-r` — redo\n\n\
### Clipboard\n\
  `yy` — yank (copy) line       `p` — paste after\n\
  `P` — paste before\n\n\
### Repeat\n\
  `.` — repeat last edit\n\n\
**Prev:** [[lesson:modes|Lesson 2]]  |  \
**Next:** [[lesson:files|Lesson 4: Files]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_FILES: &str = "\
## Lesson 4: Files & Buffers\n\n\
A [[concept:buffer|buffer]] is the unit of editable content in MAE.\n\n\
### File commands\n\
  `:w` — [[cmd:save|save]] file\n\
  `:e <file>` — open file\n\
  `:q` — quit (fails if unsaved)\n\
  `:wq` or `:x` — save and quit\n\n\
### Leader shortcuts\n\
  `SPC f f` — find file (fuzzy picker)\n\
  `SPC f d` — file browser\n\
  `SPC f t` — file tree sidebar\n\
  `SPC b b` — switch buffer (palette)\n\
  `SPC b d` — close buffer\n\n\
**Prev:** [[lesson:editing|Lesson 3]]  |  \
**Next:** [[lesson:ai|Lesson 5: AI]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_AI: &str = "\
## Lesson 5: AI Features\n\n\
MAE treats the AI agent as a [[concept:ai-as-peer|peer actor]] — \
it calls the same primitives as you.\n\n\
### AI commands\n\
- `SPC a p` — **[[cmd:ai-prompt]]** (send a message / open conversation)\n\
- `SPC a a` — **[[cmd:open-ai-agent]]** (launch a dedicated AI agent in a shell)\n\
- `SPC a c` — **[[cmd:ai-cancel]]** (cancel an in-flight AI operation)\n\n\
### Conversation Memory\n\
Conversations are persistent per project. MAE automatically saves history to \
`.mae/conversation.json` in your project root. If you restart the editor, \
the previous chat will be restored automatically if `restore_session` is enabled.\n\n\
### Configuration\n\
Use `:set` or `(set-option! ...)` to configure the provider:\n\
- `:set ai_provider deepseek` (or `openai`, `claude`, `gemini`)\n\
- `:set ai_model deepseek-reasoner`\n\n\
### Tool Architecture\n\
The AI has access to 100+ tools split into two tiers:\n\
- **Core** (51 tools): always sent with every request (buffer ops, navigation, project, git basics).\n\
- **Extended** (on demand): requested via the `request_tools` meta-tool. 10 categories: \
`lsp`, `dap`, `knowledge`, `shell`, `commands`, `git`, `web`, `ai`, `visual`, `debug`.\n\n\
Key tools:\n\
- `request_tools` — load a category of extended tools into the conversation.\n\
- `editor_save_state` / `editor_restore_state` — deterministic session state capture.\n\
- `web_fetch` — fetch raw content from URLs.\n\
- `introspect` — inspect threads, performance stats, lock contention.\n\n\
### Diff Display\n\
When the AI proposes changes via `propose_changes`, a `*AI-Diff*` buffer shows \
a [[concept:diff-display|syntax-highlighted unified diff]]. Use `:ai-accept` to apply \
or `:ai-reject` to discard.\n\n\
### Self-Diagnosis\n\
The AI can introspect the editor's health. You can ask it to \"introspect\" \
to see thread states, performance stats, and lock contention.\n\n\
**Prev:** [[lesson:files|Lesson 4]]  |  \
**Next:** [[lesson:scheme|Lesson 6: Scheme]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_SCHEME: &str = "\
## Lesson 6: Scheme REPL\n\n\
MAE is extensible via R7RS Scheme (mae-scheme). [[concept:hooks|Hooks]] let \
Scheme code react to editor events.\n\n\
### Evaluate expressions\n\
  `SPC e e` — evaluate current line\n\
  `SPC e b` — evaluate entire buffer\n\
  `:eval <expr>` — evaluate a Scheme expression\n\n\
### Try it\n\
  `:eval (+ 1 2)` — should show `3`\n\
  `:eval (set-option! \"theme\" \"dracula\")` — change theme\n\n\
### Configuration\n\
Your `init.scm` is loaded at startup. Use `SPC f c` to edit it.\n\n\
**Prev:** [[lesson:ai|Lesson 5]]  |  \
**Next:** [[lesson:lsp|Lesson 7: LSP]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_LSP: &str = "\
## Lesson 7: LSP\n\n\
MAE has first-class LSP (Language Server Protocol) support.\n\
LSP starts automatically when you open a supported file type.\n\n\
### Navigation\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `gd` | [[cmd:lsp-goto-definition]] | Go to definition |\n\
| `gr` | [[cmd:lsp-find-references]] | Find all references |\n\
| `K` | [[cmd:lsp-hover]] | Show hover documentation |\n\n\
### Hover Popup\n\
When `K` shows a hover popup:\n\
- Press `K` again to scroll down\n\
- Any other key dismisses the popup\n\
- `:set nolsp_hover_popup` falls back to status bar display\n\n\
### Diagnostics\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `]d` | [[cmd:lsp-next-diagnostic]] | Jump to next diagnostic |\n\
| `[d` | [[cmd:lsp-prev-diagnostic]] | Jump to previous diagnostic |\n\
| `SPC c x` | [[cmd:lsp-show-diagnostics]] | List all diagnostics |\n\
| `SPC t d` | [[cmd:toggle-lsp-diagnostics-inline]] | Toggle inline underlines |\n\n\
Diagnostics appear as wavy underlines with end-of-line virtual text.\n\
Gutter markers show severity: `E` error, `W` warning, `I` info, `H` hint.\n\n\
### Completion (Insert Mode)\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| (auto) | [[cmd:lsp-complete]] | Triggered on typing |\n\
| `Tab` | [[cmd:lsp-accept-completion]] | Accept selected item |\n\
| `C-n` | [[cmd:lsp-complete-next]] | Next item |\n\
| `C-p` | [[cmd:lsp-complete-prev]] | Previous item |\n\n\
### Code Actions & Refactoring\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `SPC c a` | [[cmd:lsp-code-action]] | Show code actions at cursor |\n\
| `j`/`k` | next/prev | Navigate the action menu |\n\
| `Enter` | [[cmd:lsp-code-action-select]] | Apply selected action |\n\
| `Esc` | dismiss | Close action menu |\n\
| `SPC c R` | [[cmd:lsp-rename]] | Rename symbol |\n\
| `SPC c f` | [[cmd:lsp-format]] | Format buffer |\n\n\
### Status & Configuration\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `SPC c s` | [[cmd:lsp-status]] | Show LSP server status |\n\n\
LSP server paths are one of the few settings that live in the legacy \
`config.toml` bootstrap (`SPC f P`):\n\
```toml\n\
[lsp.rust]\n\
command = \"rust-analyzer\"\n\n\
[lsp.python]\n\
command = \"pyright-langserver\"\n\
```\n\
Default servers ship for Rust, Python (pyright), TypeScript/JS, Go, C/C++ \
(clangd), Ruby, YAML, JSON, TOML, and Bash — each starts automatically when \
its binary is on PATH. Override a server path per-launch with an env var \
(`MAE_LSP_RUST`, `MAE_LSP_PYTHON`, `MAE_LSP_CPP`, `MAE_LSP_RUBY`, …). \
General editor behavior, by contrast, is configured in `init.scm` via \
`(set-option!)` / `:set` / `:set-save` — not config.toml.\n\n\
**Prev:** [[lesson:scheme|Lesson 6]]  |  \
**Next:** [[lesson:terminal|Lesson 8: Terminal]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_TERMINAL: &str = "\
## Lesson 8: Terminal\n\n\
MAE embeds a full [[concept:terminal|terminal emulator]].\n\n\
### Commands\n\
  `SPC o t` — open terminal\n\
  `Ctrl-\\ Ctrl-n` — exit terminal to Normal mode\n\
  `SPC e s` — send current line to terminal\n\
  `SPC e S` — send selection to terminal\n\n\
### Features\n\
- Full VT100 support (vim, less, top, fzf all work)\n\
- MCP bridge: AI agents in the terminal can call back into MAE\n\
- Shell CWD tracking via `/proc`\n\n\
**Prev:** [[lesson:lsp|Lesson 7]]  |  \
**Next:** [[lesson:help|Lesson 9: Help]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_HELP: &str = "\
## Lesson 9: Help System\n\n\
MAE's help is a [[concept:knowledge-base|knowledge base]] — the same data \
the AI queries via `kb_*` tools.\n\n\
### Help commands\n\
  `SPC h h` — [[cmd:help|open help index]]\n\
  `SPC h k` — describe key\n\
  `SPC h c` — describe command\n\
  `SPC h o` — describe option\n\
  `:help <topic>` — open help for a topic\n\n\
### Navigation\n\
- **Tab** — fold/unfold heading (or next link if not on heading)\n\
- **S-Tab** — global visibility cycle (Overview → Show All)\n\
- **n** / **p** — next/previous link\n\
- **Enter** — follow link\n\
- **e** — edit source file (Obsidian-style toggle)\n\
- **C-o** — go back    **C-i** — go forward\n\
- **za** — fold toggle    **zM** — fold all    **zR** — unfold all\n\
- **q** — close help\n\
- `SPC n v` — return to rendered KB view from source editing\n\n\
**Prev:** [[lesson:terminal|Lesson 8]]  |  \
**Next:** [[lesson:leader|Lesson 10: Leader Keys]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_LEADER: &str = "\
## Lesson 10: Leader Keys\n\n\
`SPC` is the leader key (Doom Emacs style). Press `SPC` and wait to see \
available sub-keys in the which-key popup.\n\n\
### Key groups\n\
| Prefix | Group | Examples |\n\
|--------|-------|----------|\n\
| `SPC f` | +file | `SPC f f` find, `SPC f s` save |\n\
| `SPC b` | +buffer | `SPC b b` switch, `SPC b d` kill |\n\
| `SPC w` | +window | `SPC w v` vsplit, `SPC w s` hsplit |\n\
| `SPC a` | +ai | `SPC a p` prompt, `SPC a a` agent |\n\
| `SPC h` | +help | `SPC h h` index, `SPC h k` describe key |\n\
| `SPC t` | +toggle | `SPC t t` theme, `SPC t l` line nums |\n\
| `SPC l` | +lsp | `SPC l d` diagnostics |\n\
| `SPC d` | +debug | `SPC d b` breakpoint, `SPC d c` continue |\n\
| `SPC p` | +project | `SPC p f` find file, `SPC p s` search |\n\
| `SPC e` | +eval | `SPC e e` eval line, `SPC e b` eval buffer |\n\
| `SPC q` | +quit | `SPC q q` quit |\n\n\
See [[key:leader-keys|full leader key reference]] for the complete list.\n\n\
See also: [[concept:command|Commands]], [[index|Help Index]]\n\n\
**Prev:** [[lesson:help|Lesson 9]]  |  \
**Next:** [[lesson:debugging|Lesson 11: Debugging]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_DEBUGGING: &str = "\
## Lesson 11: Debugging\n\n\
MAE has a built-in [[concept:debugging|DAP client]] for debugging any language.\n\n\
### Starting a debug session\n\
  `:debug-start` or `SPC d s` — launch debuggee with adapter\n\
  `:debug-attach <adapter> <pid>` — [[concept:dap-attach|attach to running process]]\n\n\
### Breakpoints\n\
  `SPC d b` — toggle breakpoint on current line\n\
  Conditional breakpoints: `:debug-toggle-breakpoint condition=\"x > 5\"`\n\
  Log-point breakpoints: `:debug-toggle-breakpoint log=\"value is {x}\"`\n\n\
### Stepping\n\
  `SPC d c` — continue execution\n\
  `SPC d n` — step over (next line)\n\
  `SPC d i` — step into function\n\
  `SPC d o` — step out of function\n\n\
### Inspecting state\n\
  `SPC d p` — open [[cmd:debug-panel|debug panel]] (threads, stack, variables)\n\
  `SPC d v` — [[cmd:debug-self|self-debug view]] (Rust + Scheme state)\n\
  `:debug-eval <expr>` — evaluate expression in debug context\n\n\
### AI debug tools\n\
The AI agent can drive the debugger using the same tools:\n\
  `dap_start`, `dap_set_breakpoint`, `dap_remove_breakpoint`, `dap_continue`\n\
  `dap_step`, `dap_list_variables`, `dap_inspect_variable`, `dap_expand_variable`\n\
  `dap_select_frame`, `dap_select_thread`, `dap_output`, `dap_evaluate`, `dap_disconnect`\n\n\
### Try it\n\
1. Open a Python file: `:e hello.py`\n\
2. Set a breakpoint: `SPC d b`\n\
3. Start debugging: `:debug-start`\n\
4. Step through with `SPC d n`\n\
5. Inspect variables in the debug panel: `SPC d p`\n\n\
**Prev:** [[lesson:leader|Lesson 10]]  |  \
**Next:** [[lesson:observability|Lesson 12: Observability]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_OBSERVABILITY: &str = "\
## Lesson 12: Observability\n\n\
MAE has built-in tools for diagnosing issues and understanding editor behavior.\n\n\
### Watchdog\n\
The [[concept:watchdog|watchdog]] monitors the event loop for stalls. If the \
main thread stops responding for >2 seconds, it dumps thread backtraces to the log.\n\
  `MAE_LOG=mae=trace mae` — enable watchdog logging\n\
  The watchdog runs automatically; no user action needed.\n\n\
### Event recording\n\
[[concept:event-recording|Event recording]] captures every input event and \
command dispatch for replay and bug reporting.\n\
  `:record-start` — start recording\n\
  Type some keys, trigger the bug…\n\
  `:record-stop` — stop recording\n\
  `:record-save /tmp/events.json` — save to JSON file\n\n\
### Try it\n\
1. `:record-start`\n\
2. Type `iHello, world!` then Escape\n\
3. `:record-stop` — note the event count\n\
4. `:record-save /tmp/demo.json`\n\n\
### Introspect\n\
The [[concept:introspect|introspect]] AI tool provides a diagnostic snapshot of \
the editor's internal state: threads, performance counters, lock contention, \
buffers, shell processes, and AI session info.\n\
  Ask the AI: \"introspect\" to see the full report.\n\n\
### Messages buffer\n\
  `:messages` or `SPC b m` — view the *Messages* log\n\
  All status messages, warnings, and errors are captured here.\n\n\
### Debug mode\n\
  `SPC t D` — toggle debug mode (RSS/CPU/frame-time in status bar)\n\
  `SPC t F` — toggle FPS overlay\n\n\
**Prev:** [[lesson:debugging|Lesson 11]]  |  \
**Next:** [[lesson:kb-import-roam|Lesson 13]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const CONCEPT_WATCHDOG: &str = "\
The **watchdog** is a background thread that monitors the editor's main event loop \
for responsiveness.\n\n\
## How it works\n\
1. The main thread bumps a heartbeat counter on every event loop iteration.\n\
2. The watchdog thread checks the counter every 2 seconds.\n\
3. If the counter hasn't advanced, the watchdog declares a **stall** and:\n\
   - Logs a warning with the stall duration.\n\
   - On Linux, dumps `/proc/self/task/*/status` for all threads.\n\
   - Records the stall in an anomaly log for later inspection.\n\n\
## Configuration\n\
The watchdog is always active but only logs at `trace` level:\n\
  `MAE_LOG=mae=trace mae` — see watchdog heartbeats and stall reports.\n\n\
## Why this exists\n\
Emacs has no built-in stall detection — when it hangs, you get a spinning cursor \
and no diagnostic information. MAE's watchdog provides actionable data immediately.\n\n\
See also: [[concept:event-recording]], [[concept:introspect]], [[index]]\n";

pub(super) const CONCEPT_EVENT_RECORDING: &str = "\
**Event recording** captures every input event and command dispatch during a session, \
enabling reproducible bug reports and automated replay.\n\n\
## Commands\n\
- `:record-start` — begin capturing events.\n\
- `:record-stop` — stop capturing. Shows event count in status bar.\n\
- `:record-save <path>` — write captured events to a JSON file.\n\n\
## JSON format\n\
Each event entry contains:\n\
- `timestamp` — milliseconds since recording started.\n\
- `event_type` — key press, mouse event, command dispatch, etc.\n\
- `details` — serialized event data.\n\n\
## AI integration\n\
The `event_recording` AI tool can dump the current recording buffer \
for automated analysis. Ask the AI: \"show me the event recording.\"\n\n\
## Use cases\n\
- **Bug reports:** record → reproduce → save → attach JSON to issue.\n\
- **Macros:** replay a recorded sequence (planned).\n\
- **Testing:** validate that a sequence of inputs produces expected state.\n\n\
See also: [[concept:watchdog]], [[concept:introspect]], [[index]]\n";

pub(super) const CONCEPT_DAP_ATTACH: &str = "\
**DAP attach** lets MAE connect its debugger to an already-running process, \
rather than launching a new debuggee.\n\n\
## Usage\n\
`:debug-attach <adapter> <pid>`\n\n\
## Adapters\n\
| Adapter | Language | Notes |\n\
|---------|----------|-------|\n\
| `lldb` | C/C++/Rust | Requires `lldb-dap` (LLVM project) |\n\
| `debugpy` | Python | Requires `debugpy` pip package |\n\
| `codelldb` | Rust/C++ | CodeLLDB VS Code extension adapter |\n\n\
## Example\n\
```\n\
;; Attach to a Python process:\n\
:debug-attach debugpy 12345\n\
\n\
;; Attach to a Rust binary:\n\
:debug-attach codelldb 67890\n\
```\n\n\
## Cross-instance debugging\n\
You can debug one MAE instance from another — attach to the target's PID and \
set breakpoints in the Rust source. This is how MAE developers debug the editor itself.\n\n\
## AI tool\n\
The `dap_start` AI tool supports an `attach` mode with `pid` parameter.\n\n\
See also: [[concept:debugging]], [[cmd:debug-start]], [[index]]\n";

pub(super) const CONCEPT_INTROSPECT: &str = "\
The **introspect** AI tool produces a diagnostic snapshot of the editor's internal \
state. It is the AI's equivalent of a doctor's checkup.\n\n\
## Sections\n\
| Section | Contents |\n\
|---------|----------|\n\
| **threads** | Thread count, names, watchdog status |\n\
| **performance** | Event loop latency, frame times, memory (RSS) |\n\
| **locks** | FairMutex contention stats, wait times, holder info |\n\
| **buffers** | Buffer count, sizes, kinds, modification state |\n\
| **shell** | Shell process count, PIDs, CWDs, exit status |\n\
| **ai** | Session state, message count, token usage, model |\n\
| **frame** | Per-frame render profiling: phase timing, cache hit/miss, visible buffers |\n\n\
## Usage\n\
Ask the AI: \"introspect\" or \"show me editor diagnostics.\"\n\
The AI calls the `introspect` tool and receives a structured JSON report.\n\n\
## When to use\n\
- Editor feels slow → check performance and lock contention sections.\n\
- Shell not responding → check shell section for process status.\n\
- AI behaving oddly → check AI section for session state.\n\n\
See also: [[concept:watchdog]], [[concept:event-recording]], [[concept:ai-as-peer]], [[index]]\n";

pub(super) const CONCEPT_RENDER_PROFILING: &str = "\
**Render profiling** lets you diagnose GUI frame time issues using built-in \
MCP tools — no external profiler needed.\n\n\
## Quick Diagnosis Workflow\n\
1. **Snapshot frame state:** `introspect(section: \"frame\")` → identify hot phase (syntax/layout/draw)\n\
2. **Scroll stress test:** `perf_benchmark(benchmark: \"scroll_stress\")` → find position-dependent spikes\n\
3. **Cache analysis:** Check `caches` section in the frame snapshot → which cache is thrashing?\n\n\
## Frame Snapshot Fields\n\
| Field | Meaning |\n\
|-------|---------|\n\
| `render_phase_us.syntax` | Time computing syntax highlight spans |\n\
| `render_phase_us.layout` | Time computing line positions + wrap |\n\
| `render_phase_us.draw` | Time drawing text + backgrounds |\n\
| `caches.syntax` | Tree-sitter parse cache hit/miss |\n\
| `caches.markup` | Org/Markdown markup span cache |\n\
| `caches.visual_rows` | Word-wrap row count cache |\n\n\
## Deep Debugging with DAP\n\
- Attach to MAE: `dap_start(adapter: \"lldb\", mode: \"attach\", pid: <PID>)`\n\
- Set conditional breakpoint: `dap_set_breakpoint(source: \"...\", line: N, condition: \"scale > 1.0\")`\n\
- Step through the hot path, inspect variables\n\n\
See also: [[concept:introspect]], [[concept:debugging]], [[concept:event-recording]], [[index]]\n";

pub(super) const LESSON_KB_IMPORT: &str = "\
## Lesson 13: Importing Your Knowledge Base\n\n\
If you have an org-roam directory (or any directory of `.org` files with `:ID:` \
properties), you can register it as a federated KB instance in MAE.\n\n\
### Prerequisites\n\
- Your org files need `:PROPERTIES: :ID: <uuid> :END:` drawers.\n\
- Nested subdirectories are supported (recursive import).\n\
- Files without `:ID:` are skipped (counted in the report).\n\n\
### Step 1: Register\n\
```\n\
:kb-register MyNotes ~/RoamNotes\n\
```\n\
Or ask the AI: \"import my KB at ~/RoamNotes\"\n\n\
MAE will recursively walk the directory, parse all `.org` files, \
and report results:\n\
```\n\
Registered 'MyNotes': 2,342 nodes, 4,891 links\n\
  Health: 45 orphans, 12 broken links, 3 duplicate IDs\n\
```\n\n\
### Step 2: Verify\n\
- `:kb-instances` — shows your registered instance with node count.\n\
- `:kb-health` — detailed health report (orphans, broken links, namespaces).\n\
- `SPC n f` or `SPC h s` — search should find your notes.\n\n\
### Step 3: Use\n\
Your notes are now searchable from `:help`, `SPC h`, and the AI agent. \
The AI can find and summarize your notes just like built-in help.\n\n\
### Step 4: Update after edits\n\
When you edit org files externally, refresh the graph:\n\
```\n\
:kb-reimport MyNotes\n\
```\n\n\
### Data safety\n\
- MAE **never writes** to your org directory (except `eor-instance.org` sentinel).\n\
- Your org files are the source of truth. The index is always re-derivable.\n\
- Back up your org files with git. The CozoDB index is disposable.\n\n\
### Unregister\n\
```\n\
:kb-unregister MyNotes\n\
```\n\
Removes from registry and frees memory. Your org files are untouched.\n\n\
**Prev:** [[lesson:observability|Lesson 12]]  |  \
**Index:** [[tutor:index|Tutorial]]\n\n\
See also: [[concept:kb-federation]], [[concept:kb-workflows]], [[concept:kb-vs-alternatives]]\n";

pub(super) const LESSON_COLLAB_SETUP: &str = "\
## Setting Up Collaborative Editing\n\n\
This lesson walks you through enabling real-time collaborative editing in MAE, \
from installing the daemon to sharing your first buffer with a peer.\n\n\
### Step 1 — Install the daemon\n\n\
Build and install `mae-daemon` from source:\n\
```bash\n\
cargo install --path daemon\n\
# or use the Makefile shortcut:\n\
make install-daemon\n\
```\n\n\
Verify it is on your PATH:\n\
```bash\n\
mae-daemon --version\n\
```\n\n\
### Step 2 — Start the server\n\n\
For local (loopback) use:\n\
```bash\n\
mae-daemon\n\
# Listening on 127.0.0.1:9473\n\
```\n\n\
For multi-machine use, bind to all interfaces:\n\
```bash\n\
mae-daemon --bind 0.0.0.0:9473\n\
```\n\n\
Or press `SPC C s` inside MAE to start a local server automatically.\n\n\
### Step 3 — Authenticate (trusted-peer `key` mode)\n\n\
MAE has three auth modes — `none`, `psk`, and `key`. The recommended mode is \
**`key`**: each editor holds an Ed25519 identity and the daemon trusts only \
explicitly authorized peers over mTLS — nothing secret is shared, so nothing \
can leak from a config file. One command on each **client** sets it up:\n\
```bash\n\
mae setup-collab --server 127.0.0.1:9473\n\
```\n\
This mints the editor's identity and writes `collab-auth-mode` + \
`collab-server-address` into `init.scm`. Add `--ssh-key ~/.ssh/id_ed25519` to \
reuse an existing key. Then print the client's public identity and authorize \
it on the **host**:\n\
```bash\n\
mae --collab-identity                 # on the client: prints the pubkey line\n\
mae-daemon authorize <pubkey-line>    # on the host: trust that client\n\
```\n\n\
The legacy symmetric `psk` mode (HMAC-SHA256, one shared secret for server + \
all clients) still works — set `collab-auth-mode` to `psk` — but supply the \
key out of band, never as plaintext in config:\n\
```scheme\n\
(set-option! \"collab-psk-command\" \"pass show mae/collab-psk\")\n\
```\n\n\
### Step 4 — Configure MAE to use the server\n\n\
In your Scheme REPL (`:eval`) or `init.scm`:\n\
```scheme\n\
(set-option! \"collab-server-address\" \"127.0.0.1:9473\")\n\
```\n\n\
For remote servers, replace `127.0.0.1:9473` with `host:port`.\n\n\
### Step 5 — Connect\n\n\
Either enable auto-connect so MAE connects on every startup:\n\
```scheme\n\
(set-option! \"collab-auto-connect\" \"true\")\n\
```\n\n\
Or connect manually: `SPC C c` (`:collab-connect`).\n\n\
### Step 6 — Share a buffer\n\n\
Open a file you want to collaborate on, then press `SPC C S` \
(`:collab-share`). The buffer is now visible to all connected peers.\n\n\
### Step 6b — Discover and join shared documents\n\n\
- `SPC C l` (`:collab-list`) — list all documents shared on the server.\n\
- `SPC C j` (`:collab-join`) — open a picker to select and join a shared document.\n\
- `:collab-join <name>` — join a specific document by name.\n\n\
**Joined buffers have no local file path by default.** The buffer is \
live-synced via CRDT, but you choose where (or whether) to save locally:\n\
- `:saveas <path>` — save the joined buffer to a local file.\n\
- `:w` on a pathless joined buffer shows guidance to use `:saveas`.\n\
- Enable `collab_auto_resolve_paths` to get prompted when the file \
  matches a path in your local project.\n\n\
### Step 7 — Verify the connection\n\n\
- `SPC C i` (`:collab-status`) — shows server address, connected peers, \
  and shared document list.\n\
- `mae doctor` (from the terminal) — checks server process health, \
  port availability, and WAL integrity.\n\n\
### Step 8 — AI tools for collaboration\n\n\
The AI agent has direct access to collaboration state via four tools:\n\n\
| Tool | Description |\n\
|------|-------------|\n\
| `collab_status` | Report connection state and peer list |\n\
| `collab_connect` | Connect to (or reconnect to) the configured server |\n\
| `collab_share` | Share a named buffer with connected peers |\n\
| `collab_doctor` | Run diagnostics: reachability, WAL, peer count |\n\n\
Ask the AI: \"connect to the collab server and share this buffer\" to \
have it set everything up for you.\n\n\
### Systemd User Service\n\n\
Install and enable the daemon as a systemd user service:\n\
```bash\n\
make install-service\n\
systemctl --user enable --now mae-daemon\n\
journalctl --user -u mae-daemon -f  # view logs\n\
```\n\n\
### Client-Frame Workflow\n\n\
Use `mae --connect` to open a frame that auto-connects to the server \
(like `emacsclient -c`):\n\
```bash\n\
mae --connect              # connects to 127.0.0.1:9473\n\
mae --connect 10.0.0.5:9473  # connects to a remote server\n\
```\n\n\
Add a sway/i3 keybind for instant connected frames:\n\
```\n\
bindsym $mod+Shift+e exec mae --connect\n\
```\n\n\
### Network & Firewall\n\n\
For multi-machine collaboration, bind to all interfaces:\n\
```bash\n\
mae-daemon --bind 0.0.0.0:9473\n\
```\n\n\
Open the firewall port:\n\
- Fedora: `sudo firewall-cmd --add-port=9473/tcp --permanent && sudo firewall-cmd --reload`\n\
- Ubuntu: `sudo ufw allow 9473/tcp`\n\n\
**Security:** prefer trusted-peer `key` mode (Ed25519 identities over mTLS) — \
authorize each client with `mae-daemon authorize`. The legacy `psk` mode \
(HMAC-SHA256, shared secret) is also supported; keep the secret out of \
plaintext config (`collab-psk-command`). For untrusted networks, use a VPN \
(Tailscale/WireGuard).\n\n\
### Troubleshooting\n\n\
- **Connection refused** — check `mae-daemon` is running: `ss -tlnp | grep 9473`\n\
- **Auth failed** — in `key` mode the client isn't authorized: run `mae-daemon authorize <pubkey-line>` on the host; in `psk` mode the keys differ\n\
- **No peers visible** — ensure all clients use the same `collab-server-address`\n\
- **Stale state after restart** — run `:collab-doctor` to inspect WAL health; \
  the server recovers from WAL automatically on restart\n\
- **Permission denied on port** — use a port above 1024 (default 9473 is fine)\n\
- **Firewall blocking** — run `:collab-doctor` for connectivity diagnostics\n\n\
**Index:** [[tutor:index|Tutorial]]\n\n\
See also: [[concept:collab-architecture]], [[concept:collab-workflows]], \
[[concept:sync-engine]], [[index]]\n";

// --- E2E KB-sharing workflow lessons (Workstream F, #250) ---
// Each is a followable step-by-step workflow ending in a Verify step, naming
// ONLY registered commands/options (enforced by the verifiable-docs guard in
// mod.rs, `kb_sharing_lessons_name_only_real_commands`).

pub(super) const LESSON_KB_SET_ENCRYPTION: &str = "\
## Enabling E2E Encryption on a KB\n\n\
End-to-end encryption is [[concept:kb-e2e-security-boundaries|owner-only]], \
per-KB, and one-way — once on, it stays on. A per-KB content key is sealed to \
each member's X25519 wrap key; the daemon, hub, and any relay see only \
ciphertext (XChaCha20-Poly1305).\n\n\
### Steps\n\
1. Share the KB (creates it as a collaborative collection):\n\
   `:kb-share my-kb`\n\
2. Set the join policy to **invite** — `permissive` admits keyless members and \
is incompatible with E2E:\n\
   `:kb-set-policy my-kb invite`\n\
3. Turn on encryption. This records a signed SetEncryption op and re-seals the \
content key to every current member:\n\
   `:kb-set-encryption my-kb e2e`\n\n\
Members are named by key **fingerprint** (e.g. `SHA256:abc123…`), never by \
username. Adding a member re-seals the key to them; removing one rotates the \
key (new epoch) so future ops are sealed anew.\n\n\
> Note: enable E2E on the **hub**, not the mesh. See \
[[lesson:kb-share-p2p|Sharing over P2P]] for the current mesh limitation.\n\n\
### Verify\n\
Run `:kb-sharing-status` and confirm `my-kb` shows encryption **e2e** and \
policy **invite**. The status snapshot lists each member fingerprint with a \
sealed content key.\n\n\
**See also:** [[concept:kb-e2e-security-boundaries|What E2E does NOT protect]]\n\n\
**Prev:** [[tutor:index|Tutorial]]  |  \
**Next:** [[lesson:kb-join-encrypted|Joining an Encrypted KB]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_KB_JOIN_ENCRYPTED: &str = "\
## Joining an Encrypted KB\n\n\
To read an [[lesson:kb-set-encryption|encrypted KB]] you must be admitted by \
the owner. Your published X25519 wrap key lets the owner seal the content key \
to you — without it you can join the roster but cannot decrypt.\n\n\
### Steps\n\
1. Make sure you are connected to the daemon hosting the KB:\n\
   `:collab-connect`\n\
2. Give the owner your identity fingerprint (`SHA256:…`) out-of-band. Find it \
in the sharing status of any KB you own, or ask the owner to read it from a \
pending request.\n\
3. Request to join:\n\
   `:kb-join my-kb`\n\
4. Wait for the owner to approve you as a role (`:kb-approve`). On approval the \
owner re-seals and you receive the content key automatically.\n\n\
If decryption is not yet available after joining, confirm the owner approved \
you *after* E2E was enabled — a stale approval leaves you keyless.\n\n\
### Verify\n\
Run `:kb-sharing-status` and confirm `my-kb` lists your fingerprint with a \
role (owner/editor/viewer) and that node contents render as plaintext, not \
ciphertext. `:collab-status` should show the KB as synced.\n\n\
**See also:** [[concept:kb-e2e-security-boundaries|What E2E does NOT protect]]\n\n\
**Prev:** [[lesson:kb-set-encryption|Enabling E2E Encryption]]  |  \
**Next:** [[lesson:kb-manage-members|Managing Members & Roles]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_KB_MANAGE_MEMBERS: &str = "\
## Managing Members & Roles\n\n\
Roles nest: **owner ⊇ editor ⊇ viewer**. Only the owner can change membership. \
Every member is named by key **fingerprint** (`SHA256:…`), not username.\n\n\
### Add a member\n\
1. Get the peer's fingerprint out-of-band.\n\
2. Add them:\n\
   `:kb-add-member my-kb SHA256:abc123… editor`\n\
   On an encrypted KB this re-seals the content key to their wrap key.\n\n\
### Approve a pending request\n\
1. List who is waiting:\n\
   `:kb-pending my-kb`\n\
2. Approve as a role:\n\
   `:kb-approve my-kb SHA256:def456… viewer`\n\n\
### Remove a member\n\
`:kb-remove-member my-kb SHA256:abc123…`\n\
Removal **rotates the content key** (a new epoch): future ops are sealed to the \
remaining members only. It does NOT retroactively protect history already \
sealed under the old key.\n\n\
### Set the join policy\n\
`:kb-set-policy my-kb invite`  — one of `restrictive` | `invite` | \
`permissive`. Use **invite** with E2E; `permissive` admits keyless members and \
is incompatible with encryption.\n\n\
### Verify\n\
Run `:kb-sharing-status` and confirm the member list, each fingerprint's role, \
the policy, and (after a removal) an incremented epoch.\n\n\
**See also:** [[concept:kb-e2e-security-boundaries|What E2E does NOT protect]]\n\n\
**Prev:** [[lesson:kb-join-encrypted|Joining an Encrypted KB]]  |  \
**Next:** [[lesson:collab-rotate-identity|Rotating Your Identity Key]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_COLLAB_ROTATE_IDENTITY: &str = "\
## Rotating Your Identity Key\n\n\
Your identity is a single 32-byte Ed25519 seed at \
`~/.local/share/mae/collab/id_ed25519` (mode 0600). Rotation cross-signs a \
successor: the old key endorses the new one, so peers can verify continuity \
across the change.\n\n\
### Steps\n\
1. While your current identity is healthy, rotate:\n\
   `:collab-rotate-identity`\n\
   This mints a new seed, cross-signs it with the old key, and records the \
rebind.\n\
2. Your **node-id changes**. Authorize the new key on the daemon \
**out-of-band** (add its fingerprint to the daemon's trusted peers).\n\
3. Reconnect:\n\
   `:collab-disconnect` then `:collab-connect`\n\
4. Make your first edit. Rotation trips the **epoch fence once** — the first \
post-rotation op is fenced, then normal sync resumes.\n\n\
> Register a [[lesson:collab-register-recovery-key|recovery key]] BEFORE you \
need it — there is no server copy and no password reset.\n\n\
### Verify\n\
Run `:collab-doctor` and confirm the new fingerprint is authorized and \
connected. `:collab-status` should show your peer as active under the new \
node-id.\n\n\
**Prev:** [[lesson:kb-manage-members|Managing Members & Roles]]  |  \
**Next:** [[lesson:collab-register-recovery-key|Registering a Recovery Key]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_COLLAB_REGISTER_RECOVERY_KEY: &str = "\
## Registering a Recovery Key\n\n\
There is no password reset and no server-side copy of your identity. If you \
lose `~/.local/share/mae/collab/id_ed25519` without a backup or recovery key, \
it is **unrecoverable**. Register a recovery key WHILE HEALTHY.\n\n\
### Steps\n\
1. Back up your primary seed to a safe, offline location first.\n\
2. Register an offline recovery key:\n\
   `:collab-register-recovery-key`\n\
   This writes `~/.local/share/mae/collab/recovery/id_ed25519` and cross-signs \
it with your primary so it can later author a recovery rebind.\n\
3. **Move the recovery key OFFLINE** — copy it to removable media or a secrets \
vault, then delete the on-disk copy. At-rest keys are protected by file perms \
only, so an online recovery key is as exposed as the primary.\n\n\
Note your current identity **fingerprint** (`SHA256:…`) and store it alongside \
the recovery key — you will need it to recover.\n\n\
### Verify\n\
Run `:collab-doctor` and confirm it reports a registered recovery key. Then \
confirm `~/.local/share/mae/collab/recovery/id_ed25519` exists before you move \
it offline.\n\n\
**Prev:** [[lesson:collab-rotate-identity|Rotating Your Identity Key]]  |  \
**Next:** [[lesson:collab-recover-identity|Recovering a Lost Identity]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_COLLAB_RECOVER_IDENTITY: &str = "\
## Recovering a Lost Identity\n\n\
If your primary seed is lost, a [[lesson:collab-register-recovery-key|recovery \
key registered while healthy]] can author a recovery-signed rebind onto a fresh \
primary — no server, no password reset.\n\n\
### Steps\n\
1. Bring your offline recovery key back online at \
`~/.local/share/mae/collab/recovery/id_ed25519` (or note the directory it \
lives in).\n\
2. Recover, passing the recovery directory and your **old fingerprint**:\n\
   `:collab-recover-identity ~/.local/share/mae/collab/recovery SHA256:abc123…`\n\
   This generates a fresh primary seed and records a recovery-signed rebind \
that peers verify against the old key.\n\
3. Your node-id is new — authorize it on the daemon **out-of-band**, then \
reconnect:\n\
   `:collab-disconnect` then `:collab-connect`\n\
4. Your first edit trips the **epoch fence once**, then sync resumes.\n\n\
> After recovering, move the recovery key back offline and consider \
registering a new one.\n\n\
### Verify\n\
Run `:collab-doctor` and confirm the rebind chains from your old fingerprint to \
the new key and that you are connected. `:collab-status` should show your peer \
active again.\n\n\
**Prev:** [[lesson:collab-register-recovery-key|Registering a Recovery Key]]  |  \
**Next:** [[lesson:kb-share-p2p|Sharing a KB over P2P]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const LESSON_KB_SHARE_P2P: &str = "\
## Sharing a KB over P2P (beta)\n\n\
P2P mesh sharing lets peers sync a KB directly with **no central hub**. This \
ships **beta**.\n\n\
### Steps (owner)\n\
1. Share the KB normally first:\n\
   `:kb-share my-kb`\n\
2. Publish a mesh ticket:\n\
   `:kb-share-p2p my-kb`\n\
   This prints a `mae://join/…` ticket. Send it to your peer out-of-band.\n\n\
### Steps (peer)\n\
1. Join with the ticket:\n\
   `:kb-join-p2p mae://join/…`\n\
2. The owner authorizes your fingerprint, and the KB converges over the mesh.\n\n\
### Encryption caveat\n\
E2E content **seals over the mesh** and relaying daemons are **key-blind** \
(ciphertext only). BUT a member who joins *over the mesh* **cannot decrypt \
yet** — this is a tracked gap. For encrypted KBs, enable E2E on the **hub** and \
admit members there (see [[lesson:kb-set-encryption|Enabling E2E]]).\n\n\
### Verify\n\
On both peers run `:kb-sharing-status` and confirm `my-kb` lists the same \
members. `:kb-list-remote` should show the KB, and edits on one peer should \
appear on the other via `:collab-status`.\n\n\
**See also:** [[concept:kb-e2e-security-boundaries|What E2E does NOT protect]]\n\n\
**Prev:** [[lesson:collab-recover-identity|Recovering a Lost Identity]]  |  \
**Next:** [[tutor:index|Tutorial]]  |  \
**Index:** [[tutor:index|Tutorial]]\n";

pub(super) const CONCEPT_KB_E2E_SECURITY_BOUNDARIES: &str = "\
## What E2E Encryption Does NOT Protect\n\n\
[[lesson:kb-set-encryption|E2E encryption]] hides node **content** from the \
daemon, hub, and relays. It does not hide everything. Know the boundaries \
before you rely on it.\n\n\
### Metadata is visible\n\
Membership, roles, authorship, op sizes, timing, and the link-graph shape are \
**not** encrypted. An observer of the daemon can see who is in a KB, who wrote \
when, and how big each change was — just not the plaintext.\n\n\
### Insiders can leak\n\
E2E defends against outsiders and key-blind relays, not members. **Any current \
member** can read and re-publish the plaintext. Trust is bounded by your \
[[lesson:kb-manage-members|member list]].\n\n\
### No forward secrecy / post-compromise security\n\
There is no ratchet. A leaked content key exposes **all history sealed under \
it**. Removing a member re-keys **forward only** — it does not protect ops \
already sealed under the old epoch.\n\n\
### Keys are plaintext at rest\n\
Your identity seed (`~/.local/share/mae/collab/id_ed25519`) and any online \
recovery key are protected by **file permissions only** (0600), not a \
passphrase. Anyone who reads the file is you.\n\n\
### Key loss is fatal\n\
No server copy, no password reset. Losing your seed without a backup or a \
registered [[lesson:collab-register-recovery-key|recovery key]] is \
**unrecoverable**.\n\n\
**See also:** [[lesson:kb-set-encryption|Enabling E2E Encryption]]  |  \
[[lesson:kb-manage-members|Managing Members & Roles]]\n\n\
**Index:** [[tutor:index|Tutorial]]\n";
