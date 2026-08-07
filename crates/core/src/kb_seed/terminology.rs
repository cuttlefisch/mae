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
             The kernel defines only vi-modal primitives per mode (normal, insert, \
             visual, command) plus buffer-kind overlays (help, git-status, file-tree, \
             org, markdown) — it binds NO SPC leader keys. The shared `leader` keymap \
             is the single source of truth for the which-key menu; flavor modules \
             (`doom` = modal default, `nonmodal` = CUA) wire an entry into it. Switch \
             flavors live with `:keymap-set-flavor <name>`. Scheme API: \
             `(define-key MAP KEY CMD)`, `(set-group-name MAP PREFIX LABEL)`, \
             `(define-keymap NAME PARENT)`, `(undefine-key! MAP KEY)`.",
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
            "The entry into the transient which-key keypad, which resolves keys \
             against the shared `leader` keymap (the single source of truth). The \
             entry binding is flavor-specific: the default `doom` flavor binds SPC in \
             normal mode (Doom Emacs style — SPC f f = find file, SPC b s = switch \
             buffer, SPC h = help, SPC a = AI agent), while `nonmodal` (CUA) binds \
             `C-;` in insert mode. Leader bindings live in the `leader` keymap and \
             appear in every flavor's keypad.",
        ),
        (
            "term:provider",
            "Provider",
            "An AI API backend: Claude, OpenAI, Gemini, or DeepSeek. Selected at startup \
             via the `[ai]` bootstrap in config.toml (the narrow legacy bootstrap that \
             carries provider/model + API key, e.g. `api_key_command`), or via an \
             environment API key. The provider determines the model, API format, and \
             capabilities available to the AI agent.",
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
             Shell (execute shell commands), Privileged (all operations). Set it with the \
             `MAE_AI_PERMISSIONS` env var, or the `[ai] auto_approve_tier` config.toml key. \
             Values are case-insensitive (`PermissionTier::parse` lowercases its input), and \
             an unrecognised value is REFUSED — MAE fails to start rather than resolving it \
             to any tier. \
             NOTE: the `ai_tier` OptionRegistry option currently updates only the status-bar \
             badge and does NOT change the enforced policy, and the tier is not enforced on \
             every path — see SECURITY.md. Treat it as a guardrail against accident, not as \
             a boundary against an adversarial or prompt-injected model.",
        ),
    ];

    for (id, title, body) in terms {
        let node = Node::new(*id, *title, NodeKind::Concept, *body).with_tags(["terminology"]);
        kb.insert(node);
    }
}

#[cfg(test)]
mod tier_terminology_accuracy_tests {
    use super::*;

    /// Find a seeded node's body by id, so these tests read the shipped text
    /// rather than a copy of it.
    fn body_of(id: &str) -> String {
        let mut kb = KnowledgeBase::new();
        install_terminology_nodes(&mut kb);
        kb.get(id)
            .unwrap_or_else(|| panic!("{id} must be seeded"))
            .body
            .clone()
    }

    /// `term:tier` is delivered to AI sessions as authoritative guidance about a
    /// SECURITY control, so a false claim in it is worse than no claim: it primes
    /// every session with a wrong mental model of what the tier gate does.
    ///
    /// Until 2026-08 this node stated that "an unrecognised value currently falls
    /// back to shell". That was true of an older build and was fixed in
    /// `config.rs` — which now errors with "unknown AI permission tier" and has a
    /// test asserting an unknown tier "must be rejected, never resolved". The
    /// prose kept describing the former BUG as current behaviour, telling agents
    /// that a typo in `auto_approve_tier` silently grants shell. Exactly
    /// backwards, and in the least safe direction.
    ///
    /// The assertion is on the dangerous CLAIM, not on the current wording, so
    /// rephrasing the node freely is fine and only reintroducing the falsehood
    /// fails.
    #[test]
    fn tier_node_does_not_claim_an_unknown_value_falls_back_to_a_tier() {
        let body = body_of("term:tier").to_ascii_lowercase();
        assert!(
            !body.contains("falls back to shell") && !body.contains("fall back to shell"),
            "term:tier must not claim an unrecognised tier falls back to shell — \
             config.rs refuses to start on an unknown tier. Body: {body}"
        );
    }

    /// The other half, and the reason the first assertion is not enough on its
    /// own: a node could satisfy it by saying nothing at all about unknown
    /// values, leaving the reader to assume a silent default. The refusal is the
    /// safety-relevant fact and must be stated.
    #[test]
    fn tier_node_states_that_an_unknown_value_is_refused() {
        let body = body_of("term:tier").to_ascii_lowercase();
        assert!(
            body.contains("refused") || body.contains("fails to start"),
            "term:tier must state that an unrecognised tier is refused, not merely \
             omit the question — silence reads as 'there is some default'. Body: {body}"
        );
    }

    /// `PermissionTier::parse` lowercases its input (`crates/ai/src/types.rs`), so
    /// the old "lowercase values only" instruction was wrong too. Harmless next to
    /// the fallback claim, but it is the same drift and would have someone
    /// "fixing" a working config.
    #[test]
    fn tier_node_does_not_claim_lowercase_only() {
        let body = body_of("term:tier").to_ascii_lowercase();
        assert!(
            !body.contains("lowercase values only"),
            "tier parsing is case-insensitive; do not tell users otherwise. Body: {body}"
        );
    }
}
