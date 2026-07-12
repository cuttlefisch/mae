//! Editor -> Scheme state injection, continued: buffer-list/command/
//! keymap introspection, daemon capability parity (ADR-035), and
//! sync/CRDT read-only accessors. See `state_sync_inject.rs` for the
//! dispatcher (`inject_editor_state`) and the rest of the sections.
//!
//! Split out of `runtime.rs` (CLAUDE.md architecture debt reduction pass)
//! -- pure code motion, no behavior change.

use mae_core::Editor;

use crate::ffi::arg_string;
use crate::lisp_error::Arity;
use crate::value::Value;

use super::SchemeRuntime;

impl SchemeRuntime {
    /// Buffer-list, window-list, option-list/get-option, command-list,
    /// keymap introspection, buffer-string/buffer-text, and collab/KB
    /// sharing status snapshots.
    pub(super) fn inject_buffer_list_and_command_fns(&mut self, editor: &Editor) {
        // *buffer-list*
        let buf_info: Vec<Value> = editor
            .buffers
            .iter()
            .enumerate()
            .map(|(i, b)| {
                Value::list(vec![
                    Value::Int(i as i64),
                    Value::string(b.name.clone()),
                    Value::string(format!("{:?}", b.kind)),
                    Value::Bool(b.modified),
                ])
            })
            .collect();
        self.vm
            .define_global("*buffer-list*", Value::list(buf_info));

        // (get-buffer-by-name NAME) — reads from SharedState for always-fresh data
        let s = self.shared.clone();
        self.vm.register_fn(
            "get-buffer-by-name",
            "Get buffer index by name",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "get-buffer-by-name")?;
                let state = s.lock();
                match state.buffer_names.iter().find(|(_, n)| n == &name) {
                    Some((i, _)) => Ok(Value::Int(*i as i64)),
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        // *window-count*
        self.vm.define_global(
            "*window-count*",
            Value::Int(editor.window_mgr.window_count() as i64),
        );

        // *window-list*
        let win_info: Vec<Value> = editor
            .window_mgr
            .iter_windows()
            .map(|w| {
                Value::list(vec![
                    Value::Int(w.id as i64),
                    Value::Int(w.buffer_idx as i64),
                    Value::Int(w.cursor_row as i64),
                    Value::Int(w.cursor_col as i64),
                ])
            })
            .collect();
        self.vm
            .define_global("*window-list*", Value::list(win_info));

        // *option-list*
        let opt_info: Vec<Value> = editor
            .option_registry
            .list()
            .iter()
            .map(|o| {
                Value::list(vec![
                    Value::string(o.name.as_ref()),
                    Value::string(format!("{}", o.kind)),
                    Value::string(o.default_value.as_ref()),
                    Value::string(o.doc.as_ref()),
                ])
            })
            .collect();
        self.vm
            .define_global("*option-list*", Value::list(opt_info));

        // Populate SharedState option_values
        {
            let values: Vec<(String, String)> = editor
                .option_registry
                .list()
                .iter()
                .filter_map(|o| {
                    editor
                        .get_option(&o.name)
                        .map(|(v, _)| (o.name.to_string(), v))
                })
                .collect();
            self.shared.lock().option_values = values;
        }

        // (get-option NAME)
        let s = self.shared.clone();
        self.vm.register_fn(
            "get-option",
            "Get current option value",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "get-option")?;
                let state = s.lock();
                match state.option_values.iter().find(|(n, _)| n == &name) {
                    Some((_, v)) => Ok(Value::string(v.clone())),
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        // *command-list*
        let cmd_info: Vec<Value> = editor
            .commands
            .list_commands()
            .iter()
            .map(|c| {
                Value::list(vec![
                    Value::string(c.name.clone()),
                    Value::string(c.doc.clone()),
                    Value::string(format!("{:?}", c.source)),
                ])
            })
            .collect();
        self.vm
            .define_global("*command-list*", Value::list(cmd_info));

        // (command-exists? NAME)
        let cmd_names: Vec<String> = editor
            .commands
            .list_commands()
            .iter()
            .map(|c| c.name.clone())
            .collect();
        self.vm.register_fn(
            "command-exists?",
            "Check if command exists",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "command-exists?")?;
                Ok(Value::Bool(cmd_names.iter().any(|n| n == &name)))
            },
        );

        // *keymap-list*
        let keymap_names: Vec<Value> = editor
            .keymaps
            .keys()
            .map(|k| Value::string(k.clone()))
            .collect();
        self.vm
            .define_global("*keymap-list*", Value::list(keymap_names));

        // (keymap-bindings MAP-NAME)
        let keymaps_snapshot: std::collections::HashMap<String, Vec<(String, String)>> = editor
            .keymaps
            .iter()
            .map(|(name, km)| {
                let bindings: Vec<(String, String)> = km
                    .bindings()
                    .map(|(seq, cmd)| (mae_core::keymap::serialize_macro(seq), cmd.clone()))
                    .collect();
                (name.clone(), bindings)
            })
            .collect();
        self.vm.register_fn(
            "keymap-bindings",
            "List bindings for a keymap",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "keymap-bindings")?;
                Ok(keymaps_snapshot
                    .get(&name)
                    .map(|bindings| {
                        Value::list(
                            bindings
                                .iter()
                                .map(|(k, c)| {
                                    Value::list(vec![
                                        Value::string(k.clone()),
                                        Value::string(c.clone()),
                                    ])
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .unwrap_or(Value::Null))
            },
        );

        // (buffer-string) — reads from SharedState for always-fresh data
        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-string",
            "Full text of active buffer",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(s.lock().current_buffer_text.clone())),
        );

        // (buffer-text NAME)
        {
            let all_buf_texts: Vec<(String, String)> = editor
                .buffers
                .iter()
                .map(|b| (b.name.clone(), b.text()))
                .collect();
            self.shared.lock().all_buffer_texts = all_buf_texts;
        }
        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-text",
            "Full text of named buffer",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "buffer-text")?;
                let state = s.lock();
                match state
                    .all_buffer_texts
                    .iter()
                    .find(|(n, _)| n == &name || n.ends_with(&name))
                {
                    Some((_, t)) => Ok(Value::string(t.clone())),
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        // (collab-status)
        let collab_status_str = editor.collab.status.as_str().to_string();
        let collab_server_addr = editor.collab.server_address.clone();
        let collab_synced_docs = editor.collab.synced_docs;
        self.vm.register_fn(
            "collab-status",
            "Current collaboration state",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                Ok(Value::list(vec![
                    Value::list(vec![
                        Value::string("status"),
                        Value::string(collab_status_str.clone()),
                    ]),
                    Value::list(vec![
                        Value::string("server"),
                        Value::string(collab_server_addr.clone()),
                    ]),
                    Value::list(vec![
                        Value::string("synced-docs"),
                        Value::Int(collab_synced_docs as i64),
                    ]),
                    Value::list(vec![Value::string("peer-count"), Value::Int(0)]),
                ]))
            },
        );

        // (kb-sharing-status) — JSON snapshot of this peer's KB-sharing state
        // (shared KBs with members + roles, policy, pending requests, my role +
        // epoch, sync status). The SAME snapshot the `*KB Sharing*` buffer and
        // the `kb_sharing_status` MCP tool expose (CLAUDE.md #3 the AI is a peer,
        // #8 one builder). Re-captured each sync so it stays fresh. Returns a JSON
        // string (parse it scheme-side); `{}` if serialization fails.
        let kb_sharing_json = editor.kb_sharing_snapshot_json();
        self.vm.register_fn(
            "kb-sharing-status",
            "JSON snapshot of this peer's KB-sharing state (members, roles, policy, pending, my role/epoch).",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(kb_sharing_json.clone())),
        );
    }

    /// Daemon capability parity (ADR-035): daemon-available?,
    /// daemon-status, feature-available?, collab-synced-buffers,
    /// collab-confirmed-shares.
    pub(super) fn inject_daemon_capability_fns(&mut self, editor: &Editor) {
        // --- Daemon capability parity (ADR-035) ---
        // The human (commands/buffers), the AI peer (MCP `daemon_status`), and
        // Scheme all read the SAME capability model: is a daemon present, and is a
        // given daemon-dependent feature available right now (with the why + fix).
        // Re-captured each sync so it stays fresh, like kb-sharing-status above.

        // (daemon-available?) — is a daemon present (control or read layer)?
        let daemon_present = editor.daemon_available();
        self.vm.register_fn(
            "daemon-available?",
            "Whether a daemon is present (control or read layer) right now.",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(daemon_present)),
        );

        // (daemon-status) — JSON: daemon state + per-feature availability.
        let daemon_status_json = editor.daemon_status_json();
        self.vm.register_fn(
            "daemon-status",
            "JSON snapshot of daemon state + per-feature availability (ADR-035 capability model).",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(daemon_status_json.clone())),
        );

        // (feature-available? FEATURE-ID) — JSON availability for one feature
        // (e.g. "p2p-sharing", "continuous-sync", "kb-hosting"). Per-id JSON is
        // precomputed at registration; the closure looks the id up.
        let feature_json: std::collections::HashMap<String, String> =
            mae_core::editor::DaemonFeature::ALL
                .iter()
                .map(|f| (f.id().to_string(), editor.feature_availability_json(f.id())))
                .collect();
        let feature_ids: Vec<String> = mae_core::editor::DaemonFeature::ALL
            .iter()
            .map(|f| f.id().to_string())
            .collect();
        self.vm.register_fn(
            "feature-available?",
            "JSON availability of a daemon-dependent feature by id (ADR-035 capability model).",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "feature-available?")?;
                let norm = id.trim().to_ascii_lowercase().replace('_', "-");
                let json = feature_json.get(&norm).cloned().unwrap_or_else(|| {
                    let known = feature_ids
                        .iter()
                        .map(|i| format!("\"{i}\""))
                        .collect::<Vec<_>>()
                        .join(",");
                    format!("{{\"error\":\"unknown feature '{id}'\",\"known\":[{known}]}}")
                });
                Ok(Value::string(json))
            },
        );

        // (collab-synced-buffers)
        let synced_names: Vec<String> = editor.collab.synced_buffers.iter().cloned().collect();
        self.vm.register_fn(
            "collab-synced-buffers",
            "List synced buffer names",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                Ok(Value::list(
                    synced_names
                        .iter()
                        .map(|n| Value::string(n.clone()))
                        .collect::<Vec<_>>(),
                ))
            },
        );

        // (collab-confirmed-shares) — doc IDs confirmed by the server.
        // Unlike collab-synced-buffers which is optimistically updated on intent
        // drain, this only contains doc IDs after BufferShared/BufferJoined events.
        let confirmed: Vec<String> = editor.collab.confirmed_shares.iter().cloned().collect();
        self.vm.register_fn(
            "collab-confirmed-shares",
            "List doc IDs confirmed by the server",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                Ok(Value::list(
                    confirmed
                        .iter()
                        .map(|n| Value::string(n.clone()))
                        .collect::<Vec<_>>(),
                ))
            },
        );
    }

    /// Sync/CRDT read-only accessors (buffer-sync-enabled?,
    /// buffer-pending-updates, buffer-sync-content, buffer-drain-updates,
    /// buffer-encode-state, undo/redo-available?).
    pub(super) fn inject_sync_crdt_fns(&mut self, editor: &Editor) {
        let buf = editor.active_buffer();

        // --- Sync/CRDT state --- reads from SharedState for always-fresh data

        let sync_enabled = buf.sync_doc.is_some();
        self.vm
            .define_global("*buffer-sync-enabled?*", Value::Bool(sync_enabled));
        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-sync-enabled?",
            "Whether sync is enabled",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(s.lock().sync_enabled)),
        );

        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-pending-updates",
            "Number of pending sync updates",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(s.lock().pending_update_count as i64)),
        );

        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-sync-content",
            "Sync doc content",
            Arity::Fixed(0),
            move |_args: &[Value]| match &s.lock().sync_content {
                Some(c) => Ok(Value::string(c.clone())),
                None => Ok(Value::Bool(false)),
            },
        );

        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-drain-updates",
            "Take accumulated sync updates",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                let mut state = s.lock();
                let updates = std::mem::take(&mut state.accumulated_sync_updates);
                Ok(Value::list(
                    updates.into_iter().map(Value::string).collect::<Vec<_>>(),
                ))
            },
        );

        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-encode-state",
            "Full yrs document state as base64",
            Arity::Fixed(0),
            move |_args: &[Value]| match &s.lock().encoded_state {
                Some(st) => Ok(Value::string(st.clone())),
                None => Ok(Value::Bool(false)),
            },
        );

        let has_undo = buf.has_undo();
        self.vm.register_fn(
            "undo-available?",
            "Whether undo stack is non-empty",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(has_undo)),
        );

        let has_redo = buf.has_redo();
        self.vm.register_fn(
            "redo-available?",
            "Whether redo stack is non-empty",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(has_redo)),
        );
    }
}
