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

/// Every KB store to consult, primary first, **each exactly once**.
///
/// @ai-caution: [architecture-debt] `SharedState::kb_instance_stores` is
/// populated as *primary-first, then each federated instance*
/// (`state_sync_inject.rs`), so it already contains the primary store —
/// concatenating it with `kb_store` would visit the primary twice, which for
/// `kb-search` means every primary hit appearing twice in the result list.
/// Do not "fix" a missing-primary bug by adding `kb_store` back on top; the
/// only case where it is not already in the list is when the list is empty.
fn all_stores(state: &SharedState) -> Vec<Arc<dyn mae_kb::KbStore>> {
    if !state.kb_instance_stores.is_empty() {
        return state.kb_instance_stores.clone();
    }
    state.kb_store.iter().cloned().collect()
}

/// Look a node up across the primary store and every registered federated
/// instance store, mirroring how `Editor::kb_owner_of` resolves an id across
/// `primary ∪ instances`. Returns `Ok(None)` when no store holds the id.
fn lookup_node(state: &SharedState, id: &str) -> Result<Option<mae_kb::Node>, LispError> {
    for store in all_stores(state) {
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

/// Load one store's nodes into an in-memory [`mae_kb::KnowledgeBase`] so the
/// SAME `search_ranked` the `kb_search` MCP tool uses can rank them.
///
/// @ai-caution: [architecture-debt] Do NOT "optimise" this back to
/// `KbStore::fts_search`. That index is demonstrably lossy — on a freshly
/// seeded store, a node titled `"alpha beta gamma delta"` with body
/// `"epsilon zeta eta"` is found by `alpha`/`beta`/`gamma`/`zeta`/`eta` and
/// **not** by `delta`/`epsilon`; `mae-kb`'s own `fts_search_finds_nodes` test
/// passes only because `quantum` happens to be one of the terms that work
/// (the "unicorn value" failure mode CLAUDE.md principle #14 names). Building
/// `kb-search` on it would make the Scheme surface silently *miss* nodes the
/// MCP `kb_search` tool returns, which is the parity asymmetry principle #3
/// rules out — in the more dangerous direction, since a missing result looks
/// like an absent node. See `docs/DECISIONS_FOR_REVIEW.md` for the defect
/// itself, which is pre-existing and independent of this primitive.
///
/// Cost: one `load_all()` per store per search. That is the same O(n) shape
/// `Editor::kb_federated_search_scoped` already pays (it scans the in-memory
/// federated mirror), so this is not a new order of cost for the editor —
/// but it does deserialize rather than reuse a resident mirror, which this
/// crate has no access to.
fn store_as_kb(store: &Arc<dyn mae_kb::KbStore>) -> Result<mae_kb::KnowledgeBase, LispError> {
    let nodes = store
        .load_all()
        .map_err(|e| LispError::internal(format!("kb-search: {e}")))?;
    let mut kb = mae_kb::KnowledgeBase::new();
    for node in nodes {
        kb.insert(node);
    }
    Ok(kb)
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
         the instance's ordinal for a federated one. Matching and ranking use the same \
         KnowledgeBase::search_ranked the kb_search MCP tool uses, so both surfaces agree on what \
         matches: a plain substring/term search over title and body, case-insensitive, with no \
         query-operator syntax — \"concept:buffer\" and \"kb-sharing\" search for themselves. An \
         empty QUERY lists everything, capped by LIMIT.",
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
            // `all_stores` is primary-first and duplicate-free (see its
            // `@ai-caution`), so index 0 is the primary and every later index
            // is a federated instance. "primary" scope takes only the first.
            let stores = all_stores(&state);
            let layers: Vec<(Value, Arc<dyn mae_kb::KbStore>)> = stores
                .into_iter()
                .enumerate()
                .take(if federated { usize::MAX } else { 1 })
                .map(|(i, store)| {
                    let label = if i == 0 {
                        Value::Bool(false)
                    } else {
                        Value::Int((i - 1) as i64)
                    };
                    (label, store)
                })
                .collect();

            let mut out = Vec::new();
            for (instance, store) in layers {
                if out.len() >= limit {
                    break;
                }
                let kb = store_as_kb(&store)?;
                // `search_ranked` is the same `mae_kb::KnowledgeBase` method
                // `Editor::kb_federated_search_scoped` ranks with, so the two
                // surfaces agree on what "matches" means, including for a
                // query full of `:` and `-` (every MAE node id).
                for (id, _score) in kb.search_ranked(&query, limit) {
                    if out.len() >= limit {
                        break;
                    }
                    // A ranked id that is no longer in the KB would be a
                    // KnowledgeBase inconsistency, not a result — skip it
                    // rather than fabricating empty title/kind fields.
                    if let Some(node) = kb.get(&id) {
                        out.push(Value::list(vec![
                            Value::string(node.id.clone()),
                            Value::string(node.title.clone()),
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
