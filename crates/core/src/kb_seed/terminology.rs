//! Terminology KB nodes — canonical vocabulary definitions for MAE.
//!
//! Each term gets a `term:<name>` node. AI agents can `kb_get "term:buffer"`
//! to get the precise definition. These are the single source of truth for
//! MAE vocabulary.

use mae_kb::{KnowledgeBase, Node, NodeKind};

/// Install all `term:*` vocabulary nodes into the KB.
pub fn install_terminology_nodes(kb: &mut KnowledgeBase) {
    let terms: &[(&str, &str, &str)] = &[
        (
            "term:buffer",
            "Buffer",
            "In-memory text content. May have a backing file (file buffer) or not \
             (scratch, conversation, help). Buffers are the fundamental unit of text \
             in MAE — every window displays exactly one buffer.",
        ),
        (
            "term:window",
            "Window",
            "A visible pane showing a buffer. Windows are arranged in a binary split \
             tree (the layout). Multiple windows can show the same buffer.",
        ),
        (
            "term:layout",
            "Layout",
            "The tree of window splits. Horizontal and vertical splits form a binary \
             tree. The layout determines how screen space is divided among windows.",
        ),
        (
            "term:mode",
            "Mode",
            "Current editing state: Normal, Insert, Visual (char/line/block), Command, \
             Search, ShellInsert, FilePicker, FileBrowser, CommandPalette, \
             ConversationInput. Not to be confused with Emacs major-modes — MAE modes \
             are vi-style modal states.",
        ),
        (
            "term:command",
            "Command",
            "A named, dispatchable action registered in the CommandRegistry. Commands \
             can be bound to keys, called from Scheme, or invoked by the AI agent. \
             Every command has a name, doc string, and source (Builtin, Scheme, Autoload).",
        ),
        (
            "term:keybinding",
            "Keybinding",
            "A key sequence mapped to a command within a keymap. Key sequences can be \
             single keys (j) or multi-key chords (SPC f f). Bindings are looked up \
             in the active keymap for the current mode.",
        ),
        (
            "term:keymap",
            "Keymap",
            "A named set of keybindings with an optional parent for fallback lookup. \
             MAE has keymaps for each mode (normal, insert, visual, command) plus \
             buffer-kind overlays (help, git-status, file-tree, org, markdown).",
        ),
        (
            "term:hook",
            "Hook",
            "A named event that functions can subscribe to via `(add-hook!)`. When the \
             hook fires, all registered functions execute in registration order. Hooks \
             are composition points — no advice chaining.",
        ),
        (
            "term:option",
            "Option",
            "A named, typed configuration value registered in the OptionRegistry. \
             Options have a name, type (Bool/Int/String/Float/Theme), default value, \
             and doc string. Changed via `:set` and persisted via `:set-save`.",
        ),
        (
            "term:module",
            "Module",
            "A self-contained package of commands, keybindings, hooks, and options. \
             Modules have a `module.toml` manifest, `autoloads.scm` (eager registration), \
             and `init.scm` (lazy initialization). Modules are Scheme-only packages; \
             Rust-level changes go through the kernel.",
        ),
        (
            "term:feature",
            "Feature",
            "A named capability provided by a module via `(provide-feature name)`. \
             Other code can check availability with `(require-feature name)` or \
             `(feature-loaded? name)`. Features enable lazy loading — the module's \
             init.scm runs on first `require-feature` call.",
        ),
        (
            "term:flag",
            "Flag",
            "An optional sub-feature of a module, enabled with `+name` syntax in the \
             `mae!` block. Flags are declared in `module.toml` and checked in Scheme \
             with `(when-flag \"+name\" thunk)`. Example: `(org +agenda +babel)`.",
        ),
        (
            "term:manifest",
            "Manifest",
            "`module.toml` — TOML metadata describing a module's identity, version, \
             dependencies, flags, and entry points. Parseable before the Scheme runtime \
             starts, enabling offline introspection (`mae pkg list`).",
        ),
        (
            "term:autoload",
            "Autoload",
            "A command stub that loads its module on first use. Registered via \
             `(autoload cmd-name feature doc)` in `autoloads.scm`. When the command \
             is invoked, MAE loads the feature's `init.scm`, then re-dispatches.",
        ),
        (
            "term:kernel",
            "Kernel",
            "MAE's Rust core — buffer management, window layout, modal editing, event \
             loop, rendering, Scheme runtime, syntax highlighting. The kernel provides \
             primitives that modules compose. If it needs tokio, PTY, or FFI, it's kernel.",
        ),
        (
            "term:display-region",
            "Display Region",
            "A virtual overlay on buffer text that modifies how a range of lines is \
             displayed. Used for folds (hiding lines), link concealment (showing display \
             text instead of raw markup), and code block backgrounds.",
        ),
        (
            "term:leader-key",
            "Leader Key",
            "SPC in normal mode — the prefix key for command sequences. Inspired by \
             Doom Emacs. SPC f f = find file, SPC b s = switch buffer, SPC h = help, \
             SPC a = AI agent, etc.",
        ),
        (
            "term:provider",
            "Provider",
            "An AI API backend: Claude, OpenAI, Gemini, or DeepSeek. Configured in \
             config.toml under `[ai]`. The provider determines the model, API format, \
             and capabilities available to the AI agent.",
        ),
        (
            "term:profile",
            "Profile",
            "An AI agent personality. Built-in profiles: pair-programmer (default), \
             explorer, reviewer. Profiles determine the system prompt, tool preferences, \
             and behavioral guardrails. Set via `:set ai_profile <name>`.",
        ),
        (
            "term:tier",
            "Tier",
            "AI permission level controlling what actions the agent can take. Tiers: \
             ReadOnly (read buffers, navigate), Write (edit buffers, create files), \
             Shell (execute shell commands), Privileged (all operations). Set via \
             config.toml or `MAE_AI_PERMISSIONS` env var.",
        ),
    ];

    for (id, title, body) in terms {
        let node = Node::new(*id, *title, NodeKind::Concept, *body).with_tags(["terminology"]);
        kb.insert(node);
    }
}
