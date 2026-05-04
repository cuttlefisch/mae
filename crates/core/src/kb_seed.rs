//! Seed the knowledge base with built-in help content.
//!
//! The KB is MAE's answer to Emacs's built-in `*Help*` and its Info
//! manuals. Two sources feed it:
//!
//! 1. **Generated:** every entry in `CommandRegistry` becomes a
//!    `cmd:<name>` node so `:describe-command` (and the AI's `kb_get`
//!    tool) can return consistent docs without a separate table.
//! 2. **Hand-authored:** concept and key nodes embedded at compile time.
//!    These are the architectural stories (buffer, window, mode,
//!    AI-as-peer, …) that motivate the code.
//!
//! The hand-authored nodes live in `themes/…`-style static strings here
//! rather than on disk. Phase 5 will add a persistent store; until then,
//! regenerating the KB on every startup keeps help docs and commands in
//! lockstep with the code that ships.

use std::collections::HashMap;

use mae_kb::{KnowledgeBase, Node, NodeKind};

use crate::commands::CommandRegistry;
use crate::hooks::HookRegistry;
use crate::keymap::{serialize_macro, Keymap};
use crate::options::OptionRegistry;

/// Build the initial KB: hand-authored concept/index nodes + generated
/// `cmd:*` nodes derived from the command registry, enriched with
/// keybinding and hook information.
pub fn seed_kb(
    registry: &CommandRegistry,
    keymaps: &HashMap<String, Keymap>,
    hooks: &HookRegistry,
) -> KnowledgeBase {
    let mut kb = KnowledgeBase::new();
    install_static_nodes(&mut kb);
    install_tutor_nodes(&mut kb);
    install_tutorial_nodes(&mut kb);
    install_scheme_nodes(&mut kb);
    let keybinding_map = collect_keybindings(keymaps);
    install_command_nodes(&mut kb, registry, &keybinding_map, hooks);
    install_category_nodes(&mut kb, registry, &keybinding_map);
    install_option_nodes(&mut kb);
    install_user_help_nodes(&mut kb);
    kb
}

/// Convenience for tests: seed with empty keymaps and hooks.
pub fn seed_kb_default(registry: &CommandRegistry) -> KnowledgeBase {
    seed_kb(registry, &HashMap::new(), &HookRegistry::new())
}

/// Build a reverse index: command_name → [(mode_name, key_display_string)].
pub fn collect_keybindings(
    keymaps: &HashMap<String, Keymap>,
) -> HashMap<String, Vec<(String, String)>> {
    let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (mode_name, keymap) in keymaps {
        for (keys, command) in keymap.bindings() {
            let display = serialize_macro(keys);
            map.entry(command.clone())
                .or_default()
                .push((mode_name.clone(), display));
        }
    }
    // Sort each command's bindings by mode name for consistency
    for bindings in map.values_mut() {
        bindings.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    }
    map
}

/// Single-command variant: return bindings for one command.
pub fn collect_keybindings_for(
    keymaps: &HashMap<String, Keymap>,
    cmd_name: &str,
) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for (mode_name, keymap) in keymaps {
        for (keys, command) in keymap.bindings() {
            if command == cmd_name {
                let display = serialize_macro(keys);
                result.push((mode_name.clone(), display));
            }
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    result
}

/// Infer a category from a command name based on prefix conventions.
pub fn infer_category(name: &str) -> &'static str {
    if name.starts_with("move-")
        || name.starts_with("scroll-")
        || name.starts_with("goto-")
        || name.starts_with("jump-")
        || name == "center-cursor-vertically"
    {
        "movement"
    } else if name.starts_with("delete-")
        || name.starts_with("change-")
        || name.starts_with("insert-")
        || name.starts_with("yank")
        || name.starts_with("paste")
        || name.starts_with("indent")
        || name == "undo"
        || name == "redo"
        || name == "join-lines"
        || name == "open-line-below"
        || name == "open-line-above"
        || name == "replace-char"
        || name == "dot-repeat"
    {
        "editing"
    } else if name.starts_with("git-") {
        "git"
    } else if name.starts_with("lsp-") {
        "lsp"
    } else if name.starts_with("debug-") || name.starts_with("dap-") {
        "debug"
    } else if name.starts_with("window-")
        || name.starts_with("split-")
        || name.starts_with("focus-")
    {
        "window"
    } else if name.starts_with("file-tree-") {
        "file-tree"
    } else if name.starts_with("visual-")
        || name.starts_with("enter-visual")
        || name.starts_with("block-visual")
    {
        "visual"
    } else if name.starts_with("ai-") || name.starts_with("open-ai") {
        "ai"
    } else if name.starts_with("help")
        || name.starts_with("describe-")
        || name.starts_with("kb-")
        || name == "tutor"
    {
        "help"
    } else if name.starts_with("org-")
        || name.starts_with("md-")
        || name.starts_with("insert-heading")
    {
        "org"
    } else if name.starts_with("toggle-") {
        "toggle"
    } else if name.starts_with("enter-") {
        "mode"
    } else if name.starts_with("search") || name.starts_with("find-") || name == "nohlsearch" {
        "search"
    } else if name.starts_with("save")
        || name.starts_with("open-file")
        || name.starts_with("close-buffer")
        || name.starts_with("kill-buffer")
        || name == "quit"
        || name == "force-quit"
        || name.starts_with("next-buffer")
        || name.starts_with("prev-buffer")
        || name == "new-buffer"
    {
        "file"
    } else if name.starts_with("shell")
        || name.starts_with("terminal")
        || name.starts_with("send-to-shell")
        || name.starts_with("send-region")
    {
        "shell"
    } else if name.starts_with("macro-")
        || name.starts_with("record-")
        || name.starts_with("play-macro")
    {
        "macro"
    } else if name.starts_with("register-") {
        "register"
    } else {
        "general"
    }
}

/// Install the hand-authored index + concept + key nodes.
fn install_static_nodes(kb: &mut KnowledgeBase) {
    for node in static_nodes() {
        kb.insert(node);
    }
}

/// Install tutorial lesson nodes into the KB.
fn install_tutor_nodes(kb: &mut KnowledgeBase) {
    for node in tutor_nodes() {
        kb.insert(node);
    }
}

/// Tutorial nodes: an index + 10 lessons.
fn tutor_nodes() -> Vec<Node> {
    vec![
        Node::new(
            "tutor:index",
            "MAE Tutorial",
            NodeKind::Concept,
            TUTOR_INDEX,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:navigation",
            "Lesson 1: Navigation",
            NodeKind::Concept,
            LESSON_NAVIGATION,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:modes",
            "Lesson 2: Modes",
            NodeKind::Concept,
            LESSON_MODES,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:editing",
            "Lesson 3: Editing",
            NodeKind::Concept,
            LESSON_EDITING,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:files",
            "Lesson 4: Files & Buffers",
            NodeKind::Concept,
            LESSON_FILES,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:ai",
            "Lesson 5: AI Features",
            NodeKind::Concept,
            LESSON_AI,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:scheme",
            "Lesson 6: Scheme REPL",
            NodeKind::Concept,
            LESSON_SCHEME,
        )
        .with_tags(["tutorial"]),
        Node::new("lesson:lsp", "Lesson 7: LSP", NodeKind::Concept, LESSON_LSP)
            .with_tags(["tutorial"]),
        Node::new(
            "lesson:terminal",
            "Lesson 8: Terminal",
            NodeKind::Concept,
            LESSON_TERMINAL,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:help",
            "Lesson 9: Help System",
            NodeKind::Concept,
            LESSON_HELP,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:leader",
            "Lesson 10: Leader Keys",
            NodeKind::Concept,
            LESSON_LEADER,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:debugging",
            "Lesson 11: Debugging",
            NodeKind::Concept,
            LESSON_DEBUGGING,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:observability",
            "Lesson 12: Observability",
            NodeKind::Concept,
            LESSON_OBSERVABILITY,
        )
        .with_tags(["tutorial"]),
    ]
}

const TUTOR_INDEX: &str = "\
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

const LESSON_NAVIGATION: &str = "\
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

const LESSON_MODES: &str = "\
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

const LESSON_EDITING: &str = "\
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

const LESSON_FILES: &str = "\
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

const LESSON_AI: &str = "\
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
- **Core** (~43 tools): always sent with every request (buffer ops, navigation, project, git basics).\n\
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

const LESSON_SCHEME: &str = "\
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

const LESSON_LSP: &str = "\
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

const LESSON_TERMINAL: &str = "\
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

const LESSON_HELP: &str = "\
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

const LESSON_LEADER: &str = "\
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

const LESSON_DEBUGGING: &str = "\
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

const LESSON_OBSERVABILITY: &str = "\
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

const CONCEPT_WATCHDOG: &str = "\
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

const CONCEPT_EVENT_RECORDING: &str = "\
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

const CONCEPT_DAP_ATTACH: &str = "\
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

const CONCEPT_INTROSPECT: &str = "\
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
| **ai** | Session state, message count, token usage, model |\n\n\
## Usage\n\
Ask the AI: \"introspect\" or \"show me editor diagnostics.\"\n\
The AI calls the `introspect` tool and receives a structured JSON report.\n\n\
## When to use\n\
- Editor feels slow → check performance and lock contention sections.\n\
- Shell not responding → check shell section for process status.\n\
- AI behaving oddly → check AI section for session state.\n\n\
See also: [[concept:watchdog]], [[concept:event-recording]], [[concept:ai-as-peer]], [[index]]\n";

const CONCEPT_GIT_STATUS: &str = "\
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

const CONCEPT_ORG_MODE: &str = "\
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

const CONCEPT_MARKDOWN: &str = "\
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

const CONCEPT_EX_COMMANDS: &str = "\
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

const CONCEPT_SET_SYNTAX: &str = "\
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

const CONCEPT_SCROLLBAR: &str = "\
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

const CONCEPT_AUTOSAVE: &str = "\
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

const CONCEPT_FILE_TREE: &str = "\
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

const CONCEPT_DIFF_DISPLAY: &str = "\
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

const CONCEPT_BUFFER_MODE: &str = "\
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

