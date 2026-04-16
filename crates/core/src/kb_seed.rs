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

## Reference
- [[key:normal-mode|Normal-mode keys]]
- Commands: run `:command-list` for the full list, or visit any `cmd:<name>` node.

## Getting around
- **Enter** on a link follows it.
- **C-o** goes back, **C-i** goes forward (history, like vim jumps).
- **q** closes the help buffer.
";

const CONCEPT_BUFFER: &str = "A **buffer** is the unit of editable content in MAE.\n\
It has an optional file path, a kind ([[concept:buffer-kind|BufferKind]]), modification \
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
MAE's tiling [[concept:window-manager|WindowManager]] owns the layout tree (splits, sizes) \
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
- **FilePicker** — fuzzy file open overlay.\n\n\
Mode transitions are commands — see [[cmd:enter-normal-mode]], [[cmd:enter-insert-mode]], \
[[cmd:enter-command-mode]]. The AI agent can trigger them too (that's the point of [[concept:ai-as-peer]]).\n\n\
See also: [[key:normal-mode]]\n";

const CONCEPT_COMMAND: &str =
    "A **command** is a named, documented operation with a stable string identifier. \
Commands are registered in a shared [[concept:command-registry|CommandRegistry]] and can \
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
- This knowledge base (`kb_get`, `kb_search`, `kb_list`, `kb_links_from`, `kb_links_to`).\n\n\
## Permission tiers\n\
Every tool has a [[concept:permission-tier|permission tier]]: ReadOnly, Write, Shell, \
Privileged. Users control how far the agent can act autonomously.\n\n\
See also: [[concept:knowledge-base]], [[concept:command]]\n";

const CONCEPT_KB: &str =
    "MAE's **knowledge base** is a typed graph of [[concept:kb-node|nodes]] with \
bidirectional [[link]] markers. It started as the help system's backing store and is \
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
### Editing\n\
- `i` / `a` — enter insert mode (before / after cursor) ([[cmd:enter-insert-mode]])\n\
- `o` / `O` — open line below / above ([[cmd:open-line-below]])\n\
- `dd` — delete line ([[cmd:delete-line]])\n\
- `yy` — yank line\n\
- `u` / `C-r` — undo / redo ([[cmd:undo]], [[cmd:redo]])\n\n\
### Windows, buffers, files\n\
- `:e <path>` — open file\n\
- `:ls` — list buffers ([[cmd:list-buffers]])\n\
- `C-^` — switch to alternate buffer\n\n\
### Help\n\
- `:help` — open this page\n\
- `:describe-command <name>` — show docs for any command\n\n\
See also: [[index]], [[concept:mode]]\n";

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
