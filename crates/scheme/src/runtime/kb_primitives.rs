//! Typed link functions, KB collaboration lifecycle, collab/identity ACTION
//! primitives, meta-node functions, and relationship type management.
//!
//! Split out of `runtime.rs`'s `SchemeRuntime::new()` (CLAUDE.md architecture
//! debt reduction pass) — pure code motion, no behavior change.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi::arg_string;
use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// Register typed-link, KB collaboration lifecycle, collab/identity ACTION,
/// meta-node, and relationship-type-management primitives.
pub(super) fn register_kb_primitive_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    // --- Typed link functions ---

    // (kb-add-link! SOURCE-ID TARGET-ID REL-TYPE)
    let s = shared.clone();
    vm.register_fn(
        "kb-add-link!",
        "Add a typed link between KB nodes",
        Arity::Fixed(3),
        move |args: &[Value]| {
            let src = arg_string(args, 0, "kb-add-link!")?;
            let dst = arg_string(args, 1, "kb-add-link!")?;
            let rel = arg_string(args, 2, "kb-add-link!")?;
            s.lock().pending_kb_links.push((src, dst, rel));
            Ok(Value::Void)
        },
    );

    // --- KB collaboration lifecycle (first-class, route through CollabIntent) ---

    // (kb-share [KB-NAME]) — share a KB (default = primary).
    let s = shared.clone();
    vm.register_fn(
        "kb-share",
        "Share a knowledge base for collaborative editing (default: primary KB)",
        Arity::Variadic(0),
        move |args: &[Value]| {
            let kb_name = if args.is_empty() {
                mae_core::KB_DEFAULT_NAME.to_string()
            } else {
                arg_string(args, 0, "kb-share")?
            };
            s.lock()
                .pending_kb_collab_actions
                .push(mae_core::KbCollabAction::Share { kb_name });
            Ok(Value::Void)
        },
    );

    // (kb-share-p2p [KB-ID]) — mint a shareable P2P join ticket ("magnet
    // link") and RETURN it (mae://join/…). Unlike (kb-share) this is a
    // synchronous daemon control-socket call, so it returns the ticket string
    // directly. Same single backend as the kb-share-p2p command + kb_share_p2p
    // MCP tool (ADR-025 §"Driving surfaces").
    let s = shared.clone();
    vm.register_fn(
            "kb-share-p2p",
            "Mint a P2P join ticket (magnet link) for a KB and return the mae://join/… string (default: primary KB).",
            Arity::Variadic(0),
            move |args: &[Value]| {
                let kb_id = if args.is_empty() {
                    mae_core::KB_DEFAULT_NAME.to_string()
                } else {
                    arg_string(args, 0, "kb-share-p2p")?
                };
                // Clone the Arc out before the blocking call so the SharedState
                // lock is not held across daemon I/O.
                let control = s.lock().daemon_control.clone();
                match control {
                    Some(c) => c
                        .mint_p2p_ticket(&kb_id)
                        .map(Value::string)
                        .map_err(|e| LispError::user(e, vec![])),
                    None => Err(LispError::user(
                        "not connected to a daemon — start one and enable P2P with \
                         `mae setup-collab --p2p`"
                            .to_string(),
                        vec![],
                    )),
                }
            },
        );

    // (kb-join-ticket TICKET) — queue a P2P join from a "magnet link" and
    // RETURN the daemon's confirmation. Synchronous daemon control-socket call;
    // the background dialer then connects + pulls the KB (after the owner
    // approves). Same single backend as the kb-join-p2p command + kb_join_p2p
    // MCP tool + `mae kb-join` CLI (ADR-025 §"Driving surfaces").
    let s = shared.clone();
    vm.register_fn(
            "kb-join-ticket",
            "Queue a P2P join from a mae://join/… ticket; the dialer pulls the KB after the owner approves.",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let ticket = arg_string(args, 0, "kb-join-ticket")?;
                let control = s.lock().daemon_control.clone();
                match control {
                    Some(c) => c
                        .join_p2p_ticket(&ticket)
                        .map(Value::string)
                        .map_err(|e| LispError::user(e, vec![])),
                    None => Err(LispError::user(
                        "not connected to a daemon — start one and enable P2P with \
                         `mae setup-collab --p2p`"
                            .to_string(),
                        vec![],
                    )),
                }
            },
        );

    // (kb-join KB-ID) — join a shared KB.
    let s = shared.clone();
    vm.register_fn(
        "kb-join",
        "Join a shared knowledge base from the daemon",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let kb_id = arg_string(args, 0, "kb-join")?;
            s.lock()
                .pending_kb_collab_actions
                .push(mae_core::KbCollabAction::Join { kb_id });
            Ok(Value::Void)
        },
    );

    // (kb-leave KB-ID) — leave a shared KB (local copy preserved).
    let s = shared.clone();
    vm.register_fn(
        "kb-leave",
        "Leave a shared knowledge base (local copy preserved)",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let kb_id = arg_string(args, 0, "kb-leave")?;
            s.lock()
                .pending_kb_collab_actions
                .push(mae_core::KbCollabAction::Leave { kb_id });
            Ok(Value::Void)
        },
    );

    // (kb-add-member KB-ID FINGERPRINT [ROLE]) — owner-only.
    let s = shared.clone();
    vm.register_fn(
        "kb-add-member",
        "Add a peer to a shared KB by fingerprint with a role (default editor; owner-only)",
        Arity::Variadic(2),
        move |args: &[Value]| {
            let kb_id = arg_string(args, 0, "kb-add-member")?;
            let member = arg_string(args, 1, "kb-add-member")?;
            let role = if args.len() > 2 {
                arg_string(args, 2, "kb-add-member")?
            } else {
                "editor".to_string()
            };
            s.lock()
                .pending_kb_collab_actions
                .push(mae_core::KbCollabAction::AddMember {
                    kb_id,
                    member,
                    role,
                });
            Ok(Value::Void)
        },
    );

    // (kb-remove-member KB-ID FINGERPRINT) — owner-only.
    let s = shared.clone();
    vm.register_fn(
        "kb-remove-member",
        "Remove a peer from a shared KB by fingerprint (owner-only)",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let kb_id = arg_string(args, 0, "kb-remove-member")?;
            let member = arg_string(args, 1, "kb-remove-member")?;
            s.lock()
                .pending_kb_collab_actions
                .push(mae_core::KbCollabAction::RemoveMember { kb_id, member });
            Ok(Value::Void)
        },
    );

    // (kb-approve KB-ID FINGERPRINT [ROLE]) — approve a pending join (owner-only).
    let s = shared.clone();
    vm.register_fn(
        "kb-approve",
        "Approve a pending join request by fingerprint at a role (default editor; owner-only)",
        Arity::Variadic(2),
        move |args: &[Value]| {
            let kb_id = arg_string(args, 0, "kb-approve")?;
            let principal = arg_string(args, 1, "kb-approve")?;
            let role = if args.len() > 2 {
                arg_string(args, 2, "kb-approve")?
            } else {
                "editor".to_string()
            };
            s.lock()
                .pending_kb_collab_actions
                .push(mae_core::KbCollabAction::Approve {
                    kb_id,
                    principal,
                    role,
                });
            Ok(Value::Void)
        },
    );

    // (kb-set-policy KB-ID POLICY) — restrictive|invite|permissive (owner-only).
    let s = shared.clone();
    vm.register_fn(
        "kb-set-policy",
        "Set a shared KB's join policy: restrictive | invite | permissive (owner-only)",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let kb_id = arg_string(args, 0, "kb-set-policy")?;
            let policy = arg_string(args, 1, "kb-set-policy")?;
            s.lock()
                .pending_kb_collab_actions
                .push(mae_core::KbCollabAction::SetPolicy { kb_id, policy });
            Ok(Value::Void)
        },
    );

    // (kb-block-member KB-ID FINGERPRINT) — add a principal to this daemon's LOCAL
    // self-protection blocklist (ADR-039 A2, #162). Local-only, never propagated;
    // not owner-gated (you may block even the owner).
    let s = shared.clone();
    vm.register_fn(
            "kb-block-member",
            "Locally block a principal on a KB by fingerprint (self-protection deny-list; local-only, not propagated)",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-block-member")?;
                let member = arg_string(args, 1, "kb-block-member")?;
                s.lock().pending_kb_collab_actions.push(
                    mae_core::KbCollabAction::SetBlock {
                        kb_id,
                        member,
                        blocked: true,
                    },
                );
                Ok(Value::Void)
            },
        );

    // (kb-unblock-member KB-ID FINGERPRINT) — remove a principal from the LOCAL
    // self-protection blocklist (ADR-039 A2, #162).
    let s = shared.clone();
    vm.register_fn(
            "kb-unblock-member",
            "Locally unblock a principal on a KB by fingerprint (removes the local self-protection block)",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-unblock-member")?;
                let member = arg_string(args, 1, "kb-unblock-member")?;
                s.lock().pending_kb_collab_actions.push(
                    mae_core::KbCollabAction::SetBlock {
                        kb_id,
                        member,
                        blocked: false,
                    },
                );
                Ok(Value::Void)
            },
        );

    // (kb-set-encryption KB-ID MODE) — enable E2E content encryption (owner-only,
    // one-way: MODE = "e2e"). ADR-037/039.
    let s = shared.clone();
    vm.register_fn(
        "kb-set-encryption",
        "Enable E2E content encryption on an owned KB (owner-only, one-way): MODE = \"e2e\". \
Protects node CONTENT from non-members/relay; does NOT provide forward secrecy, hide metadata \
(who/when/which-node/size), or retroactively protect already-shared plaintext — enable before \
sharing. Lost identity key = permanent loss. See :help concept:kb-encryption.",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let kb_id = arg_string(args, 0, "kb-set-encryption")?;
            let mode = arg_string(args, 1, "kb-set-encryption")?;
            s.lock()
                .pending_kb_collab_actions
                .push(mae_core::KbCollabAction::SetEncryption { kb_id, mode });
            Ok(Value::Void)
        },
    );

    // (kb-join-p2p TICKET) — parity alias for (kb-join-ticket): the P2P
    // join command surface is `kb-join-p2p`, so the same name resolves in
    // Scheme (principle #3 — one action, same name on every surface). Same
    // single backend (daemon control-socket `join_p2p_ticket`).
    let s = shared.clone();
    vm.register_fn(
        "kb-join-p2p",
        "Queue a P2P join from a mae://join/… ticket (alias of kb-join-ticket).",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let ticket = arg_string(args, 0, "kb-join-p2p")?;
            let control = s.lock().daemon_control.clone();
            match control {
                Some(c) => c
                    .join_p2p_ticket(&ticket)
                    .map(Value::string)
                    .map_err(|e| LispError::user(e, vec![])),
                None => Err(LispError::user(
                    "not connected to a daemon — start one and enable P2P with \
                         `mae setup-collab --p2p`"
                        .to_string(),
                    vec![],
                )),
            }
        },
    );

    // --- Collab/identity ACTION primitives (first-class parity, #3) ---
    // These editor actions have a command + an MCP tool; give them a named
    // Scheme prim too (instead of forcing generic (run-command …)) so they
    // are discoverable via :help scheme:* and self-documenting. Each just
    // routes the confirmed command name through the same dispatch the
    // command surface uses (no-arg → pending_commands; arg-taking →
    // pending_ex_commands / the ex parser). The action runs on the next
    // editor-loop drain.
    macro_rules! register_collab_command_prim {
        ($name:literal, $doc:literal) => {{
            let s = shared.clone();
            vm.register_fn($name, $doc, Arity::Fixed(0), move |_args: &[Value]| {
                s.lock().pending_commands.push($name.to_string());
                Ok(Value::Void)
            });
        }};
    }
    register_collab_command_prim!(
        "collab-rotate-identity",
        "Rotate this peer's collab identity key across every KB it owns/belongs to (ADR-040). \
Authorize the new key on the daemon out-of-band, then reconnect."
    );
    register_collab_command_prim!(
        "collab-register-recovery-key",
        "Register an offline recovery key across your KBs (ADR-040 §Recovery-key). Back up the \
saved recovery key OFFLINE — it can later authorize a rebind if the primary is lost."
    );
    register_collab_command_prim!(
        "collab-disconnect",
        "Disconnect from the collaboration daemon."
    );
    register_collab_command_prim!(
        "collab-doctor",
        "Run collaboration connectivity diagnostics and report the results."
    );
    register_collab_command_prim!(
        "collab-list",
        "List shared documents advertised by the connected daemon."
    );
    register_collab_command_prim!(
        "collab-discover",
        "Discover MAE collaboration peers on the local network via mDNS."
    );
    register_collab_command_prim!(
            "collab-share",
            "Share the active buffer for collaborative editing (parity with the command + collab_share MCP tool)."
        );
    register_collab_command_prim!(
        "collab-sync",
        "Force a sync of shared buffers with the daemon now."
    );
    register_collab_command_prim!(
        "kb-list-remote",
        "List shared KBs advertised by the connected daemon."
    );

    // (kb-pending KB-ID) — list pending join requests for a shared KB you own
    // (the same set surfaced in kb-sharing-status). Arg-taking → ex parser.
    let s = shared.clone();
    vm.register_fn(
        "kb-pending",
        "List pending join requests for a shared KB by id (owner-only view).",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let kb_id = arg_string(args, 0, "kb-pending")?;
            s.lock()
                .pending_ex_commands
                .push(format!("kb-pending {kb_id}"));
            Ok(Value::Void)
        },
    );

    // (kb-set-ai-residency KB-ID-OR-PRIMARY POLICY) — open | local_models_only
    // (ADR-048). NOT a collab/daemon action despite living alongside the KB-sharing
    // prims above — a plain, freely-toggleable local registry field (one local user's
    // own KB, not a multi-peer trust problem), so it routes through the ex parser
    // (`dispatch_kb` in `editor/dispatch/kb.rs`) like `kb-pending` above, not through
    // `pending_kb_collab_actions`/`CollabIntent`.
    let s = shared.clone();
    vm.register_fn(
            "kb-set-ai-residency",
            "Set a KB's AI-residency policy: open | local_models_only (ADR-048). Use \"primary\" for the primary/local KB.",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-set-ai-residency")?;
                let policy = arg_string(args, 1, "kb-set-ai-residency")?;
                s.lock()
                    .pending_ex_commands
                    .push(format!("kb-set-ai-residency {kb_id} {policy}"));
                Ok(Value::Void)
            },
        );

    // (kb-set-role NODE-ID ROLE) — source | atom | molecule | hub, the molecular-note
    // classification (Source→Atom→Molecule→Hub). Also NOT a collab/daemon action —
    // routes through the ex parser like kb-set-ai-residency above.
    let s = shared.clone();
    vm.register_fn(
        "kb-set-role",
        "Set a KB node's molecular-note role: source | atom | molecule | hub.",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-set-role")?;
            let role = arg_string(args, 1, "kb-set-role")?;
            s.lock()
                .pending_ex_commands
                .push(format!("kb-set-role {id} {role}"));
            Ok(Value::Void)
        },
    );

    // (collab-connect [ADDR]) — connect to a daemon; ADDR optional (defaults
    // to the configured server). Arg-taking → route through the ex parser.
    let s = shared.clone();
    vm.register_fn(
            "collab-connect",
            "Connect to a collaboration daemon. Optional ADDR (host:port) overrides the configured server.",
            Arity::Variadic(0),
            move |args: &[Value]| {
                let cmd = if args.is_empty() {
                    "collab-connect".to_string()
                } else {
                    format!("collab-connect {}", arg_string(args, 0, "collab-connect")?)
                };
                s.lock().pending_ex_commands.push(cmd);
                Ok(Value::Void)
            },
        );

    // (collab-recover-identity RECOVERY-KEY-PATH OLD-FINGERPRINT) — recover a
    // lost identity via a pre-registered offline recovery key (ADR-040
    // §Recovery-key). Arg-taking → ex parser (parsed in editor/command.rs).
    // Closes the G5 parity gap (had an MCP tool + command but no Scheme peer).
    let s = shared.clone();
    vm.register_fn(
            "collab-recover-identity",
            "Recover a lost identity via an offline recovery key: RECOVERY-KEY-PATH (dir holding the \
restored recovery id_ed25519) + OLD-FINGERPRINT (the lost key's SHA256:…). Authors a recovery-signed \
rebind so a fresh primary inherits the lost key's seats (ADR-040 §Recovery-key).",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "collab-recover-identity")?;
                let old_fp = arg_string(args, 1, "collab-recover-identity")?;
                s.lock()
                    .pending_ex_commands
                    .push(format!("collab-recover-identity {path} {old_fp}"));
                Ok(Value::Void)
            },
        );

    // (kb-remove-link! SOURCE-ID TARGET-ID)
    let s = shared.clone();
    vm.register_fn(
        "kb-remove-link!",
        "Remove a link between KB nodes",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let src = arg_string(args, 0, "kb-remove-link!")?;
            let dst = arg_string(args, 1, "kb-remove-link!")?;
            s.lock().pending_kb_link_removals.push((src, dst));
            Ok(Value::Void)
        },
    );

    // --- Meta-node functions ---

    // (kb-add-meta-member! META-ID MEMBER-ID ROLE)
    let s = shared.clone();
    vm.register_fn(
        "kb-add-meta-member!",
        "Add a member to a meta-node",
        Arity::Fixed(3),
        move |args: &[Value]| {
            let meta = arg_string(args, 0, "kb-add-meta-member!")?;
            let member = arg_string(args, 1, "kb-add-meta-member!")?;
            let role = arg_string(args, 2, "kb-add-meta-member!")?;
            s.lock().pending_kb_meta_adds.push((meta, member, role));
            Ok(Value::Void)
        },
    );

    // (kb-remove-meta-member! META-ID MEMBER-ID)
    let s = shared.clone();
    vm.register_fn(
        "kb-remove-meta-member!",
        "Remove a member from a meta-node",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let meta = arg_string(args, 0, "kb-remove-meta-member!")?;
            let member = arg_string(args, 1, "kb-remove-meta-member!")?;
            s.lock().pending_kb_meta_removes.push((meta, member));
            Ok(Value::Void)
        },
    );

    // (kb-compose-meta META-ID) — recompose meta body from members
    let s = shared.clone();
    vm.register_fn(
        "kb-compose-meta",
        "Recompose a meta-node body from its members",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-compose-meta")?;
            s.lock()
                .pending_ex_commands
                .push(format!("kb-compose-meta {}", id));
            Ok(Value::Void)
        },
    );

    // --- Relationship type management ---

    // (kb-add-rel-type! NAME LABEL DESCRIPTION INVERSE DIRECTED)
    let s = shared.clone();
    vm.register_fn(
        "kb-add-rel-type!",
        "Add a custom relationship type to the KB",
        Arity::Fixed(5),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "kb-add-rel-type!")?;
            let label = arg_string(args, 1, "kb-add-rel-type!")?;
            let desc = arg_string(args, 2, "kb-add-rel-type!")?;
            let inverse = arg_string(args, 3, "kb-add-rel-type!")?;
            let directed = match &args[4] {
                Value::Bool(b) => *b,
                _ => true,
            };
            s.lock().pending_ex_commands.push(format!(
                "kb-add-rel-type {} {} {} {} {}",
                name, label, desc, inverse, directed
            ));
            Ok(Value::Void)
        },
    );
}
