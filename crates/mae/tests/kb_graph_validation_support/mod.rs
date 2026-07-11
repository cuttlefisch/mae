//! Shared fixtures/helpers for the kb_graph_validation test suite, split
//! across kb_graph_validation_*.rs files to stay under the 500-line test
//! ceiling. NOT itself a test target: Cargo's `tests/*.rs` auto-discovery
//! only globs direct children of `tests/`, so `tests/<dir>/mod.rs` is never
//! picked up as its own integration-test binary.
//!
//! Not every consumer uses every helper (each `kb_graph_validation_*.rs` file
//! is a separate compiled crate, so this module is recompiled per-binary) --
//! `#[allow(dead_code)]` on each fn suppresses the resulting per-binary warning.

use std::collections::HashMap;

use mae_core::commands::CommandRegistry;
use mae_core::hooks::HookRegistry;
use mae_core::kb_seed::seed_kb;
use mae_kb::{CozoKbStore, IngestMode, KbStore};

/// Extract a string value from raw_query's Debug-formatted DataValue output.
/// The `raw_query` method uses `format!("{v:?}")` which for CozoDB DataValue::Str
/// produces strings like `"\"hello\""` (a JSON-style quoted string).
#[allow(dead_code)]
pub fn dv_str(s: &str) -> String {
    // Most common: string with surrounding quotes like "\"value\""
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        return s[1..s.len() - 1].replace("\\\"", "\"").to_string();
    }
    // Fallback: DataValue Debug format variants
    if let Some(inner) = s.strip_prefix("Str(\"").and_then(|s| s.strip_suffix("\")")) {
        return inner.to_string();
    }
    if let Some(inner) = s
        .strip_prefix("Num(Int(")
        .and_then(|s| s.strip_suffix("))"))
    {
        return inner.to_string();
    }
    s.to_string()
}