const CONCEPT_BUFFER_VIEW: &str = "\
The **BufferView** enum (`buffer_view.rs`) stores mode-specific state on `Buffer`. \
Variants: `Conversation`, `Help`, `Debug`, `GitStatus`, `Visual`, `FileTree`, `None`.\n\n\
Accessor methods: `buf.conversation()`, `buf.help_view()`, `buf.git_status_view()`, etc. \
Each returns `Option<&T>` (or `Option<&mut T>` for the `_mut` variant).\n\n\
This replaced 6 `Option<T>` fields that were always mutually exclusive.\n\n\
See also: [[concept:buffer]], [[concept:buffer-mode]]\n";

const CONCEPT_KEYMAP_INHERITANCE: &str = "\
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

const CONCEPT_CONCEAL: &str = "\
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

/// Load user-authored help nodes from `~/.config/mae/help/*.org`.
fn install_user_help_nodes(kb: &mut KnowledgeBase) {
    let help_dir = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".config"))
        })
        .map(|p| p.join("mae").join("help"));

    if let Some(dir) = help_dir {
        if dir.is_dir() {
            let report = kb.ingest_org_dir(&dir);
            if report.indexed > 0 {
                tracing::info!(
                    dir = %dir.display(),
                    nodes = report.indexed,
                    skipped = report.skipped_no_id,
                    "loaded user help nodes"
                );
            }
        }
    }
}

/// Install `scheme:<name>` nodes for all Scheme API functions and variables.
fn install_scheme_nodes(kb: &mut KnowledgeBase) {
    // Each entry: (name, signature, doc, example, category)
    let functions: &[(&str, &str, &str, &str, &str)] = &[
        // Buffer editing
        (
            "buffer-insert",
            "(buffer-insert TEXT)",
            "Insert TEXT at the current cursor position.",
            "(buffer-insert \"hello world\")",
            "buffer-editing",
        ),
        (
            "buffer-delete-range",
            "(buffer-delete-range START END)",
            "Delete characters from byte offset START to END.",
            "(buffer-delete-range 0 10)",
            "buffer-editing",
        ),
        (
            "buffer-replace-range",
            "(buffer-replace-range START END TEXT)",
            "Replace characters from START to END with TEXT.",
            "(buffer-replace-range 0 5 \"new\")",
            "buffer-editing",
        ),
        (
            "buffer-undo",
            "(buffer-undo)",
            "Undo the last edit in the active buffer.",
            "(buffer-undo)",
            "buffer-editing",
        ),
        (
            "buffer-redo",
            "(buffer-redo)",
            "Redo the last undone edit in the active buffer.",
            "(buffer-redo)",
            "buffer-editing",
        ),
        // Cursor / navigation
        (
            "cursor-goto",
            "(cursor-goto ROW COL)",
            "Move the cursor to absolute position (0-indexed).",
            "(cursor-goto 0 0) ; go to top-left",
            "navigation",
        ),
        (
            "open-file",
            "(open-file PATH)",
            "Open a file in a new buffer.",
            "(open-file \"/tmp/test.txt\")",
            "navigation",
        ),
        (
            "switch-to-buffer",
            "(switch-to-buffer IDX)",
            "Switch to the buffer at index IDX.",
            "(switch-to-buffer 0)",
            "navigation",
        ),
        // Buffer read
        (
            "buffer-line",
            "(buffer-line N)",
            "Return the text of line N (0-indexed) in the active buffer.",
            "(buffer-line 0) ; first line",
            "buffer-read",
        ),
        (
            "buffer-text-range",
            "(buffer-text-range START END)",
            "Return a substring of the active buffer from char START to END.",
            "(buffer-text-range 0 100)",
            "buffer-read",
        ),
        (
            "get-buffer-by-name",
            "(get-buffer-by-name NAME)",
            "Return the buffer index for NAME, or #f if not found.",
            "(get-buffer-by-name \"*scratch*\")",
            "buffer-read",
        ),
        // Commands
        (
            "define-command",
            "(define-command NAME DOC FN-NAME)",
            "Register a new command backed by a Scheme function.",
            "(define-command \"greet\" \"Say hello\" \"my-greet-fn\")",
            "commands",
        ),
        (
            "run-command",
            "(run-command NAME)",
            "Dispatch a registered command by name.",
            "(run-command \"save\")",
            "commands",
        ),
        (
            "command-exists?",
            "(command-exists? NAME)",
            "Return #t if a command with NAME is registered.",
            "(command-exists? \"save\") ; => #t",
            "commands",
        ),
        // Keymaps
        (
            "define-key",
            "(define-key MAP KEY COMMAND)",
            "Bind KEY in keymap MAP to COMMAND.",
            "(define-key \"normal\" \"g g\" \"goto-first-line\")",
            "keymaps",
        ),
        (
            "define-keymap",
            "(define-keymap NAME PARENT)",
            "Create a new keymap with an optional parent for inheritance.",
            "(define-keymap \"my-mode\" \"normal\")",
            "keymaps",
        ),
        (
            "undefine-key!",
            "(undefine-key! MAP KEY)",
            "Remove a key binding from a keymap.",
            "(undefine-key! \"normal\" \"q\")",
            "keymaps",
        ),
        (
            "keymap-bindings",
            "(keymap-bindings MAP-NAME)",
            "Return a list of (key command) pairs for a keymap.",
            "(keymap-bindings \"normal\")",
            "keymaps",
        ),
        // Options
        (
            "set-option!",
            "(set-option! KEY VALUE)",
            "Set a global editor option.",
            "(set-option! \"theme\" \"dracula\")",
            "options",
        ),
        (
            "set-local-option!",
            "(set-local-option! KEY VALUE)",
            "Set a buffer-local option on the active buffer.",
            "(set-local-option! \"word_wrap\" \"true\")",
            "options",
        ),
        (
            "get-option",
            "(get-option NAME)",
            "Return the current value of an option as a string, or #f.",
            "(get-option \"theme\") ; => \"dracula\"",
            "options",
        ),
        // Hooks
        (
            "add-hook!",
            "(add-hook! HOOK-NAME FN-NAME)",
            "Register FN-NAME to run when HOOK-NAME fires.",
            "(add-hook! \"buffer-open\" \"my-on-open\")",
            "hooks",
        ),
        (
            "remove-hook!",
            "(remove-hook! HOOK-NAME FN-NAME)",
            "Remove a function from a hook.",
            "(remove-hook! \"buffer-open\" \"my-on-open\")",
            "hooks",
        ),
        // Display
        (
            "set-status",
            "(set-status MSG)",
            "Set the status bar message.",
            "(set-status \"Done!\")",
            "display",
        ),
        (
            "set-theme",
            "(set-theme NAME)",
            "Switch the editor theme.",
            "(set-theme \"gruvbox\")",
            "display",
        ),
        (
            "message",
            "(message TEXT)",
            "Append TEXT to the *Messages* log buffer.",
            "(message \"Init complete\")",
            "display",
        ),
        // Visual buffer (canvas)
        (
            "visual-buffer-add-rect!",
            "(visual-buffer-add-rect! X Y W H FILL STROKE)",
            "Draw a rectangle on the visual canvas.",
            "(visual-buffer-add-rect! 10.0 10.0 100.0 50.0 \"#ff0000\" #f)",
            "visual",
        ),
        (
            "visual-buffer-clear!",
            "(visual-buffer-clear!)",
            "Clear all shapes from the visual canvas.",
            "(visual-buffer-clear!)",
            "visual",
        ),
        (
            "visual-buffer-add-line!",
            "(visual-buffer-add-line! X1 Y1 X2 Y2 COLOR THICKNESS)",
            "Draw a line on the visual canvas.",
            "(visual-buffer-add-line! 0.0 0.0 100.0 100.0 \"white\" 2.0)",
            "visual",
        ),
        (
            "visual-buffer-add-circle!",
            "(visual-buffer-add-circle! CX CY R FILL STROKE)",
            "Draw a circle on the visual canvas.",
            "(visual-buffer-add-circle! 50.0 50.0 25.0 \"blue\" #f)",
            "visual",
        ),
        (
            "visual-buffer-add-text!",
            "(visual-buffer-add-text! X Y TEXT FONT-SIZE COLOR)",
            "Draw text on the visual canvas.",
            "(visual-buffer-add-text! 10.0 20.0 \"Hello\" 16.0 \"white\")",
            "visual",
        ),
        // Shell
        (
            "shell-send-input",
            "(shell-send-input BUF-IDX TEXT)",
            "Send TEXT to the PTY of a terminal buffer.",
            "(shell-send-input 1 \"ls\\n\")",
            "shell",
        ),
        (
            "shell-cwd",
            "(shell-cwd BUF-IDX)",
            "Return the current working directory of a shell buffer.",
            "(shell-cwd 1)",
            "shell",
        ),
        (
            "shell-read-output",
            "(shell-read-output BUF-IDX MAX-LINES)",
            "Read the last MAX-LINES from a shell buffer's viewport.",
            "(shell-read-output 1 20)",
            "shell",
        ),
        // File I/O
        (
            "read-file",
            "(read-file PATH)",
            "Read a file's contents as a string (max 1MB).",
            "(read-file \"/etc/hostname\")",
            "file-io",
        ),
        (
            "file-exists?",
            "(file-exists? PATH)",
            "Return #t if PATH exists on disk.",
            "(file-exists? \"/tmp/test.txt\")",
            "file-io",
        ),
        (
            "list-directory",
            "(list-directory PATH)",
            "Return a list of (name is-dir?) pairs for entries in PATH.",
            "(list-directory \"/tmp\")",
            "file-io",
        ),
        // Packages
        (
            "provide-feature",
            "(provide-feature FEATURE)",
            "Mark FEATURE as loaded in the package system.",
            "(provide-feature \"my-package\")",
            "packages",
        ),
        (
            "featurep",
            "(featurep FEATURE)",
            "Return #t if FEATURE has been loaded.",
            "(featurep \"my-package\")",
            "packages",
        ),
        (
            "require-feature",
            "(require-feature FEATURE)",
            "Request loading of FEATURE from the load-path.",
            "(require-feature \"my-package\")",
            "packages",
        ),
        (
            "load-path",
            "(load-path)",
            "Return the current load-path as a list of directory strings.",
            "(load-path)",
            "packages",
        ),
        (
            "add-to-load-path!",
            "(add-to-load-path! DIR)",
            "Prepend DIR to the load-path.",
            "(add-to-load-path! \"~/.config/mae/lisp\")",
            "packages",
        ),
        (
            "autoload",
            "(autoload COMMAND FEATURE DOC)",
            "Register a command that auto-loads FEATURE on first use.",
            "(autoload \"my-cmd\" \"my-pkg\" \"Does something\")",
            "packages",
        ),
        // Recent files
        (
            "recent-files-add!",
            "(recent-files-add! PATH)",
            "Add PATH to the recent files list.",
            "(recent-files-add! \"/tmp/test.txt\")",
            "navigation",
        ),
        (
            "recent-projects-add!",
            "(recent-projects-add! PATH)",
            "Add PATH to the recent projects list.",
            "(recent-projects-add! \"~/src/my-project\")",
            "navigation",
        ),
        // Display policy
        (
            "display-buffer-policy",
            "(display-buffer-policy KIND)",
            "Query the active display rule for a BufferKind. Returns a string like \"reuse-or-split:vertical:0.5\" or \"avoid-conversation\".",
            "(display-buffer-policy \"help\")",
            "configuration",
        ),
        (
            "set-display-rule!",
            "(set-display-rule! KIND ACTION)",
            "Override the display policy for a BufferKind. ACTION formats: \"replace-focused\", \"avoid-conversation\", \"hidden\", \"reuse-or-split:vertical:0.5\".",
            "(set-display-rule! \"help\" \"replace-focused\")",
            "configuration",
        ),
    ];

    // Variables (injected from editor state before each eval)
    let variables: &[(&str, &str, &str)] = &[
        ("*buffer-name*", "string", "Name of the active buffer."),
        (
            "*buffer-modified?*",
            "boolean",
            "Whether the active buffer has unsaved changes.",
        ),
        (
            "*buffer-line-count*",
            "integer",
            "Number of lines in the active buffer.",
        ),
        (
            "*buffer-char-count*",
            "integer",
            "Total characters in the active buffer.",
        ),
        (
            "*buffer-text*",
            "string",
            "Full text content of the active buffer.",
        ),
        ("*buffer-count*", "integer", "Number of open buffers."),
        (
            "*buffer-list*",
            "list of (index name kind modified?)",
            "Information about all open buffers.",
        ),
        (
            "*buffer-language*",
            "string",
            "Detected language of the active buffer (e.g. \"rust\", \"text\").",
        ),
        (
            "*buffer-file-path*",
            "string",
            "File path of the active buffer, or empty if unsaved.",
        ),
        ("*cursor-row*", "integer", "Current cursor row (0-indexed)."),
        (
            "*cursor-col*",
            "integer",
            "Current cursor column (0-indexed).",
        ),
        (
            "*mode*",
            "string",
            "Current editor mode (\"normal\", \"insert\", \"visual\", etc).",
        ),
        ("*window-count*", "integer", "Number of open windows."),
        (
            "*window-list*",
            "list of (id buffer-idx cursor-row cursor-col)",
            "Information about all windows.",
        ),
        (
            "*shell-buffers*",
            "list of integers",
            "Buffer indices that are shell terminals.",
        ),
        (
            "*option-list*",
            "list of (name kind default doc)",
            "All registered editor options.",
        ),
        (
            "*command-list*",
            "list of (name doc source)",
            "All registered commands.",
        ),
        ("*keymap-list*", "list of strings", "Names of all keymaps."),
    ];

    for &(name, sig, doc, example, category) in functions {
        let body = format!(
            "## Signature\n```scheme\n{sig}\n```\n\n\
             {doc}\n\n\
             ## Example\n```scheme\n{example}\n```\n\n\
             **Category:** {category}\n\n\
             See also: [[concept:scheme-api]], [[index]]"
        );
        let id = format!("scheme:{}", name);
        let title = format!("Scheme: {}", name);
        kb.insert(
            Node::new(id, title, NodeKind::Concept, body).with_tags(["scheme", "api", category]),
        );
    }

    for &(name, typ, doc) in variables {
        let body = format!(
            "**Type:** {typ}\n\n\
             {doc}\n\n\
             This is a read-only variable injected from editor state before each Scheme evaluation. \
             Access it directly by name in your Scheme code.\n\n\
             ## Example\n```scheme\n(message (string-append \"Buffer: \" {name}))\n```\n\n\
             See also: [[concept:scheme-api]], [[index]]"
        );
        let id = format!("scheme:{}", name);
        let title = format!("Scheme: {}", name);
        kb.insert(
            Node::new(id, title, NodeKind::Concept, body).with_tags(["scheme", "api", "variable"]),
        );
    }
}

