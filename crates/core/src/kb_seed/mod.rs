//! Seed the knowledge base with built-in manual content.
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
//! regenerating the KB on every startup keeps manual entries and commands in
//! lockstep with the code that ships.

mod concepts;
mod keys;
mod lessons;
pub mod modules;
mod scheme_api;
mod terminology;
mod tutorials;

use std::collections::HashMap;

use mae_kb::{KnowledgeBase, Node, NodeKind};

use crate::commands::CommandRegistry;
use crate::hooks::HookRegistry;
use crate::keymap::{serialize_macro, Keymap};
use crate::options::OptionRegistry;

use concepts::*;
use keys::*;
use lessons::*;
use scheme_api::install_scheme_nodes;
use terminology::install_terminology_nodes;
use tutorials::install_tutorial_nodes;

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
    install_terminology_nodes(&mut kb);
    let keybinding_map = collect_keybindings(keymaps);
    install_command_nodes(&mut kb, registry, &keybinding_map, hooks);
    install_category_nodes(&mut kb, registry, &keybinding_map);
    install_option_nodes(&mut kb);
    // Stamp all nodes seeded so far with provenance metadata.
    // This enables versioned re-seeding: "update all Seed nodes where source_version < N".
    stamp_seed_provenance(&mut kb);
    // User help nodes are added last — they are UserOrg, not Seed.
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

