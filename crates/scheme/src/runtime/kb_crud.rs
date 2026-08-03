//! KB search/get/create/update/delete primitives — the Scheme half of the
//! `kb_search`/`kb_get`/`kb_create`/`kb_update`/`kb_delete` MCP tools
//! (CLAUDE.md principle #3: the AI is a peer, not a plugin).
//!
//! `docs/CROSS_SURFACE_PARITY.md` recorded this as gap #2: the graph-shaped
//! KB queries (`kb-graph`, `kb-neighborhood`, `kb-related`, …) had Scheme
//! primitives, while basic CRUD/search had Command + MCP surfaces only.
//!
//! ## Two shapes, and why
//!
//! This crate never holds a live `&Editor` — only `Arc<Mutex<SharedState>>`
//! (see `kb_export.rs`'s header for the same constraint). That splits the
//! five primitives:
//!
//! - **Reads** (`kb-search`, `kb-get`) run *synchronously* against the live
//!   `KbStore` handles `SharedState` already carries, exactly as every
//!   sibling primitive in `kb_queries.rs` does. `kb-get` additionally
//!   consults `kb_instance_stores` so a federated-instance id resolves the
//!   same way `Editor.kb.instances`-aware code resolves it.
//! - **Writes** (`kb-create`, `kb-update`, `kb-delete`) *queue* a
//!   [`KbNodeOp`] that `state_sync_apply.rs` drains into the real
//!   `Editor::kb_create_node`/`kb_update_node`/`kb_delete_node` — the same
//!   methods `execute_kb_create`/`_update`/`_delete` call for the AI
//!   (principle #15: one implementation, two callers).
//!
//! ## The pre-check on the write path
//!
//! @ai-caution: [architecture-debt] `kb-update`/`kb-delete` verify the node
//! exists *before* queueing, so a Scheme program gets a real, catchable
//! Scheme error for the case it can actually branch on ("no such node")
//! instead of a status-line message it cannot see until the next eval. This
//! is a **fast-fail validation against the same store the write targets**,
//! not a second authorization implementation: seed-node protection, KB write
//! blocking (ADR-048 residency, epoch fencing) and instance routing all stay
//! in `Editor::kb_*_node`, which re-checks everything. Do not grow this
//! pre-check into a policy decision — if you find yourself copying a rule out
//! of `kb_ops/nodes.rs`, the answer is a result-slot read, not a second copy.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi::{arg_int, arg_string};
use crate::lisp_error::{Arity, LispError};
use crate::permission::tier;
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// A queued KB node mutation, lowered editor-side into the matching
/// `Editor::kb_*_node` call by `state_sync_apply.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KbNodeOp {
    Create {
        id: String,
        title: String,
        body: String,
        kind: String,
    },
    Update {
        id: String,
        title: Option<String>,
        body: Option<String>,
        tags: Option<Vec<String>>,
    },
    Delete {
        id: String,
    },
}

/// Look a node up across the primary store and every registered federated
/// instance store, mirroring how `Editor::kb_owner_of` resolves an id across
/// `primary ∪ instances`. Returns `Ok(None)` when no store holds the id.
fn lookup_node(state: &SharedState, id: &str) -> Result<Option<mae_kb::Node>, LispError> {
    let mut stores: Vec<&Arc<dyn mae_kb::KbStore>> = Vec::new();
    if let Some(ref primary) = state.kb_store {
        stores.push(primary);
    }
    for inst in &state.kb_instance_stores {
        stores.push(inst);
    }
    for store in stores {
        match store.get_node(id) {
            Ok(Some(node)) => return Ok(Some(node)),
            Ok(None) => {}
            // A storage failure must not read as "node absent" (ADR-086's
            // read-side rule) — surface it as a Scheme error instead.
            Err(e) => return Err(LispError::internal(format!("kb store error: {e}"))),
        }
    }
    Ok(None)
}

/// `(id title kind body tags)` — the Scheme shape of one node. `tags` is a
/// list of strings (possibly empty), never `#f`, so callers can `map` over it
/// unconditionally.
fn node_to_value(node: &mae_kb::Node) -> Value {
    Value::list(vec![
        Value::string(node.id.clone()),
        Value::string(node.title.clone()),
        Value::string(node.kind.as_str()),
        Value::string(node.body.clone()),
        Value::list(
            node.tags
                .iter()
                .map(|t| Value::string(t.clone()))
                .collect::<Vec<_>>(),
        ),
    ])
}

