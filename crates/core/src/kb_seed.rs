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

use mae_kb::{KnowledgeBase, Node, NodeKind};

use crate::commands::CommandRegistry;

/// Build the initial KB: hand-authored concept/index nodes + generated
/// `cmd:*` nodes derived from the command registry.
pub fn seed_kb(registry: &CommandRegistry) -> KnowledgeBase {
    let mut kb = KnowledgeBase::new();
    install_static_nodes(&mut kb);
    install_command_nodes(&mut kb, registry);
    kb
}

/// Install the hand-authored index + concept + key nodes.
fn install_static_nodes(kb: &mut KnowledgeBase) {
    for node in static_nodes() {
        kb.insert(node);
    }
}

/// Install a `cmd:<name>` node for every registered command. Source
/// (builtin vs scheme) is surfaced in the body so users can tell which
/// commands are implemented in Rust vs Scheme.
fn install_command_nodes(kb: &mut KnowledgeBase, registry: &CommandRegistry) {
    for cmd in registry.list_commands() {
        let source_line = match &cmd.source {
            crate::commands::CommandSource::Builtin => "**Source:** built-in (Rust)".to_string(),
            crate::commands::CommandSource::Scheme(fn_name) => {
                format!("**Source:** Scheme (`{}`)", fn_name)
            }
        };
        let body = format!(
            "{doc}\n\n{source_line}\n\nSee also: [[index]], [[concept:command]], [[concept:ai-as-peer]]",
            doc = cmd.doc,
            source_line = source_line,
        );
        let id = format!("cmd:{}", cmd.name);
        let title = format!("Command: {}", cmd.name);
        kb.insert(Node::new(id, title, NodeKind::Command, body));
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
    ]
}

const INDEX_BODY: &str = "Welcome to MAE's built-in help. This knowledge base is the same data \
surface the AI agent queries via its `kb_*` tools — you and the AI read the same pages.

## Core concepts
- [[concept:buffer|Buffer]] — the unit of editable content
- [[concept:window|Window]] — a view onto a buffer
- [[concept:mode|Mode]] — which keymap is active
- [[concept:command|Command]] — the shared API between human, Scheme, and AI
- [[concept:ai-as-peer|The AI as Peer Actor]] — the fundamental design stance
- [[concept:knowledge-base|Knowledge Base]] — this page, and why it exists
- [[concept:terminal|Embedded Terminal]] — full terminal emulator inside MAE
- [[concept:hooks|Hooks]] — Scheme extension points for editor events
- [[concept:options|Editor Options]] — configuring MAE from Scheme

## Reference
- [[key:normal-mode|Normal-mode keys]]
- [[key:leader-keys|SPC leader bindings]] (14 groups, Doom Emacs style)
- [[concept:project|Project management]]
- Commands: run `:command-list` for the full list, or visit any `cmd:<name>` node.

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
See also: [[concept:window]], [[concept:command]], [[cmd:list-buffers]]\n";

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
- This knowledge base (`kb_get`, `kb_search`, `kb_list`, `kb_links_from`, `kb_links_to`).\n\
- [[concept:project|Project state]] via `project_info`, `project_files`, `project_search`.\n\n\
## Permission tiers\n\
Every tool has a permission tier: ReadOnly, Write, Shell, \
Privileged. Users control how far the agent can act autonomously.\n\n\
See also: [[concept:knowledge-base]], [[concept:command]]\n";

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
| `S` | [[cmd:save-as]] | Save as |\n\n\
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
| `w` | [[cmd:toggle-word-wrap]] | Word wrap |\n\n\
### SPC a — +ai\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `a` | [[cmd:ai-prompt]] | AI prompt |\n\
| `c` | [[cmd:ai-cancel]] | Cancel AI |\n\n\
### SPC h — +help\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `h` | [[cmd:help]] | Help index |\n\
| `k` | [[cmd:describe-key]] | Describe key |\n\
| `c` | [[cmd:describe-command]] | Describe command |\n\
| `s` | [[cmd:help-search]] | Search help |\n\n\
### SPC d — +debug\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `d` | [[cmd:debug-start]] | Start debug |\n\
| `s` | [[cmd:debug-self]] | Self-debug |\n\
| `b` | [[cmd:debug-toggle-breakpoint]] | Toggle breakpoint |\n\
| `c` | [[cmd:debug-continue]] | Continue |\n\
| `n/i/o` | step over/in/out | Step |\n\n\
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
- [[cmd:terminal-close]] (`SPC o c`) — close the terminal and kill the shell process.\n\n\
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
| `command-post` | After a command completes (planned) |\n\n\
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
| `theme` | theme name | Set the color theme |\n\n\
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_produces_index_and_commands() {
        let reg = CommandRegistry::with_builtins();
        let kb = seed_kb(&reg);
        assert!(kb.contains("index"));
        // Every registered command becomes a node.
        for cmd in reg.list_commands() {
            let id = format!("cmd:{}", cmd.name);
            assert!(kb.contains(&id), "missing command node: {}", id);
        }
    }

    #[test]
    fn seed_includes_core_concepts() {
        let kb = seed_kb(&CommandRegistry::with_builtins());
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
            "key:leader-keys",
        ] {
            assert!(kb.contains(required), "missing concept: {}", required);
        }
    }

    #[test]
    fn index_links_to_concepts() {
        let kb = seed_kb(&CommandRegistry::with_builtins());
        let links = kb.links_from("index");
        assert!(links.contains(&"concept:buffer".to_string()));
        assert!(links.contains(&"concept:ai-as-peer".to_string()));
    }

    #[test]
    fn command_node_body_has_source_and_backlinks() {
        let kb = seed_kb(&CommandRegistry::with_builtins());
        let node = kb.get("cmd:undo").expect("cmd:undo should exist");
        assert!(node.body.contains("built-in"));
        assert!(node.links().contains(&"index".to_string()));
    }

    #[test]
    fn concept_ai_as_peer_links_to_tools() {
        let kb = seed_kb(&CommandRegistry::with_builtins());
        let links = kb.links_from("concept:ai-as-peer");
        // A command referenced in the narrative should appear as a link
        // (the cmd:* targets exist because we generated them).
        assert!(links.iter().any(|l| l.starts_with("cmd:")));
    }
}