/// Install the progressive getting-started tutorial nodes.
fn install_tutorial_nodes(kb: &mut KnowledgeBase) {
    let nodes = vec![
        // Hub
        Node::new(
            "tutorial:getting-started",
            "Getting Started with MAE",
            NodeKind::Concept,
            TUTORIAL_GETTING_STARTED,
        )
        .with_tags(["tutorial"]),
        // Vim track
        Node::new(
            "tutorial:vim-familiar",
            "Tutorial: What Carries Over from Vim",
            NodeKind::Concept,
            TUTORIAL_VIM_FAMILIAR,
        )
        .with_tags(["tutorial", "vim"]),
        Node::new(
            "tutorial:vim-differences",
            "Tutorial: What's Different from Vim",
            NodeKind::Concept,
            TUTORIAL_VIM_DIFFERENCES,
        )
        .with_tags(["tutorial", "vim"]),
        // Beginner track
        Node::new(
            "tutorial:what-is-modal",
            "Tutorial: What Is Modal Editing?",
            NodeKind::Concept,
            TUTORIAL_WHAT_IS_MODAL,
        )
        .with_tags(["tutorial", "beginner"]),
        Node::new(
            "tutorial:basic-movement",
            "Tutorial: Basic Movement",
            NodeKind::Concept,
            TUTORIAL_BASIC_MOVEMENT,
        )
        .with_tags(["tutorial", "beginner"]),
        Node::new(
            "tutorial:basic-editing",
            "Tutorial: Basic Editing",
            NodeKind::Concept,
            TUTORIAL_BASIC_EDITING,
        )
        .with_tags(["tutorial", "beginner"]),
        // Shared convergence nodes
        Node::new(
            "tutorial:mae-navigation",
            "Tutorial: MAE Navigation",
            NodeKind::Concept,
            TUTORIAL_MAE_NAVIGATION,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "tutorial:mae-extending",
            "Tutorial: Extending MAE",
            NodeKind::Concept,
            TUTORIAL_MAE_EXTENDING,
        )
        .with_tags(["tutorial"]),
        // AI track
        Node::new(
            "tutorial:ai-setup",
            "Tutorial: AI Setup",
            NodeKind::Concept,
            TUTORIAL_AI_SETUP,
        )
        .with_tags(["tutorial", "ai"]),
        Node::new(
            "tutorial:ai-agent",
            "Tutorial: AI Agent (Terminal)",
            NodeKind::Concept,
            TUTORIAL_AI_AGENT,
        )
        .with_tags(["tutorial", "ai"]),
        Node::new(
            "tutorial:ai-chat",
            "Tutorial: AI Chat (Built-in)",
            NodeKind::Concept,
            TUTORIAL_AI_CHAT,
        )
        .with_tags(["tutorial", "ai"]),
    ];

    for node in nodes {
        kb.insert(node);
    }
}

// --- Tutorial content ---