/// Coerce one Scheme value to an optional string: `#f` means "leave this
/// field alone" on the update path, so an explicit `#f` and an omitted
/// trailing argument mean the same thing.
fn opt_string(v: &Value, fn_name: &str) -> Result<Option<String>, LispError> {
    match v {
        Value::Bool(false) => Ok(None),
        Value::String(s) => Ok(Some(s.to_string())),
        Value::Symbol(s) => Ok(Some(s.name().to_string())),
        other => Err(LispError::type_error(
            "string or #f",
            format!("{fn_name} got {other:?}"),
        )),
    }
}

/// Register the KB CRUD/search primitives.
pub(super) fn register_kb_crud_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    // (kb-search QUERY [SCOPE] [LIMIT]) → list of (id title kind instance)
    let s = shared.clone();
    vm.register_fn(
        "kb-search",
        "Full-text search the knowledge base. QUERY is the search string; optional SCOPE is \
         \"primary\" (the primary KB only, the default) or \"all\" (primary plus every \
         registered federated instance); optional LIMIT caps the number of hits (default 20). \
         Returns a list of (id title kind instance) — instance is #f for a primary-KB hit and \
         the instance's ordinal for a federated one. Counterpart of the kb_search MCP tool.",
        Arity::Variadic(1),
        tier::READ,
        move |args: &[Value]| {
            let query = arg_string(args, 0, "kb-search")?;
            let scope = if args.len() > 1 {
                arg_string(args, 1, "kb-search")?
            } else {
                "primary".to_string()
            };
            let limit = if args.len() > 2 {
                let n = arg_int(args, 2, "kb-search")?;
                if n < 1 {
                    return Err(LispError::internal(
                        "kb-search: LIMIT must be >= 1".to_string(),
                    ));
                }
                n as usize
            } else {
                20
            };
            let federated = match scope.as_str() {
                "primary" | "local" => false,
                "all" => true,
                other => {
                    return Err(LispError::internal(format!(
                        "kb-search: unknown SCOPE {other:?} (expected \"primary\" or \"all\")"
                    )))
                }
            };

            let state = s.lock();
            let mut layers: Vec<(Value, &Arc<dyn mae_kb::KbStore>)> = Vec::new();
            if let Some(ref primary) = state.kb_store {
                layers.push((Value::Bool(false), primary));
            }
            if federated {
                for (i, inst) in state.kb_instance_stores.iter().enumerate() {
                    layers.push((Value::Int(i as i64), inst));
                }
            }

            let mut out = Vec::new();
            for (instance, store) in layers {
                if out.len() >= limit {
                    break;
                }
                let hits = store
                    .fts_search(&query, limit)
                    .map_err(|e| LispError::internal(format!("kb-search: {e}")))?;
                for hit in hits {
                    if out.len() >= limit {
                        break;
                    }
                    // A hit whose node cannot be re-read is a store
                    // inconsistency, not a result — skip it rather than
                    // fabricating empty title/kind fields.
                    if let Ok(Some(node)) = store.get_node(&hit.id) {
                        out.push(Value::list(vec![
                            Value::string(node.id),
                            Value::string(node.title),
                            Value::string(node.kind.as_str()),
                            instance.clone(),
                        ]));
                    }
                }
            }
            Ok(Value::list(out))
        },
    );

    // (kb-get ID) → (id title kind body tags) or #f
    let s = shared.clone();
    vm.register_fn(
        "kb-get",
        "Fetch one KB node by ID from the primary KB or any registered federated instance. \
         Returns (id title kind body tags) — tags is a (possibly empty) list of strings — or #f \
         when no node with that id exists. Signals an error if the underlying store fails (a \
         storage failure must not read as \"node absent\"). Counterpart of the kb_get MCP tool.",
        Arity::Fixed(1),
        tier::READ,
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-get")?;
            let state = s.lock();
            match lookup_node(&state, &id)? {
                Some(node) => Ok(node_to_value(&node)),
                None => Ok(Value::Bool(false)),
            }
        },
    );

    // (kb-create ID TITLE BODY [KIND]) → #t
    let s = shared.clone();
    vm.register_fn(
        "kb-create",
        "Create a KB node. ID must be non-empty and must not already exist. Optional KIND is one \
         of index|command|concept|key|note|project|category|lesson|tutorial|meta|block|task|view \
         (default \"note\"; an unrecognized kind falls back to note, matching the kb_create MCP \
         tool). Returns #t once the create is queued; it is applied on the next editor tick via \
         Editor::kb_create_node — the same method the kb_create MCP tool and the :kb-create \
         command use, which re-checks seed protection and KB write policy.",
        Arity::Variadic(3),
        tier::WRITE,
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-create")?;
            if id.trim().is_empty() {
                return Err(LispError::internal(
                    "kb-create: ID must not be empty".to_string(),
                ));
            }
            let title = arg_string(args, 1, "kb-create")?;
            let body = arg_string(args, 2, "kb-create")?;
            let kind = if args.len() > 3 {
                arg_string(args, 3, "kb-create")?
            } else {
                "note".to_string()
            };
            let mut state = s.lock();
            if lookup_node(&state, &id)?.is_some() {
                return Err(LispError::internal(format!(
                    "kb-create: KB node already exists: {id}"
                )));
            }
            state.pending_kb_node_ops.push(KbNodeOp::Create {
                id,
                title,
                body,
                kind,
            });
            Ok(Value::Bool(true))
        },
    );

    // (kb-update ID [TITLE] [BODY] [TAGS]) → #t
    let s = shared.clone();
    vm.register_fn(
        "kb-update",
        "Update an existing KB node's TITLE, BODY, and/or TAGS. Omit a field (or pass #f) to \
         leave it unchanged; TAGS, when given, is a list of strings that REPLACES the node's tag \
         set. Signals an error if no node with that id exists. Returns #t once the update is \
         queued; it is applied on the next editor tick via Editor::kb_update_node — the same \
         method the kb_update MCP tool uses.",
        Arity::Variadic(1),
        tier::WRITE,
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-update")?;
            let title = if args.len() > 1 {
                opt_string(&args[1], "kb-update")?
            } else {
                None
            };
            let body = if args.len() > 2 {
                opt_string(&args[2], "kb-update")?
            } else {
                None
            };
            let tags = if args.len() > 3 {
                match &args[3] {
                    Value::Bool(false) => None,
                    other => {
                        let items = other.to_list().ok_or_else(|| {
                            LispError::type_error(
                                "list of strings or #f",
                                format!("kb-update got {other:?}"),
                            )
                        })?;
                        let mut tags = Vec::with_capacity(items.len());
                        for item in &items {
                            match opt_string(item, "kb-update")? {
                                Some(t) => tags.push(t),
                                None => {
                                    return Err(LispError::type_error(
                                        "string",
                                        "kb-update tag list contains #f".to_string(),
                                    ))
                                }
                            }
                        }
                        Some(tags)
                    }
                }
            } else {
                None
            };
            if title.is_none() && body.is_none() && tags.is_none() {
                return Err(LispError::internal(
                    "kb-update: nothing to update (pass at least one of TITLE, BODY, TAGS)"
                        .to_string(),
                ));
            }
            let mut state = s.lock();
            if lookup_node(&state, &id)?.is_none() {
                return Err(LispError::internal(format!("kb-update: No KB node: {id}")));
            }
            state.pending_kb_node_ops.push(KbNodeOp::Update {
                id,
                title,
                body,
                tags,
            });
            Ok(Value::Bool(true))
        },
    );

    // (kb-delete ID) → #t
    let s = shared.clone();
    vm.register_fn(
        "kb-delete",
        "Delete a KB node by ID. Signals an error if no node with that id exists. Returns #t once \
         the delete is queued; it is applied on the next editor tick via Editor::kb_delete_node — \
         the same method the kb_delete MCP tool uses, which refuses to delete a protected seed \
         node.",
        Arity::Fixed(1),
        tier::WRITE,
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-delete")?;
            let mut state = s.lock();
            if lookup_node(&state, &id)?.is_none() {
                return Err(LispError::internal(format!("kb-delete: No KB node: {id}")));
            }
            state.pending_kb_node_ops.push(KbNodeOp::Delete { id });
            Ok(Value::Bool(true))
        },
    );
}
