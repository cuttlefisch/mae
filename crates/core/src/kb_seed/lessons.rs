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
12. [[lesson:observability|Observability]] — watchdog, event recording, introspect\n\n\
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
MAE is extensible via R7RS Scheme (Steel). [[concept:hooks|Hooks]] let \
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
Configure servers in `~/.config/mae/config.toml`:\n\
```toml\n\
[lsp.rust]\n\
command = \"rust-analyzer\"\n\n\
[lsp.python]\n\
command = \"pylsp\"\n\
```\n\n\
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
- **Tab** — next link    **Shift-Tab** — previous link\n\
- **Enter** — follow link\n\
- **C-o** — go back    **C-i** — go forward\n\
- **q** — close help\n\n\
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