const TUTORIAL_GETTING_STARTED: &str = "\
# Getting Started with MAE\n\n\
MAE (Modern AI Editor) is an AI-native Lisp machine editor with modal editing.\n\n\
Choose your track:\n\n\
## I know Vim\n\
→ [[tutorial:vim-familiar|Start here]] — what carries over, what's different, MAE extensions\n\n\
## I'm new to modal editing\n\
→ [[tutorial:what-is-modal|Start here]] — what modes are, basic movement, basic editing\n\n\
## Set up AI\n\
→ [[tutorial:ai-setup|AI Setup]] — API key configuration, provider selection, agent vs chat\n\n\
Each track is a linked sequence of short lessons. Follow the **Next:** links at the bottom.\n\n\
See also: [[tutor:index|Lesson-style Tutorial]], [[index|Help Index]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_VIM_FAMILIAR: &str = "\
# What Carries Over from Vim\n\n\
If you know Vim, you already know most of MAE. These all work as expected:\n\n\
## Movement\n\
`h`/`j`/`k`/`l`, `w`/`b`/`e`, `gg`/`G`, `0`/`$`, `Ctrl-d`/`Ctrl-u`, `f`/`t`, `%`\n\n\
## Modes\n\
Normal, Insert (`i`/`a`/`o`/`O`), Visual (`v`/`V`), Command (`:`)\n\n\
## Editing\n\
`dd`/`yy`/`p`/`P`, `ciw`/`diw`/`yiw`, `x`/`r`, `.` (dot-repeat), `u`/`Ctrl-r`\n\n\
## Text objects\n\
`iw`/`aw`, `i\"`/`a\"`, `i(`/`a(`, `i{`/`a{`, `it`/`at`\n\n\
## Registers\n\
`\"ay` to yank into register a, `\"ap` to paste from it. `\"+` for system clipboard.\n\n\
## Ex commands\n\
`:w`, `:q`, `:wq`, `:e file`, `:set option`, `/search`\n\n\
## Macros\n\
`q{reg}` to record, `q` to stop, `@{reg}` to replay\n\n\
**Next:** [[tutorial:vim-differences|What's Different from Vim]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_VIM_DIFFERENCES: &str = "\
# What's Different from Vim\n\n\
## SPC leader instead of backslash\n\
MAE uses **Space** as the leader key (Doom Emacs style). This gives access \
to 14+ command groups:\n\
- `SPC f` — file operations\n\
- `SPC b` — buffer operations\n\
- `SPC w` — window operations\n\
- `SPC a` — AI commands\n\
- `SPC h` — help system\n\
- `SPC p` — project commands\n\
...and more. A **which-key** popup appears after pressing SPC.\n\n\
## Scheme instead of VimL/Lua\n\
Configuration is in `init.scm` (R7RS Scheme), not `.vimrc` or `init.lua`.\n\
Edit it with `SPC f c`.\n\n\
```scheme\n\
(set-option! \"theme\" \"dracula\")\n\
(define-key \"normal\" \"g g\" \"goto-first-line\")\n\
(add-hook! \"buffer-open\" \"my-on-open\")\n\
```\n\n\
## Built-in AI\n\
- `SPC a p` — AI Chat (built-in conversation with editor context)\n\
- `SPC a a` — AI Agent (terminal-based, e.g. Claude Code)\n\
See [[tutorial:ai-setup|AI Setup]] for configuration.\n\n\
## No plugins — packages\n\
MAE uses a Scheme-based package system with `require-feature`/`provide-feature` \
instead of Vim plugins. See [[concept:package-system|Package System]].\n\n\
**Next:** [[tutorial:mae-navigation|MAE Navigation]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_WHAT_IS_MODAL: &str = "\
# What Is Modal Editing?\n\n\
In most editors, pressing `j` types the letter \"j\". In MAE, what a key does \
depends on which **mode** you're in.\n\n\
## Normal mode (default)\n\
Keys are **commands**: `j` moves down, `dd` deletes a line, `w` jumps to the next word.\n\
You navigate and manipulate text without ever reaching for the mouse.\n\n\
## Insert mode\n\
Keys type text, like a normal editor. Press `i` from Normal mode to enter Insert mode.\n\
Press `Escape` to return to Normal mode.\n\n\
## Visual mode\n\
Select text with movement keys. Press `v` from Normal mode.\n\n\
## Command mode\n\
Type commands after `:`. Press `:` from Normal mode.\n\n\
## Why modal?\n\
- Your fingers never leave the home row\n\
- Every key does something useful (no wasted Ctrl-Shift-Alt chords)\n\
- Composable: `d` + `w` = delete word, `c` + `i` + `\"` = change inside quotes\n\n\
**The golden rule:** If you get lost, press **Escape** to return to Normal mode.\n\n\
**Next:** [[tutorial:basic-movement|Basic Movement]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_BASIC_MOVEMENT: &str = "\
# Basic Movement\n\n\
All movement happens in **Normal mode** (press Escape if you're elsewhere).\n\n\
## Character movement\n\
```\n\
     k\n\
  h     l\n\
     j\n\
```\n\
`h` = left, `j` = down, `k` = up, `l` = right\n\n\
## Word movement\n\
- `w` — jump to next word start\n\
- `b` — jump to previous word start\n\
- `e` — jump to next word end\n\n\
## Line movement\n\
- `0` — beginning of line\n\
- `$` — end of line\n\
- `^` — first non-blank character\n\n\
## File movement\n\
- `gg` — go to first line\n\
- `G` — go to last line\n\
- `Ctrl-d` — half page down\n\
- `Ctrl-u` — half page up\n\n\
## Searching\n\
- `/pattern` — search forward\n\
- `n` — next match\n\
- `N` — previous match\n\n\
**Try it:** Open a file with `:e filename` and practice moving around!\n\n\
**Next:** [[tutorial:basic-editing|Basic Editing]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_BASIC_EDITING: &str = "\
# Basic Editing\n\n\
## Entering Insert mode\n\
- `i` — insert before cursor\n\
- `a` — insert after cursor\n\
- `o` — open new line below\n\
- `O` — open new line above\n\
- `Escape` — return to Normal mode\n\n\
## Deleting\n\
- `x` — delete character under cursor\n\
- `dd` — delete entire line\n\
- `dw` — delete from cursor to next word\n\
- `d$` — delete to end of line\n\n\
## Copy and paste\n\
- `yy` — yank (copy) entire line\n\
- `yw` — yank word\n\
- `p` — paste after cursor\n\
- `P` — paste before cursor\n\n\
## Undo and redo\n\
- `u` — undo\n\
- `Ctrl-r` — redo\n\n\
## Saving and quitting\n\
- `:w` — save\n\
- `:q` — quit\n\
- `:wq` — save and quit\n\
- `:q!` — quit without saving\n\n\
## The dot command\n\
`.` repeats your last edit. Delete a word with `dw`, then press `.` to delete another.\n\n\
**Next:** [[tutorial:mae-navigation|MAE Navigation]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_MAE_NAVIGATION: &str = "\
# MAE Navigation\n\n\
MAE's **SPC leader** gives fast access to every subsystem.\n\n\
## Files\n\
- `SPC f f` — fuzzy find file in project\n\
- `SPC f d` — file browser (directory listing)\n\
- `SPC f t` — toggle file tree sidebar\n\
- `SPC f r` — recent files\n\
- `SPC f c` — edit config (init.scm)\n\n\
## Buffers\n\
- `SPC b b` — switch buffer (fuzzy palette)\n\
- `SPC b d` — close current buffer\n\
- `SPC b n` / `SPC b p` — next / previous buffer\n\n\
## Windows\n\
- `SPC w v` — split vertically\n\
- `SPC w s` — split horizontally\n\
- `SPC w h/j/k/l` — focus left/down/up/right window\n\
- `SPC w q` — close window\n\
- `SPC w =` — balance window sizes\n\n\
## Project\n\
- `SPC p s` — search text in project (grep)\n\
- `SPC p f` — find file in project\n\n\
## Search\n\
- `/` — search in current buffer\n\
- `SPC p s` — search across project\n\
- `SPC SPC` — command palette (search for any command)\n\n\
## Help\n\
- `SPC h` — help menu\n\
- `SPC h s` — search help topics\n\
- `SPC h k` — describe key\n\
- `:help topic` — look up a topic\n\n\
**Next:** [[tutorial:mae-extending|Extending MAE]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_MAE_EXTENDING: &str = "\
# Extending MAE\n\n\
MAE is extensible via **R7RS Scheme** (the Steel runtime).\n\n\
## The REPL\n\
- `:eval (+ 1 2)` — evaluate an expression (result shown in status bar)\n\
- `SPC e e` — evaluate current line\n\
- `SPC e b` — evaluate entire buffer\n\n\
## Configuration (init.scm)\n\
Your config file is `~/.config/mae/init.scm`. Open it with `SPC f c`.\n\n\
```scheme\n\
;; Set theme\n\
(set-option! \"theme\" \"dracula\")\n\n\
;; Custom keybinding\n\
(define-key \"normal\" \"g r\" \"lsp-find-references\")\n\n\
;; React to editor events\n\
(add-hook! \"buffer-save\" \"my-on-save\")\n\n\
;; Define a custom command\n\
(define-command \"hello\" \"Say hello\" \"my-hello-fn\")\n\
(define (my-hello-fn) (set-status \"Hello from Scheme!\"))\n\
```\n\n\
## Packages\n\
Place `.scm` files in `~/.config/mae/packages/`. Use `require-feature` and \
`provide-feature` to manage dependencies.\n\
See [[concept:package-system|Package System]] for details.\n\n\
## Full Scheme API\n\
MAE exposes 45+ functions and 18 variables to Scheme.\n\
See [[concept:scheme-api|Scheme API]] for the full reference, or use \
`:help scheme:function-name` for individual docs.\n\n\
See also: [[tutorial:ai-setup|Set up AI]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_AI_SETUP: &str = "\
# AI Setup\n\n\
MAE has two AI interfaces: **AI Agent** (terminal) and **AI Chat** (built-in).\n\n\
## AI Chat — built-in conversation\n\
The built-in AI has full access to your editor context (buffers, LSP, diagnostics).\n\n\
### Configure provider\n\
In `~/.config/mae/config.toml`:\n\
```toml\n\
[ai]\n\
provider = \"claude\"        # or \"openai\", \"gemini\", \"deepseek\"\n\
model = \"claude-sonnet-4-20250514\"\n\
```\n\n\
### API key\n\
Set the appropriate environment variable:\n\
- Claude: `ANTHROPIC_API_KEY`\n\
- OpenAI: `OPENAI_API_KEY`\n\
- Gemini: `GEMINI_API_KEY`\n\
- DeepSeek: `DEEPSEEK_API_KEY`\n\n\
Or use `api_key_command` to fetch from a secrets manager:\n\
```toml\n\
[ai]\n\
api_key_command = \"pass show api/anthropic\"\n\
```\n\n\
## AI Agent — terminal-based\n\
`SPC a a` opens an external AI agent (Claude Code, gemini-cli, etc.) in a terminal.\n\
Configure which command to run:\n\
```toml\n\
[ai]\n\
editor = \"claude\"          # command to run\n\
```\n\n\
## Verify setup\n\
Press `SPC a p` and type a message. If you see a response, AI Chat is working.\n\
Press `SPC a a` to launch the agent terminal.\n\n\
**Next:** [[tutorial:ai-agent|AI Agent (Terminal)]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_AI_AGENT: &str = "\
# AI Agent (Terminal)\n\n\
The **AI Agent** (`SPC a a`) runs an external tool like Claude Code or gemini-cli \
in MAE's embedded terminal.\n\n\
## How it works\n\
1. Press `SPC a a` — MAE opens a terminal and runs the configured `ai.editor` command\n\
2. The agent has access to your project files via the filesystem\n\
3. Via the MCP bridge, the agent can also call MAE's editor tools\n\n\
## Configuration\n\
```toml\n\
[ai]\n\
editor = \"claude\"          # or \"gemini\", or a custom command\n\
```\n\n\
## When to use\n\
- Autonomous coding tasks (write a feature, fix a bug)\n\
- Complex multi-file refactors\n\
- Tasks that need shell access (running tests, installing packages)\n\
- When you want the AI to drive and you review\n\n\
## Terminal controls\n\
- `Ctrl-\\ Ctrl-n` — exit terminal mode (return to Normal)\n\
- The agent's terminal is a full VT100 emulator (colors, scrollback)\n\n\
**Next:** [[tutorial:ai-chat|AI Chat (Built-in)]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_AI_CHAT: &str = "\
# AI Chat (Built-in)\n\n\
The **AI Chat** (`SPC a p`) is MAE's native conversation interface.\n\n\
## How it works\n\
1. Press `SPC a p` — the prompt line activates at the bottom of the conversation buffer\n\
2. Type your message and press Enter\n\
3. The AI responds with full editor context: it can see your buffers, LSP diagnostics, \
syntax trees, and debug state\n\n\
## What the AI can do\n\
- Read and edit buffers\n\
- Navigate files and project structure\n\
- Run registered commands\n\
- Query LSP (definitions, references, hover)\n\
- Inspect DAP debug state\n\
- Search the knowledge base\n\n\
## Configuration\n\
```toml\n\
[ai]\n\
provider = \"claude\"\n\
model = \"claude-sonnet-4-20250514\"\n\
permission = \"trusted\"      # readonly, standard, trusted, privileged\n\
budget_warn_tokens = 50000\n\
budget_limit_tokens = 100000\n\
```\n\n\
## Conversation persistence\n\
Conversations are saved per project in `.mae/conversation.json`.\n\
- `:ai-save` — manually save the conversation\n\
- `:ai-load` — load a saved conversation\n\
- `restore_session = true` — auto-restore on startup\n\n\
## Tips\n\
- Use `SPC a c` to cancel an in-flight AI request\n\
- Use `Escape` during AI operation to cancel and regain input\n\
- The token budget dashboard shows usage in the status bar\n\n\
See also: [[concept:ai-as-peer|AI as Peer]], [[concept:ai-modes|Agent vs Chat]]\n\n\
* Getting Help\n\
- `SPC h` opens the help system\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

/// Install a `cmd:<name>` node for every registered command. Source
/// (builtin vs scheme) is surfaced in the body so users can tell which
/// commands are implemented in Rust vs Scheme.
fn install_command_nodes(
    kb: &mut KnowledgeBase,
    registry: &CommandRegistry,
    keybinding_map: &HashMap<String, Vec<(String, String)>>,
    hooks: &HookRegistry,
) {
    for cmd in registry.list_commands() {
        let source_line = match &cmd.source {
            crate::commands::CommandSource::Builtin => "**Source:** built-in (Rust)".to_string(),
            crate::commands::CommandSource::Scheme(fn_name) => {
                format!("**Source:** Scheme (`{}`)", fn_name)
            }
            crate::commands::CommandSource::Autoload { feature } => {
                format!("**Source:** autoload (feature `{}`)", feature)
            }
        };
        let category = infer_category(&cmd.name);

        let keybindings_section = match keybinding_map.get(&cmd.name) {
            Some(bindings) if !bindings.is_empty() => {
                let mut lines = String::from("\n\n**Keybindings:**\n");
                for (mode, key) in bindings {
                    lines.push_str(&format!("  {}: `{}`\n", mode, key));
                }
                lines
            }
            _ => String::new(),
        };

        let hook_names = hooks.hooks_containing(&cmd.name);
        let hooks_section = if hook_names.is_empty() {
            String::new()
        } else {
            format!("\n\n**Hooks:** {}", hook_names.join(", "))
        };

        let body = format!(
            "{doc}\n\n**Category:** {category}\n{source_line}{keybindings}{hooks}\n\nSee also: [[index]], [[concept:command]], [[category:{category}]]",
            doc = cmd.doc,
            category = category,
            source_line = source_line,
            keybindings = keybindings_section,
            hooks = hooks_section,
        );
        let id = format!("cmd:{}", cmd.name);
        let title = format!("Command: {}", cmd.name);
        kb.insert(Node::new(id, title, NodeKind::Command, body));
    }
}

/// Install one `category:<name>` index node per distinct category.
fn install_category_nodes(
    kb: &mut KnowledgeBase,
    registry: &CommandRegistry,
    keybinding_map: &HashMap<String, Vec<(String, String)>>,
) {
    let mut categories: HashMap<&str, Vec<&str>> = HashMap::new();
    for cmd in registry.list_commands() {
        let cat = infer_category(&cmd.name);
        categories.entry(cat).or_default().push(&cmd.name);
    }
    for (cat, mut commands) in categories {
        commands.sort();
        let mut body = format!("Commands in the **{}** category:\n\n", cat);
        for name in &commands {
            let binding_hint = match keybinding_map.get(*name) {
                Some(bindings) if !bindings.is_empty() => {
                    let keys: Vec<String> = bindings
                        .iter()
                        .map(|(m, k)| format!("{}: `{}`", m, k))
                        .collect();
                    format!(" ({})", keys.join(", "))
                }
                _ => String::new(),
            };
            body.push_str(&format!("- [[cmd:{}]]{}\n", name, binding_hint));
        }
        body.push_str("\nSee also: [[index]], [[concept:command]]");
        let id = format!("category:{}", cat);
        let title = format!("Category: {}", cat);
        kb.insert(Node::new(id, title, NodeKind::Concept, body).with_tags(["category"]));
    }
}

/// Install an `option:<name>` node for every registered option.
fn install_option_nodes(kb: &mut KnowledgeBase) {
    let registry = OptionRegistry::new();
    for def in registry.list() {
        let aliases = if def.aliases.is_empty() {
            String::new()
        } else {
            format!(
                "\n**Aliases:** {}",
                def.aliases
                    .iter()
                    .map(|a| format!("`{}`", a))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let config_line = match def.config_key {
            Some(key) => format!("\n**Config key:** `{}`", key),
            None => String::new(),
        };
        let body = format!(
            "{doc}\n\n\
             **Type:** {kind}  \n\
             **Default:** `{default}`{aliases}{config}\n\n\
             ## Usage\n\
             ```\n\
             :set {name} <value>       \" set from command line\n\
             :set {name}               \" toggle (booleans) or show current value\n\
             :set-save {name} <value>  \" set and persist to config.toml\n\
             ```\n\
             ```scheme\n\
             (set-option! \"{scheme_name}\" \"<value>\")  ; set from Scheme\n\
             ```\n\n\
             See also: [[concept:options]], [[index]]",
            doc = def.doc,
            kind = def.kind,
            default = def.default_value,
            aliases = aliases,
            config = config_line,
            name = def.name,
            scheme_name = def.aliases.first().unwrap_or(&def.name),
        );
        let id = format!("option:{}", def.name);
        let title = format!("Option: {}", def.name);
        kb.insert(
            Node::new(id, title, NodeKind::Concept, body).with_tags(["option", "configuration"]),
        );
    }
}

/// Static hand-authored concept/index/key nodes.
fn static_nodes() -> Vec<Node> {
    vec![
        Node::new("index", "MAE Help Index", NodeKind::Index, INDEX_BODY),
        Node::new(
            "concept:buffer",
            "Concept: Buffer",
            NodeKind::Concept,
            CONCEPT_BUFFER,
        )
        .with_tags(["data-model", "core"]),
        Node::new(
            "concept:window",
            "Concept: Window",
            NodeKind::Concept,
            CONCEPT_WINDOW,
        )
        .with_tags(["data-model", "core"]),
        Node::new(
            "concept:mode",
            "Concept: Mode",
            NodeKind::Concept,
            CONCEPT_MODE,
        )
        .with_tags(["data-model", "modal-editing"]),
        Node::new(
            "concept:command",
            "Concept: Command",
            NodeKind::Concept,
            CONCEPT_COMMAND,
        )
        .with_tags(["data-model", "extensibility"]),
        Node::new(
            "concept:ai-as-peer",
            "Concept: The AI as Peer Actor",
            NodeKind::Concept,
            CONCEPT_AI_AS_PEER,
        )
        .with_tags(["ai", "architecture"]),
        Node::new(
            "concept:knowledge-base",
            "Concept: Knowledge Base",
            NodeKind::Concept,
            CONCEPT_KB,
        )
        .with_tags(["kb", "architecture"]),
        Node::new(
            "key:normal-mode",
            "Keys: Normal Mode",
            NodeKind::Key,
            KEY_NORMAL,
        )
        .with_tags(["keys", "modal-editing"]),
        Node::new(
            "key:leader-keys",
            "Keys: SPC Leader Bindings",
            NodeKind::Key,
            KEY_LEADER,
        )
        .with_tags(["keys", "leader", "doom"]),
        Node::new(
            "concept:project",
            "Concept: Project",
            NodeKind::Concept,
            CONCEPT_PROJECT,
        )
        .with_tags(["project", "workflow"]),
        Node::new(
            "concept:terminal",
            "Concept: Embedded Terminal",
            NodeKind::Concept,
            CONCEPT_TERMINAL,
        )
        .with_tags(["terminal", "shell", "phase-6"]),
        Node::new(
            "concept:hooks",
            "Concept: Hooks",
            NodeKind::Concept,
            CONCEPT_HOOKS,
        )
        .with_tags(["hooks", "extensibility", "scheme"]),
        Node::new(
            "concept:options",
            "Concept: Editor Options",
            NodeKind::Concept,
            CONCEPT_OPTIONS,
        )
        .with_tags(["options", "configuration", "scheme"]),
        Node::new(
            "concept:agent-bootstrap",
            "Concept: Agent Bootstrap",
            NodeKind::Concept,
            CONCEPT_AGENT_BOOTSTRAP,
        )
        .with_tags(["agents", "mcp", "ai"]),
        Node::new(
            "concept:self-test",
            "Concept: AI Self-Test",
            NodeKind::Concept,
            CONCEPT_SELF_TEST,
        )
        .with_tags(["ai", "testing", "tools"]),
        Node::new(
            "concept:debugging",
            "Concept: Debugging (DAP)",
            NodeKind::Concept,
            CONCEPT_DEBUGGING,
        )
        .with_tags(["dap", "debugging", "ai"]),
        Node::new(
            "concept:gui",
            "Concept: GUI Backend",
            NodeKind::Concept,
            CONCEPT_GUI,
        )
        .with_tags(["rendering", "gui"]),
        Node::new(
            "concept:watchdog",
            "Concept: Watchdog",
            NodeKind::Concept,
            CONCEPT_WATCHDOG,
        )
        .with_tags(["debugging", "observability"]),
        Node::new(
            "concept:event-recording",
            "Concept: Event Recording",
            NodeKind::Concept,
            CONCEPT_EVENT_RECORDING,
        )
        .with_tags(["debugging", "observability"]),
        Node::new(
            "concept:dap-attach",
            "Concept: DAP Attach",
            NodeKind::Concept,
            CONCEPT_DAP_ATTACH,
        )
        .with_tags(["debugging", "dap"]),
        Node::new(
            "concept:introspect",
            "Concept: Introspect",
            NodeKind::Concept,
            CONCEPT_INTROSPECT,
        )
        .with_tags(["debugging", "ai", "observability"]),
        Node::new(
            "concept:git-status",
            "Concept: Git Status (Magit-lite)",
            NodeKind::Concept,
            CONCEPT_GIT_STATUS,
        )
        .with_tags(["git", "workflow"]),
        Node::new(
            "concept:org-mode",
            "Concept: Org-mode",
            NodeKind::Concept,
            CONCEPT_ORG_MODE,
        )
        .with_tags(["org", "editing"]),
        Node::new(
            "concept:markdown",
            "Concept: Markdown",
            NodeKind::Concept,
            CONCEPT_MARKDOWN,
        )
        .with_tags(["markdown", "editing"]),
        Node::new(
            "concept:ex-commands",
            "Concept: Ex-Command Grammar",
            NodeKind::Concept,
            CONCEPT_EX_COMMANDS,
        )
        .with_tags(["commands", "vim"]),
        Node::new(
            "concept:set-syntax",
            "Concept: :set Option Syntax",
            NodeKind::Concept,
            CONCEPT_SET_SYNTAX,
        )
        .with_tags(["options", "configuration", "vim"]),
        Node::new(
            "concept:scrollbar",
            "Concept: Scrollbar",
            NodeKind::Concept,
            CONCEPT_SCROLLBAR,
        )
        .with_tags(["gui", "rendering"]),
        Node::new(
            "concept:autosave",
            "Concept: Autosave",
            NodeKind::Concept,
            CONCEPT_AUTOSAVE,
        )
        .with_tags(["files", "configuration"]),
        Node::new(
            "concept:file-tree",
            "Concept: File Tree",
            NodeKind::Concept,
            CONCEPT_FILE_TREE,
        )
        .with_tags(["files", "navigation", "gui"]),
        Node::new(
            "concept:diff-display",
            "Concept: Diff Display",
            NodeKind::Concept,
            CONCEPT_DIFF_DISPLAY,
        )
        .with_tags(["ai", "diff", "rendering"]),
        Node::new(
            "concept:conceal",
            "Concept: Conceal (Link & Markup Rendering)",
            NodeKind::Concept,
            CONCEPT_CONCEAL,
        )
        .with_tags(["rendering", "configuration", "conversation"]),
        Node::new(
            "concept:buffer-mode",
            "Concept: BufferMode Trait",
            NodeKind::Concept,
            CONCEPT_BUFFER_MODE,
        )
        .with_tags(["data-model", "core", "extensibility"]),
        Node::new(
            "concept:buffer-view",
            "Concept: BufferView Enum",
            NodeKind::Concept,
            CONCEPT_BUFFER_VIEW,
        )
        .with_tags(["data-model", "core"]),
        Node::new(
            "concept:keymap-inheritance",
            "Concept: Keymap Inheritance",
            NodeKind::Concept,
            CONCEPT_KEYMAP_INHERITANCE,
        )
        .with_tags(["data-model", "modal-editing", "extensibility"]),
        Node::new(
            "concept:package-system",
            "Concept: Package System",
            NodeKind::Concept,
            CONCEPT_PACKAGE_SYSTEM,
        )
        .with_tags(["extensibility", "scheme", "packages"]),
        Node::new(
            "concept:option-registry",
            "Concept: Option Registry",
            NodeKind::Concept,
            CONCEPT_OPTION_REGISTRY,
        )
        .with_tags(["configuration", "core", "options"]),
        Node::new(
            "concept:scheme-api",
            "Concept: Scheme API",
            NodeKind::Concept,
            CONCEPT_SCHEME_API,
        )
        .with_tags(["extensibility", "scheme", "api"]),
        Node::new(
            "concept:ai-modes",
            "Concept: AI Agent vs AI Chat",
            NodeKind::Concept,
            CONCEPT_AI_MODES,
        )
        .with_tags(["ai", "configuration"]),
        Node::new(
            "concept:prompt-tiers",
            "Concept: Prompt Tiers",
            NodeKind::Concept,
            CONCEPT_PROMPT_TIERS,
        )
        .with_tags(["ai", "configuration"]),
        Node::new(
            "concept:display-policy",
            "Concept: Display Policy",
            NodeKind::Concept,
            CONCEPT_DISPLAY_POLICY,
        )
        .with_tags(["core", "window", "conversation"]),
        Node::new(
            "concept:mcp-development",
            "Concept: MCP Development Workflow",
            NodeKind::Concept,
            CONCEPT_MCP_DEVELOPMENT,
        )
        .with_tags(["mcp", "ai", "tools", "development"]),
    ]
}

const CONCEPT_PROMPT_TIERS: &str = "\
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

const CONCEPT_DISPLAY_POLICY: &str = "\
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

const CONCEPT_MCP_DEVELOPMENT: &str = "\
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
Socket: `$XDG_RUNTIME_DIR/mae-mcp.sock`\n\
Shim: `mae-mcp-shim` (stdio ↔ Unix socket bridge)\n\n\
### When to Use\n\
- **Navigating MAE code:** `lsp_definition` / `lsp_references` over grep — structured, no false positives\n\
- **Understanding architecture:** `kb_search` or `kb_get` — curated docs\n\
- **Debugging MAE:** `dap_start` with `lldb-dap`, `debug_state` for inspection\n\
- **Testing changes:** `execute_command`, `self_test_suite` for E2E\n\n\
See also: [[concept:ai-as-peer]], [[concept:agent-bootstrap]], [[concept:self-test]]\n";

const INDEX_BODY: &str = "Welcome to MAE's built-in help. This knowledge base is the same data \
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
- [[concept:scheme-api|Scheme API]] — 40+ functions for buffer/window/command/keymap access\n\
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

const CONCEPT_BUFFER: &str = "A **buffer** is the unit of editable content in MAE.\n\
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

const CONCEPT_WINDOW: &str =
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

const CONCEPT_MODE: &str = "MAE is **modal** like Vim. The current [[concept:mode|Mode]] \
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

const CONCEPT_COMMAND: &str =
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

const CONCEPT_AI_AS_PEER: &str = "MAE's single-most-important design stance: **the AI agent is a peer actor, not a plugin.**\n\n\
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

const CONCEPT_KB: &str = "MAE's **knowledge base** is a typed graph of nodes with \
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

const KEY_NORMAL: &str = "## Normal-mode keys (summary)\n\n\
### Movement\n\
- `h j k l` — left / down / up / right\n\
- `w` / `b` / `e` — next word / previous word / end of word (see [[cmd:move-word-forward]])\n\
- `0` / `$` — start / end of line\n\
- `gg` / `G` — first / last line\n\
- `f<char>` — find char on line\n\n\
### Operators (compose with any motion)\n\
- `d{motion}` — delete (e.g. `dw`, `dG`, `dgg`, `d%`, `d}`)\n\
- `c{motion}` — change (delete + enter insert)\n\
- `y{motion}` — yank (copy)\n\
- `dd` / `cc` / `yy` — linewise specials\n\
- `di(` / `ca\"` / `yi{` — text objects\n\n\
### Editing\n\
- `i` / `a` — enter insert mode (before / after cursor) ([[cmd:enter-insert-mode]])\n\
- `o` / `O` — open line below / above ([[cmd:open-line-below]])\n\
- `u` / `C-r` — undo / redo ([[cmd:undo]], [[cmd:redo]])\n\n\
### Leader keys (SPC)\n\
See [[key:leader-keys]] for the full SPC leader reference.\n\n\
### Windows, buffers, files\n\
- `:e <path>` — open file\n\
- `:ls` — list buffers ([[cmd:list-buffers]])\n\
- `C-^` — switch to alternate buffer\n\n\
### Help\n\
- `:help` — open this page\n\
- `:describe-command <name>` — show docs for any command\n\n\
See also: [[index]], [[concept:mode]]\n";

const KEY_LEADER: &str = "## SPC Leader Bindings (Doom Emacs style)\n\n\
MAE uses `SPC` as leader in normal mode, organized into 14 groups.\n\
Press `SPC` to see the which-key popup showing available sub-keys.\n\n\
### SPC SPC — Command Palette\n\
Fuzzy-search all commands (like Doom's `M-x` or VSCode's `Ctrl-Shift-P`).\n\n\
### SPC / — Project Search\n\
Quick shortcut for `project-search` (ripgrep in project root).\n\n\
### SPC b — +buffer\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s` | [[cmd:save]] | Save current buffer |\n\
| `b` | [[cmd:switch-buffer]] | Switch buffer (fuzzy) |\n\
| `d` | [[cmd:kill-buffer]] | Kill buffer |\n\
| `n` | [[cmd:next-buffer]] | Next buffer |\n\
| `p` | [[cmd:prev-buffer]] | Previous buffer |\n\
| `l` | [[cmd:alternate-file]] | Alternate file |\n\
| `m` | [[cmd:view-messages]] | Messages buffer |\n\
| `N` | [[cmd:new-buffer]] | New buffer |\n\
| `D` | [[cmd:force-kill-buffer]] | Force kill |\n\
| `o` | [[cmd:kill-other-buffers]] | Kill other buffers |\n\
| `S` | [[cmd:save-all-buffers]] | Save all |\n\
| `r` | [[cmd:revert-buffer]] | Revert from disk |\n\n\
### SPC f — +file\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `f` | [[cmd:find-file]] | Open file picker |\n\
| `d` | [[cmd:file-browser]] | Directory browser |\n\
| `s` | [[cmd:save]] | Save |\n\
| `r` | [[cmd:recent-files]] | Recent files |\n\
| `y` | [[cmd:yank-file-path]] | Yank file path |\n\
| `R` | [[cmd:rename-file]] | Rename file |\n\
| `S` | [[cmd:save-as]] | Save as |\n\
| `c` | [[cmd:edit-config]] | Edit config |\n\n\
### SPC p — +project\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `f` | [[cmd:project-find-file]] | Find file in project |\n\
| `s` | [[cmd:project-search]] | Grep in project |\n\
| `d` | [[cmd:project-browse]] | Browse project dir |\n\
| `r` | [[cmd:project-recent-files]] | Recent project files |\n\n\
### SPC w — +window\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `v` | [[cmd:split-vertical]] | Vertical split |\n\
| `s` | [[cmd:split-horizontal]] | Horizontal split |\n\
| `q` | [[cmd:close-window]] | Close window |\n\
| `h/j/k/l` | focus-{dir} | Move focus |\n\n\
### SPC s — +search/syntax\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s` | [[cmd:search-buffer]] | Search in buffer |\n\
| `n` | [[cmd:syntax-select-node]] | Select syntax node |\n\
| `e` | [[cmd:syntax-expand-selection]] | Expand selection |\n\
| `c` | [[cmd:syntax-contract-selection]] | Contract selection |\n\
| `p` | [[cmd:project-search]] | Project search |\n\
| `h` | [[cmd:clear-search-highlight]] | Clear highlights |\n\n\
### SPC c — +code\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `d` | [[cmd:lsp-goto-definition]] | Go to definition |\n\
| `r` | [[cmd:lsp-find-references]] | Find references |\n\
| `k` | [[cmd:lsp-hover]] | Hover info |\n\
| `x` | [[cmd:lsp-show-diagnostics]] | Diagnostics |\n\
| `a` | [[cmd:lsp-code-action]] | Code action |\n\
| `R` | [[cmd:lsp-rename]] | Rename symbol |\n\
| `f` | [[cmd:lsp-format]] | Format |\n\n\
### SPC g — +git\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s` | [[cmd:git-status]] | Git status |\n\
| `b` | [[cmd:git-blame]] | Git blame |\n\
| `d` | [[cmd:git-diff]] | Git diff |\n\
| `l` | [[cmd:git-log]] | Git log |\n\n\
### SPC t — +toggle\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `t` | [[cmd:cycle-theme]] | Cycle theme |\n\
| `s` | [[cmd:set-theme]] | Set theme |\n\
| `l` | [[cmd:toggle-line-numbers]] | Line numbers |\n\
| `r` | [[cmd:toggle-relative-line-numbers]] | Relative numbers |\n\
| `w` | [[cmd:toggle-word-wrap]] | Word wrap |\n\
| `F` | [[cmd:toggle-fps]] | FPS overlay |\n\n\
### SPC a — +ai\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `p` | [[cmd:ai-prompt]] | AI prompt |\n\
| `a` | [[cmd:open-ai-agent]] | Launch agent in shell |\n\
| `c` | [[cmd:ai-cancel]] | Cancel AI |\n\n\
### SPC g — +git\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s` | [[cmd:git-status]] | Status buffer |\n\
| `d` | [[cmd:git-diff]] | Diff current file |\n\
| `l` | [[cmd:git-log]] | Commit log |\n\
| `b` | [[cmd:git-blame]] | File blame |\n\n\
### Org-mode\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `TAB` | [[cmd:org-cycle]] | Three-state fold cycle |\n\
| `M-h` / `M-Left` | [[cmd:org-promote]] | Promote heading |\n\
| `M-l` / `M-Right` | [[cmd:org-demote]] | Demote heading |\n\
| `M-k` / `M-Up` | [[cmd:org-move-subtree-up]] | Move subtree up |\n\
| `M-j` / `M-Down` | [[cmd:org-move-subtree-down]] | Move subtree down |\n\
| `S-Left` | [[cmd:org-todo-prev]] | Prev TODO state |\n\
| `S-Right` | [[cmd:org-todo-next]] | Next TODO state |\n\
| `Enter` | [[cmd:org-open-link]] | Follow link |\n\n\
### SPC m — +mode (org)\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s n` | [[cmd:org-narrow-subtree]] | Narrow to subtree |\n\
| `s w` | [[cmd:org-widen]] | Widen (restore full buffer) |\n\n\
### Markdown\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `TAB` | [[cmd:md-cycle]] | Three-state fold cycle |\n\
| `M-h` / `M-Left` | [[cmd:md-promote]] | Promote heading |\n\
| `M-l` / `M-Right` | [[cmd:md-demote]] | Demote heading |\n\
| `M-k` / `M-Up` | [[cmd:md-move-subtree-up]] | Move subtree up |\n\
| `M-j` / `M-Down` | [[cmd:md-move-subtree-down]] | Move subtree down |\n\n\
### SPC m — +mode (markdown)\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s n` | [[cmd:md-narrow-subtree]] | Narrow to subtree |\n\
| `s w` | [[cmd:md-widen]] | Widen (restore full buffer) |\n\n\
### SPC h — +help\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `h` | [[cmd:help]] | Help index |\n\
| `k` | [[cmd:describe-key]] | Describe key |\n\
| `c` | [[cmd:describe-command]] | Describe command |\n\
| `s` | [[cmd:help-search]] | Search help |\n\
| `o` | [[cmd:describe-option]] | Describe option |\n\n\
### SPC d — +debug\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `d` | [[cmd:debug-start]] | Start debug |\n\
| `s` | [[cmd:debug-self]] | Self-debug |\n\
| `b` | [[cmd:debug-toggle-breakpoint]] | Toggle breakpoint |\n\
| `c` | [[cmd:debug-continue]] | Continue |\n\
| `p` | [[cmd:debug-panel]] | Debug panel |\n\
| `n` | [[cmd:debug-step-over]] | Step over |\n\
| `i` | [[cmd:debug-step-into]] | Step into |\n\
| `o` | [[cmd:debug-step-out]] | Step out |\n\n\
### SPC o — +open\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `t` | [[cmd:terminal]] | Open terminal |\n\
| `r` | [[cmd:terminal-reset]] | Reset terminal |\n\
| `c` | [[cmd:terminal-close]] | Close terminal |\n\n\
### SPC n — +notes\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `f` | [[cmd:kb-find]] | Search KB nodes |\n\n\
### SPC e — +eval\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `l` | [[cmd:eval-line]] | Eval line |\n\
| `b` | [[cmd:eval-buffer]] | Eval buffer |\n\
| `o` | [[cmd:open-scheme-repl]] | REPL |\n\n\
### SPC q — +quit\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `q` | [[cmd:quit]] | Quit |\n\
| `Q` | [[cmd:force-quit]] | Force quit |\n\n\
See also: [[key:normal-mode]], [[index]]\n";

const CONCEPT_PROJECT: &str =
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

const CONCEPT_TERMINAL: &str =
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

const CONCEPT_HOOKS: &str =
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

const CONCEPT_OPTIONS: &str =
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

const CONCEPT_AGENT_BOOTSTRAP: &str =
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

const CONCEPT_SELF_TEST: &str =
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

const CONCEPT_DEBUGGING: &str =
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

const CONCEPT_GUI: &str =
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

const CONCEPT_PACKAGE_SYSTEM: &str = "\
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

const CONCEPT_OPTION_REGISTRY: &str = "\
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

const CONCEPT_SCHEME_API: &str = "\
MAE exposes ~40 Scheme functions to extension code. They fall into categories:\n\n\
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

const CONCEPT_AI_MODES: &str = "\
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_produces_index_and_commands() {
        let reg = CommandRegistry::with_builtins();
        let kb = seed_kb_default(&reg);
        assert!(kb.contains("index"));
        // Every registered command becomes a node.
        for cmd in reg.list_commands() {
            let id = format!("cmd:{}", cmd.name);
            assert!(kb.contains(&id), "missing command node: {}", id);
        }
    }

    #[test]
    fn seed_includes_core_concepts() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        for required in [
            "concept:buffer",
            "concept:window",
            "concept:mode",
            "concept:command",
            "concept:ai-as-peer",
            "concept:knowledge-base",
            "concept:project",
            "concept:terminal",
            "concept:hooks",
            "concept:options",
            "concept:agent-bootstrap",
            "concept:self-test",
            "concept:debugging",
            "concept:gui",
            "concept:watchdog",
            "concept:event-recording",
            "concept:dap-attach",
            "concept:introspect",
            "concept:ai-modes",
            "key:leader-keys",
        ] {
            assert!(kb.contains(required), "missing concept: {}", required);
        }
    }

    #[test]
    fn index_links_to_concepts() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        let links = kb.links_from("index");
        assert!(links.contains(&"concept:buffer".to_string()));
        assert!(links.contains(&"concept:ai-as-peer".to_string()));
        assert!(links.contains(&"concept:gui".to_string()));
        assert!(links.contains(&"tutor:index".to_string()));
    }

    #[test]
    fn seed_includes_tutor_lessons() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        assert!(kb.contains("tutor:index"), "missing tutor:index");
        for i in [
            "lesson:navigation",
            "lesson:modes",
            "lesson:editing",
            "lesson:files",
            "lesson:ai",
            "lesson:scheme",
            "lesson:lsp",
            "lesson:terminal",
            "lesson:help",
            "lesson:leader",
            "lesson:debugging",
            "lesson:observability",
        ] {
            assert!(kb.contains(i), "missing lesson: {}", i);
        }
        // Tutor index links to all lessons
        let links = kb.links_from("tutor:index");
        assert!(links.contains(&"lesson:navigation".to_string()));
        assert!(links.contains(&"lesson:leader".to_string()));
    }

    #[test]
    fn command_node_body_has_source_and_backlinks() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        let node = kb.get("cmd:undo").expect("cmd:undo should exist");
        assert!(node.body.contains("built-in"));
        assert!(node.links().contains(&"index".to_string()));
    }

    #[test]
    fn concept_ai_as_peer_links_to_tools() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        let links = kb.links_from("concept:ai-as-peer");
        // A command referenced in the narrative should appear as a link
        // (the cmd:* targets exist because we generated them).
        assert!(links.iter().any(|l| l.starts_with("cmd:")));
        assert!(links.contains(&"concept:introspect".to_string()));
    }

    #[test]
    fn lesson_ai_has_expected_links() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        let links = kb.links_from("lesson:ai");
        assert!(links.contains(&"cmd:ai-prompt".to_string()));
        assert!(links.contains(&"cmd:open-ai-agent".to_string()));
        assert!(links.contains(&"cmd:ai-cancel".to_string()));
    }

    #[test]
    fn infer_category_known_prefixes() {
        assert_eq!(infer_category("move-left"), "movement");
        assert_eq!(infer_category("scroll-down"), "movement");
        assert_eq!(infer_category("delete-line"), "editing");
        assert_eq!(infer_category("undo"), "editing");
        assert_eq!(infer_category("git-status"), "git");
        assert_eq!(infer_category("lsp-hover"), "lsp");
        assert_eq!(infer_category("debug-start"), "debug");
        assert_eq!(infer_category("window-grow"), "window");
        assert_eq!(infer_category("file-tree-toggle"), "file-tree");
        assert_eq!(infer_category("ai-prompt"), "ai");
        assert_eq!(infer_category("help"), "help");
        assert_eq!(infer_category("toggle-fold"), "toggle");
        assert_eq!(infer_category("unknown-thing"), "general");
    }

    #[test]
    fn collect_keybindings_reverse_index() {
        use crate::keymap::{parse_key_seq, Keymap};
        let mut keymaps = HashMap::new();
        let mut normal = Keymap::new("normal");
        normal.bind(parse_key_seq("h"), "move-left");
        normal.bind(parse_key_seq("Left"), "move-left");
        keymaps.insert("normal".to_string(), normal);

        let map = collect_keybindings(&keymaps);
        let bindings = map.get("move-left").unwrap();
        assert!(bindings.len() >= 2);
        assert!(bindings.iter().any(|(m, k)| m == "normal" && k == "h"));
    }

    #[test]
    fn collect_keybindings_for_single_command() {
        use crate::keymap::{parse_key_seq, Keymap};
        let mut keymaps = HashMap::new();
        let mut normal = Keymap::new("normal");
        normal.bind(parse_key_seq("h"), "move-left");
        normal.bind(parse_key_seq("j"), "move-down");
        keymaps.insert("normal".to_string(), normal);

        let bindings = collect_keybindings_for(&keymaps, "move-left");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0], ("normal".to_string(), "h".to_string()));
    }

    #[test]
    fn seed_kb_with_keymaps_has_categories() {
        use crate::keymap::{parse_key_seq, Keymap};
        let reg = CommandRegistry::with_builtins();
        let mut keymaps = HashMap::new();
        let mut normal = Keymap::new("normal");
        normal.bind(parse_key_seq("h"), "move-left");
        keymaps.insert("normal".to_string(), normal);
        let hooks = HookRegistry::new();
        let kb = seed_kb(&reg, &keymaps, &hooks);

        // Category nodes should exist
        assert!(
            kb.contains("category:movement"),
            "should have movement category"
        );
        assert!(
            kb.contains("category:editing"),
            "should have editing category"
        );

        // Command nodes should have category info
        let node = kb.get("cmd:move-left").unwrap();
        assert!(node.body.contains("**Category:** movement"));
    }

    #[test]
    fn enriched_cmd_node_has_keybindings() {
        use crate::keymap::{parse_key_seq, Keymap};
        let reg = CommandRegistry::with_builtins();
        let mut keymaps = HashMap::new();
        let mut normal = Keymap::new("normal");
        normal.bind(parse_key_seq("h"), "move-left");
        keymaps.insert("normal".to_string(), normal);
        let hooks = HookRegistry::new();
        let kb = seed_kb(&reg, &keymaps, &hooks);

        let node = kb.get("cmd:move-left").unwrap();
        assert!(
            node.body.contains("**Keybindings:**"),
            "should have keybinding section"
        );
        assert!(
            node.body.contains("normal: `h`"),
            "should show normal mode h binding"
        );
    }

    // --- KB Health Tests ---

    #[test]
    fn kb_health_all_cmd_nodes_have_category() {
        let reg = CommandRegistry::with_builtins();
        let kb = seed_kb_default(&reg);
        for cmd in reg.list_commands() {
            let id = format!("cmd:{}", cmd.name);
            let node = kb.get(&id).unwrap_or_else(|| panic!("missing {}", id));
            assert!(
                node.body.contains("**Category:**"),
                "{} missing category",
                id
            );
        }
    }

    #[test]
    fn kb_health_all_category_index_nodes_exist() {
        let reg = CommandRegistry::with_builtins();
        let kb = seed_kb_default(&reg);
        let mut categories = std::collections::HashSet::new();
        for cmd in reg.list_commands() {
            categories.insert(infer_category(&cmd.name));
        }
        for cat in categories {
            let id = format!("category:{}", cat);
            assert!(kb.contains(&id), "missing category node: {}", id);
        }
    }

    #[test]
    fn kb_health_all_category_links_resolve() {
        let reg = CommandRegistry::with_builtins();
        let kb = seed_kb_default(&reg);
        for id in kb.list_ids(None) {
            if id.starts_with("category:") {
                let links = kb.links_from(&id);
                for link in &links {
                    assert!(kb.contains(link), "broken link {} -> {}", id, link);
                }
            }
        }
    }

    #[test]
    fn kb_health_no_orphaned_cmd_nodes() {
        let reg = CommandRegistry::with_builtins();
        let kb = seed_kb_default(&reg);
        for cmd in reg.list_commands() {
            let id = format!("cmd:{}", cmd.name);
            let cat = infer_category(&cmd.name);
            let cat_id = format!("category:{}", cat);
            let links = kb.links_from(&cat_id);
            assert!(
                links.contains(&id),
                "cmd {} not linked from category {}",
                id,
                cat_id
            );
        }
    }

    #[test]
    fn kb_health_coverage_summary() {
        let reg = CommandRegistry::with_builtins();
        let kb = seed_kb_default(&reg);
        let all_ids = kb.list_ids(None);
        let cmd_count = all_ids.iter().filter(|id| id.starts_with("cmd:")).count();
        let concept_count = all_ids
            .iter()
            .filter(|id| id.starts_with("concept:"))
            .count();
        let category_count = all_ids
            .iter()
            .filter(|id| id.starts_with("category:"))
            .count();
        assert!(all_ids.len() >= 100, "total nodes: {} < 100", all_ids.len());
        assert!(cmd_count >= 50, "cmd nodes: {} < 50", cmd_count);
        assert!(concept_count >= 10, "concept nodes: {} < 10", concept_count);
        assert!(
            category_count >= 5,
            "category nodes: {} < 5",
            category_count
        );
    }

    // --- Round 1: Scheme nodes + Tutorial ---

    #[test]
    fn scheme_nodes_exist() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        // Check a representative set of scheme function nodes
        for name in [
            "scheme:buffer-insert",
            "scheme:cursor-goto",
            "scheme:define-key",
            "scheme:set-option!",
            "scheme:read-file",
            "scheme:add-hook!",
            "scheme:provide-feature",
            "scheme:shell-send-input",
        ] {
            assert!(kb.contains(name), "missing scheme node: {}", name);
        }
        // Check variable nodes
        for name in [
            "scheme:*buffer-name*",
            "scheme:*cursor-row*",
            "scheme:*mode*",
            "scheme:*buffer-list*",
        ] {
            assert!(kb.contains(name), "missing scheme variable node: {}", name);
        }
    }

    #[test]
    fn scheme_nodes_link_to_concept() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        let links = kb.links_from("scheme:buffer-insert");
        assert!(
            links.contains(&"concept:scheme-api".to_string()),
            "scheme node should link to concept:scheme-api"
        );
    }

    #[test]
    fn tutorial_hub_exists() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        assert!(
            kb.contains("tutorial:getting-started"),
            "missing tutorial hub"
        );
    }

    #[test]
    fn tutorial_vim_track_nodes_exist() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        for id in [
            "tutorial:vim-familiar",
            "tutorial:vim-differences",
            "tutorial:mae-navigation",
            "tutorial:mae-extending",
        ] {
            assert!(kb.contains(id), "missing vim track node: {}", id);
        }
    }

    #[test]
    fn tutorial_beginner_track_nodes_exist() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        for id in [
            "tutorial:what-is-modal",
            "tutorial:basic-movement",
            "tutorial:basic-editing",
            "tutorial:mae-navigation",
            "tutorial:mae-extending",
        ] {
            assert!(kb.contains(id), "missing beginner track node: {}", id);
        }
    }

    #[test]
    fn tutorial_ai_track_nodes_exist() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        for id in ["tutorial:ai-setup", "tutorial:ai-agent", "tutorial:ai-chat"] {
            assert!(kb.contains(id), "missing AI track node: {}", id);
        }
    }

    #[test]
    fn tutorial_shared_nodes_linked_from_both_tracks() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        // Vim track links to mae-navigation
        let vim_links = kb.links_from("tutorial:vim-differences");
        assert!(
            vim_links.contains(&"tutorial:mae-navigation".to_string()),
            "vim track should link to mae-navigation"
        );
        // Beginner track links to mae-navigation
        let beginner_links = kb.links_from("tutorial:basic-editing");
        assert!(
            beginner_links.contains(&"tutorial:mae-navigation".to_string()),
            "beginner track should link to mae-navigation"
        );
    }

    #[test]
    fn index_links_to_getting_started() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        let links = kb.links_from("index");
        assert!(
            links.contains(&"tutorial:getting-started".to_string()),
            "index should link to tutorial:getting-started"
        );
    }

    #[test]
    fn help_edit_command_registered() {
        let reg = CommandRegistry::with_builtins();
        assert!(
            reg.get("help-edit").is_some(),
            "help-edit command should be registered"
        );
    }

    #[test]
    fn help_namespace_fallback_finds_scheme_nodes() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        // The :help handler tries scheme:X as a candidate
        let candidates = [
            "buffer-insert".to_string(),
            format!("cmd:{}", "buffer-insert"),
            format!("concept:{}", "buffer-insert"),
            format!("scheme:{}", "buffer-insert"),
        ];
        let found = candidates.iter().find(|id| kb.contains(id));
        assert_eq!(found, Some(&"scheme:buffer-insert".to_string()));
    }

    #[test]
    fn help_namespace_fallback_finds_option_nodes() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        let candidates = [
            "line_numbers".to_string(),
            format!("cmd:{}", "line_numbers"),
            format!("concept:{}", "line_numbers"),
            format!("scheme:{}", "line_numbers"),
            format!("option:{}", "line_numbers"),
        ];
        let found = candidates.iter().find(|id| kb.contains(id));
        assert_eq!(found, Some(&"option:line_numbers".to_string()));
    }
}