/// Mark all existing nodes as `Seed` source with version 1.
/// Called after all built-in nodes are inserted but before user help nodes.
fn stamp_seed_provenance(kb: &mut KnowledgeBase) {
    kb.stamp_source(mae_kb::NodeSource::Seed, 1);
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
            NodeKind::Tutorial,
            TUTOR_INDEX,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:navigation",
            "Lesson 1: Navigation",
            NodeKind::Lesson,
            LESSON_NAVIGATION,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:modes",
            "Lesson 2: Modes",
            NodeKind::Lesson,
            LESSON_MODES,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:editing",
            "Lesson 3: Editing",
            NodeKind::Lesson,
            LESSON_EDITING,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:files",
            "Lesson 4: Files & Buffers",
            NodeKind::Lesson,
            LESSON_FILES,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:ai",
            "Lesson 5: AI Features",
            NodeKind::Lesson,
            LESSON_AI,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:scheme",
            "Lesson 6: Scheme REPL",
            NodeKind::Lesson,
            LESSON_SCHEME,
        )
        .with_tags(["tutorial"]),
        Node::new("lesson:lsp", "Lesson 7: LSP", NodeKind::Lesson, LESSON_LSP)
            .with_tags(["tutorial"]),
        Node::new(
            "lesson:terminal",
            "Lesson 8: Terminal",
            NodeKind::Lesson,
            LESSON_TERMINAL,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:help",
            "Lesson 9: Help System",
            NodeKind::Lesson,
            LESSON_HELP,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:leader",
            "Lesson 10: Leader Keys",
            NodeKind::Lesson,
            LESSON_LEADER,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:debugging",
            "Lesson 11: Debugging",
            NodeKind::Lesson,
            LESSON_DEBUGGING,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:observability",
            "Lesson 12: Observability",
            NodeKind::Lesson,
            LESSON_OBSERVABILITY,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "lesson:kb-import-roam",
            "Lesson 13: Importing Your Knowledge Base",
            NodeKind::Lesson,
            LESSON_KB_IMPORT,
        )
        .with_tags(["tutorial", "kb", "federation", "org-roam"]),
        Node::new(
            "lesson:collab-setup",
            "Setting Up Collaborative Editing",
            NodeKind::Lesson,
            LESSON_COLLAB_SETUP,
        )
        .with_tags(["tutorial", "collaboration", "daemon", "sync"]),
    ]
}

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
        kb.insert(Node::new(id, title, NodeKind::Category, body).with_tags(["category"]));
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
        let config_line = match def.config_key.as_deref() {
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
             :set-save {name} <value>  \" set and persist to init.scm\n\
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
            scheme_name = def
                .aliases
                .first()
                .map(|c| c.as_ref())
                .unwrap_or(def.name.as_ref()),
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
        .with_tags(["data-model", "core"])
        .with_aliases(["file", "tab", "document"]),
        Node::new(
            "concept:window",
            "Concept: Window",
            NodeKind::Concept,
            CONCEPT_WINDOW,
        )
        .with_tags(["data-model", "core"])
        .with_aliases(["pane", "split", "panel"]),
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
        .with_tags(["ai", "architecture"])
        .with_aliases(["copilot", "assistant", "llm", "chatbot"]),
        Node::new(
            "concept:knowledge-base",
            "Concept: Knowledge Base",
            NodeKind::Concept,
            CONCEPT_KB,
        )
        .with_tags(["kb", "architecture"]),
        Node::new(
            "concept:kb-federation",
            "Concept: KB Federation",
            NodeKind::Concept,
            CONCEPT_KB_FEDERATION,
        )
        .with_tags(["kb", "federation", "org-roam"])
        .with_aliases(["federation", "kb-register", "org-roam import"]),
        Node::new(
            "concept:kb-workflows",
            "Concept: KB Workflows",
            NodeKind::Concept,
            CONCEPT_KB_WORKFLOWS,
        )
        .with_tags(["kb", "workflow", "help"])
        .with_aliases(["kb usage", "help authoring", "backup"]),
        Node::new(
            "concept:kb-vs-alternatives",
            "Concept: KB vs Obsidian / Roam Research",
            NodeKind::Concept,
            CONCEPT_KB_ALTERNATIVES,
        )
        .with_tags(["kb", "comparison", "obsidian", "roam"])
        .with_aliases(["obsidian", "roam research", "notion", "logseq"]),
        Node::new(
            "concept:dailies",
            "Concept: Org-Dailies",
            NodeKind::Concept,
            CONCEPT_DAILIES,
        )
        .with_tags(["kb", "dailies", "journal", "org-roam"])
        .with_aliases(["daily notes", "journal", "org-roam-dailies"]),
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
        .with_tags(["terminal", "shell", "phase-6"])
        .with_aliases(["console", "command-line", "bash"]),
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
        .with_tags(["dap", "debugging", "ai"])
        .with_aliases(["debugger", "breakpoints", "stepping"]),
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
            "concept:render-profiling",
            "Concept: Render Profiling",
            NodeKind::Concept,
            CONCEPT_RENDER_PROFILING,
        )
        .with_tags(["performance", "rendering", "observability"])
        .with_aliases(["display", "graphics", "skia"]),
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
        .with_tags(["extensibility", "scheme", "api"])
        .with_aliases(["lisp", "scripting", "extension-api", "elisp"]),
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
        Node::new(
            "concept:modules",
            "Concept: Module System",
            NodeKind::Concept,
            CONCEPT_MODULES,
        )
        .with_tags(["modules", "extensibility", "packages"])
        .with_aliases(["plugins", "packages", "extensions", "addons"]),
        Node::new(
            "concept:flags",
            "Concept: Module Flags",
            NodeKind::Concept,
            CONCEPT_FLAGS,
        )
        .with_tags(["modules", "flags", "configuration"]),
        Node::new(
            "concept:design-philosophy",
            "Concept: Design Philosophy",
            NodeKind::Concept,
            CONCEPT_DESIGN_PHILOSOPHY,
        )
        .with_tags(["modules", "architecture", "extensibility"]),
        Node::new(
            "guide:extension-authoring",
            "Guide: Extension Authoring",
            NodeKind::Concept,
            GUIDE_EXTENSION_AUTHORING,
        )
        .with_tags(["modules", "guide", "extensibility"]),
        Node::new(
            "concept:sync-engine",
            "Concept: Sync Engine (yrs)",
            NodeKind::Concept,
            CONCEPT_SYNC_ENGINE,
        )
        .with_tags(["architecture", "sync", "crdt"])
        .with_aliases(["yrs", "yjs", "crdt", "collaboration"]),
        Node::new(
            "concept:collaborative-state",
            "Concept: Collaborative State Engine",
            NodeKind::Concept,
            CONCEPT_COLLABORATIVE_STATE,
        )
        .with_tags(["architecture", "sync", "vision"])
        .with_aliases(["collab", "multiplayer", "real-time"]),
        Node::new(
            "concept:adr-text-sync",
            "ADR-002: Text Sync (Accepted)",
            NodeKind::Concept,
            CONCEPT_ADR_TEXT_SYNC,
        )
        .with_tags(["adr", "sync", "architecture"]),
        Node::new(
            "concept:adr-kb-crdt",
            "ADR-005: KB as CRDT",
            NodeKind::Concept,
            CONCEPT_ADR_KB_CRDT,
        )
        .with_tags(["adr", "kb", "sync", "architecture"]),
        Node::new(
            "concept:adr-keymap-resolution",
            "ADR-015: Keymap Resolution Chain (Accepted)",
            NodeKind::Concept,
            CONCEPT_ADR_KEYMAP_RESOLUTION,
        )
        .with_tags(["adr", "keymap", "architecture"])
        .with_aliases(["keymap chain", "navigation mode", "keymap registry"]),
        Node::new(
            "concept:adr-artifact-interaction",
            "ADR-016: Artifact Interaction Model (Proposed)",
            NodeKind::Concept,
            CONCEPT_ADR_ARTIFACT_INTERACTION,
        )
        .with_tags(["adr", "keymap", "crdt", "architecture"])
        .with_aliases(["artifact type", "interaction model", "canvas modality"]),
        Node::new(
            "concept:collab-architecture",
            "Collaborative Editing Architecture",
            NodeKind::Concept,
            CONCEPT_COLLAB_ARCHITECTURE,
        )
        .with_tags(["architecture", "sync", "collaboration"])
        .with_aliases(["collab", "real-time", "daemon", "multiplayer"]),
        Node::new(
            "concept:collab-workflows",
            "Collaborative Editing Workflows",
            NodeKind::Concept,
            CONCEPT_COLLAB_WORKFLOWS,
        )
        .with_tags(["workflow", "sync", "collaboration"])
        .with_aliases(["collab workflows", "loopback", "multi-user"]),
        Node::new(
            "concept:kb-sharing",
            "Concept: KB Sharing (Collaborative KBs)",
            NodeKind::Concept,
            CONCEPT_KB_SHARING,
        )
        .with_tags([
            "kb",
            "collaboration",
            "sync",
            "sharing",
            "encryption",
            "e2e",
            "identity",
            "recovery",
            "mesh",
        ])
        .with_aliases([
            "kb-share",
            "kb-join",
            "kb-leave",
            "shared kb",
            "collaborative kb",
            "e2e encryption",
            "kb-set-encryption",
            "identity rotation",
            "collab-rotate-identity",
            "recovery key",
            "collab-recover-identity",
            "p2p mesh",
            "kb-share-p2p",
        ]),
        Node::new(
            "concept:scheme-testing",
            "Concept: Scheme Testing Framework",
            NodeKind::Concept,
            CONCEPT_SCHEME_TESTING,
        )
        .with_tags(["testing", "scheme", "development"])
        .with_aliases(["test", "ert", "buttercup", "plenary", "tap"]),
        Node::new(
            "concept:test-runner",
            "Concept: Headless Test Runner",
            NodeKind::Concept,
            CONCEPT_TEST_RUNNER,
        )
        .with_tags(["testing", "development", "architecture"])
        .with_aliases(["mae --test", "headless", "tap"]),
    ]
}

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
            "concept:modules",
            "concept:flags",
            "concept:design-philosophy",
            "concept:kb-federation",
            "concept:kb-workflows",
            "concept:kb-vs-alternatives",
            "concept:dailies",
            "concept:sync-engine",
            "concept:collaborative-state",
            "concept:adr-text-sync",
            "concept:adr-kb-crdt",
            "concept:collab-architecture",
            "concept:collab-workflows",
            "concept:kb-sharing",
            "guide:extension-authoring",
            "lesson:kb-import-roam",
            "lesson:collab-setup",
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
    fn concept_ai_as_peer_links_to_concepts() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        let links = kb.links_from("concept:ai-as-peer");
        // AI tool names are referenced as backtick text (not links) since they're
        // AI tools, not editor commands. Concept links should be present.
        assert!(links.contains(&"concept:introspect".to_string()));
        assert!(links.contains(&"concept:command".to_string()));
        assert!(links.contains(&"concept:knowledge-base".to_string()));
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
    fn tutorial_collab_setup_exists() {
        let kb = seed_kb_default(&CommandRegistry::with_builtins());
        assert!(
            kb.contains("tutorial:collab-setup"),
            "missing tutorial:collab-setup"
        );
        // Should link to lesson and concept nodes
        let links = kb.links_from("tutorial:collab-setup");
        assert!(
            links.contains(&"lesson:collab-setup".to_string()),
            "collab tutorial should link to lesson:collab-setup"
        );
        assert!(
            links.contains(&"concept:collab-architecture".to_string()),
            "collab tutorial should link to concept:collab-architecture"
        );
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