/// Org fixture files that exercise all extended syntax: typed links, fragments,
/// verbatim blocks, property drawers with :KIND:/:ALIASES:, multi-node files,
/// and the full lesson prerequisite chain.
#[allow(dead_code)]
pub fn write_org_fixtures(dir: &std::path::Path) {
    let fixtures: Vec<(&str, &str)> = vec![
        // Index node with categorizes links
        (
            "index.org",
            r#":PROPERTIES:
:ID: index
:KIND: index
:END:
#+title: MAE Help Index

## Core concepts
- [[concept:buffer][Buffer]]
- [[concept:mode][Mode]]
- [[concept:ai-as-peer][AI as Peer]]
- [[concept:knowledge-base][Knowledge Base]]
- [[concept:scheme-api][Scheme API]]
- [[concept:debugging][Debugging]]
"#,
        ),
        // Concept nodes with part_of, references, implements links
        (
            "concept-buffer.org",
            r#":PROPERTIES:
:ID: concept:buffer
:KIND: concept
:ALIASES: rope, text buffer
:END:
#+title: Buffer

A buffer is the unit of editable content. [[concept:window?rel=references][See windows]].
[[concept:mode?rel=references]]
"#,
        ),
        (
            "concept-mode.org",
            r#":PROPERTIES:
:ID: concept:mode
:KIND: concept
:END:
#+title: Mode

Modes control which keymap is active. [[concept:buffer?rel=references]]
"#,
        ),
        (
            "concept-window.org",
            r#":PROPERTIES:
:ID: concept:window
:KIND: concept
:END:
#+title: Window

A window is a view onto a [[concept:buffer?rel=references][buffer]].
[[concept:mode?rel=part_of]]
"#,
        ),
        (
            "concept-ai-as-peer.org",
            r#":PROPERTIES:
:ID: concept:ai-as-peer
:KIND: concept
:END:
#+title: The AI as Peer Actor

The AI is a peer, not a plugin. [[concept:scheme-api?rel=references]]
"#,
        ),
        (
            "concept-knowledge-base.org",
            r#":PROPERTIES:
:ID: concept:knowledge-base
:KIND: concept
:END:
#+title: Knowledge Base

The KB stores nodes and typed links. [[concept:buffer?rel=references]]
"#,
        ),
        (
            "concept-terminal.org",
            r#":PROPERTIES:
:ID: concept:terminal
:KIND: concept
:END:
#+title: Embedded Terminal

Full terminal emulator inside MAE. [[concept:buffer?rel=part_of]]
"#,
        ),
        (
            "concept-scheme-api.org",
            r#":PROPERTIES:
:ID: concept:scheme-api
:KIND: concept
:END:
#+title: Scheme API

~50 functions for buffer/window/command access. [[concept:buffer?rel=references]]
"#,
        ),
        (
            "concept-debugging.org",
            r#":PROPERTIES:
:ID: concept:debugging
:KIND: concept
:END:
#+title: Debugging (DAP)

DAP client, debug panel, breakpoints. [[concept:buffer?rel=references]]
"#,
        ),
        (
            "concept-command.org",
            r#":PROPERTIES:
:ID: concept:command
:KIND: concept
:END:
#+title: Command

Commands are the shared API. [[concept:scheme-api?rel=references]]
"#,
        ),
        (
            "concept-watchdog.org",
            r#":PROPERTIES:
:ID: concept:watchdog
:KIND: concept
:END:
#+title: Watchdog

Event loop stall detection. [[concept:debugging?rel=part_of]]
"#,
        ),
        (
            "concept-event-recording.org",
            r#":PROPERTIES:
:ID: concept:event-recording
:KIND: concept
:END:
#+title: Event Recording

Session capture and JSON export. [[concept:debugging?rel=part_of]]
"#,
        ),
        (
            "concept-introspect.org",
            r#":PROPERTIES:
:ID: concept:introspect
:KIND: concept
:END:
#+title: Introspect

AI diagnostic snapshot. [[concept:debugging?rel=part_of]]
"#,
        ),
        (
            "concept-hooks.org",
            r#":PROPERTIES:
:ID: concept:hooks
:KIND: concept
:END:
#+title: Hooks

Scheme extension points for editor events. [[concept:scheme-api?rel=references]]
"#,
        ),
        (
            "concept-collaborative-state.org",
            r#":PROPERTIES:
:ID: concept:collaborative-state
:KIND: concept
:END:
#+title: Collaborative State

Vision: text + visual + KB sync. [[concept:buffer?rel=references]]
"#,
        ),
        (
            "concept-sync-engine.org",
            r#":PROPERTIES:
:ID: concept:sync-engine
:KIND: concept
:END:
#+title: Sync Engine

yrs (Yjs Rust) CRDT for collaborative state.
This concept [[concept:collaborative-state?rel=implements][implements Collaborative State]].
[[concept:buffer?rel=references]]
"#,
        ),
        (
            "concept-options.org",
            r#":PROPERTIES:
:ID: concept:options
:KIND: concept
:END:
#+title: Editor Options

Configuring MAE from Scheme. [[concept:scheme-api?rel=references]]
"#,
        ),
        (
            "concept-option-registry.org",
            r#":PROPERTIES:
:ID: concept:option-registry
:KIND: concept
:END:
#+title: Option Registry

Single source of truth for settings.
This concept [[concept:options?rel=implements][implements Editor Options]].
"#,
        ),
        (
            "concept-ai-modes.org",
            r#":PROPERTIES:
:ID: concept:ai-modes
:KIND: concept
:END:
#+title: AI Agent vs Chat

When to use each AI interface. [[concept:ai-as-peer?rel=references]]
"#,
        ),
        (
            "concept-kb-federation.org",
            r#":PROPERTIES:
:ID: concept:kb-federation
:KIND: concept
:END:
#+title: KB Federation

Multi-instance knowledge sharing. [[concept:knowledge-base?rel=references]]
"#,
        ),
        // Lesson chain: 12 lessons with requires + teaches typed links
        (
            "lesson-navigation.org",
            r#":PROPERTIES:
:ID: lesson:navigation
:KIND: lesson
:END:
#+title: Lesson 1: Navigation
#+filetags: :tutorial:

This lesson covers [[concept:buffer?rel=teaches][buffers]] and [[concept:window?rel=teaches][windows]].
"#,
        ),
        (
            "lesson-modes.org",
            r#":PROPERTIES:
:ID: lesson:modes
:KIND: lesson
:END:
#+title: Lesson 2: Modes
#+filetags: :tutorial:

MAE uses [[concept:mode?rel=teaches][modal editing]].
Prerequisites: [[lesson:navigation?rel=requires][Lesson 1]].
"#,
        ),
        (
            "lesson-editing.org",
            r#":PROPERTIES:
:ID: lesson:editing
:KIND: lesson
:END:
#+title: Lesson 3: Editing
#+filetags: :tutorial:

This lesson [[concept:command?rel=teaches][teaches editing commands]].
Prerequisites: [[lesson:modes?rel=requires][Lesson 2]].
"#,
        ),
        (
            "lesson-files.org",
            r#":PROPERTIES:
:ID: lesson:files
:KIND: lesson
:END:
#+title: Lesson 4: Files & Buffers
#+filetags: :tutorial:

A [[concept:buffer?rel=teaches][buffer]] is the unit of editable content.
Prerequisites: [[lesson:editing?rel=requires][Lesson 3]].
"#,
        ),
        (
            "lesson-ai.org",
            r#":PROPERTIES:
:ID: lesson:ai
:KIND: lesson
:END:
#+title: Lesson 5: AI Features
#+filetags: :tutorial:

MAE treats AI as a [[concept:ai-as-peer?rel=teaches][peer actor]].
[[concept:ai-modes?rel=teaches][AI commands]]
Prerequisites: [[lesson:files?rel=requires][Lesson 4]].
"#,
        ),
        (
            "lesson-scheme.org",
            r#":PROPERTIES:
:ID: lesson:scheme
:KIND: lesson
:END:
#+title: Lesson 6: Scheme REPL
#+filetags: :tutorial:

MAE is extensible via R7RS Scheme. [[concept:scheme-api?rel=teaches][Scheme API]].
Prerequisites: [[lesson:ai?rel=requires][Lesson 5]].
"#,
        ),
        (
            "lesson-lsp.org",
            r#":PROPERTIES:
:ID: lesson:lsp
:KIND: lesson
:END:
#+title: Lesson 7: LSP
#+filetags: :tutorial:

LSP [[concept:command?rel=teaches][commands]] give you code navigation.
Prerequisites: [[lesson:scheme?rel=requires][Lesson 6]].
"#,
        ),
        (
            "lesson-terminal.org",
            r#":PROPERTIES:
:ID: lesson:terminal
:KIND: lesson
:END:
#+title: Lesson 8: Terminal
#+filetags: :tutorial:

MAE embeds a full [[concept:terminal?rel=teaches][terminal emulator]].
Prerequisites: [[lesson:lsp?rel=requires][Lesson 7]].
"#,
        ),
        (
            "lesson-help.org",
            r#":PROPERTIES:
:ID: lesson:help
:KIND: lesson
:END:
#+title: Lesson 9: Help System
#+filetags: :tutorial:

MAE's help is a [[concept:knowledge-base?rel=teaches][knowledge base]].
Prerequisites: [[lesson:terminal?rel=requires][Lesson 8]].
"#,
        ),
        (
            "lesson-leader.org",
            r#":PROPERTIES:
:ID: lesson:leader
:KIND: lesson
:END:
#+title: Lesson 10: Leader Keys
#+filetags: :tutorial:

See also: [[concept:command?rel=teaches][Commands]].
Prerequisites: [[lesson:help?rel=requires][Lesson 9]].
"#,
        ),
        (
            "lesson-debugging.org",
            r#":PROPERTIES:
:ID: lesson:debugging
:KIND: lesson
:END:
#+title: Lesson 11: Debugging
#+filetags: :tutorial:

MAE has a [[concept:debugging?rel=teaches][DAP client]].
Prerequisites: [[lesson:leader?rel=requires][Lesson 10]].
"#,
        ),
        (
            "lesson-observability.org",
            r#":PROPERTIES:
:ID: lesson:observability
:KIND: lesson
:END:
#+title: Lesson 12: Observability
#+filetags: :tutorial:

The [[concept:watchdog?rel=teaches][watchdog]] monitors the event loop.
[[concept:event-recording?rel=teaches][Event recording]] captures events.
[[concept:introspect?rel=teaches][introspect]] provides diagnostics.
Prerequisites: [[lesson:debugging?rel=requires][Lesson 11]].
"#,
        ),
        // Verbatim block test — links inside should NOT be parsed
        (
            "concept-org-link-syntax.org",
            r#":PROPERTIES:
:ID: concept:org-link-syntax
:KIND: concept
:END:
#+title: Org Link Syntax

Typed links use =[[NODE_ID?rel=REL_TYPE]]= syntax.

#+begin_example
:PROPERTIES:
:ID: concept:fake-should-not-parse
:KIND: concept
:END:
See [[concept:also-fake]] inside example.
#+end_example

See also: [[concept:knowledge-base?rel=references]]
"#,
        ),
        // Tutorial nodes
        (
            "tutorial-getting-started.org",
            r#":PROPERTIES:
:ID: tutorial:getting-started
:KIND: tutorial
:END:
#+title: Getting Started

Progressive guide. [[tutorial:vim-familiar?rel=contains]]
[[tutorial:ai-setup?rel=contains]]
"#,
        ),
        (
            "tutorial-vim-familiar.org",
            r#":PROPERTIES:
:ID: tutorial:vim-familiar
:KIND: tutorial
:END:
#+title: What Carries Over from Vim

[[lesson:navigation?rel=teaches][Teaches: Navigation]]
"#,
        ),
        (
            "tutorial-ai-setup.org",
            r#":PROPERTIES:
:ID: tutorial:ai-setup
:KIND: tutorial
:END:
#+title: AI Setup

[[concept:ai-as-peer?rel=teaches][Teaches: AI as Peer]]
"#,
        ),
        // Multi-node file (file + heading with separate ID)
        (
            "multi-node-test.org",
            r#":PROPERTIES:
:ID: concept:multi-parent
:KIND: concept
:END:
#+title: Multi-Node Test

Parent node body. [[concept:buffer?rel=references]]

** Child Section
:PROPERTIES:
:ID: concept:multi-child
:KIND: concept
:END:

Child body. [[concept:multi-parent?rel=part_of]]
"#,
        ),
        // Fragment link test
        (
            "concept-fragment-test.org",
            r#":PROPERTIES:
:ID: concept:fragment-test
:KIND: concept
:END:
#+title: Fragment Test

See [[concept:buffer#rope-internals]] for details.
See [[concept:scheme-api#hooks?rel=teaches]] for hooks.
"#,
        ),
    ];

    for (name, content) in &fixtures {
        std::fs::write(dir.join(name), content).unwrap();
    }
}

/// Build a CozoDB store pre-loaded with seed nodes + test fixture org files.
///
/// Uses code-generated nodes from `seed_kb()` (commands, categories, options,
/// scheme_api) plus focused org fixture files that exercise all extended syntax
/// features (typed links, fragments, verbatim blocks, multi-node files).
#[allow(dead_code)]
pub fn make_seeded_store() -> (tempfile::TempDir, CozoKbStore) {
    // In-memory CozoDB: these tests validate graph/Datalog logic, not on-disk
    // persistence. The sled (on-disk) backend's per-write fsync is pathologically
    // slow on macOS/APFS (~39s per test, 18min total), whereas the mem engine
    // runs the same queries in milliseconds. The temp dir is still used for the
    // org fixture files imported below.
    let tmp = tempfile::tempdir().unwrap();
    let store = CozoKbStore::open_mem().unwrap();

    // Build the in-memory KB with all seed nodes (commands, options, etc.)
    let registry = CommandRegistry::with_builtins();
    let keymaps = HashMap::new();
    let hooks = HookRegistry::new();
    let kb = seed_kb(&registry, &keymaps, &hooks);

    // Load all nodes into the CozoDB store
    let ids = kb.list_ids(None);
    for id in &ids {
        if let Some(node) = kb.get(id) {
            store.insert_node(node).unwrap();
        }
    }

    // Seed the type system and typed relationships
    store.seed_type_system().unwrap();
    store.seed_typed_relationships().unwrap();
    store.seed_views().unwrap();

    // Import focused org fixtures (NOT the full 232-file manual)
    let fixture_dir = tmp.path().join("fixtures");
    std::fs::create_dir_all(&fixture_dir).unwrap();
    write_org_fixtures(&fixture_dir);
    let _ = mae_kb::import_org_dir_to_store(&fixture_dir, &store, &IngestMode::Full);

    (tmp, store)
}
