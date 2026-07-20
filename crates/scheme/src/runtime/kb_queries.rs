//! Read-only KB query functions (links, meta-node members, relationship
//! types) exposed to Scheme.
//!
//! Split out of `runtime.rs`'s `SchemeRuntime::new()` (CLAUDE.md architecture
//! debt reduction pass) — pure code motion, no behavior change.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi::{arg_int, arg_string};
use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// Register read-only KB query primitives.
pub(super) fn register_kb_query_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    // --- Read-only KB query functions ---

    // (kb-links-from ID) → list of (target rel-type display)
    let s = shared.clone();
    vm.register_fn(
        "kb-links-from",
        "Return outgoing typed links from a KB node",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-links-from")?;
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                match store.links_from(&id) {
                    Ok(links) => Ok(Value::list(
                        links
                            .into_iter()
                            .map(|l| {
                                Value::list(vec![
                                    Value::string(l.dst),
                                    Value::string(l.rel_type),
                                    Value::string(l.display.unwrap_or_default()),
                                ])
                            })
                            .collect::<Vec<_>>(),
                    )),
                    Err(e) => Err(LispError::internal(format!("kb-links-from: {}", e))),
                }
            } else {
                Ok(Value::list(vec![]))
            }
        },
    );

    // (kb-links-to ID) → list of (source rel-type display)
    let s = shared.clone();
    vm.register_fn(
        "kb-links-to",
        "Return incoming typed links to a KB node",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-links-to")?;
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                match store.links_to(&id) {
                    Ok(links) => Ok(Value::list(
                        links
                            .into_iter()
                            .map(|l| {
                                Value::list(vec![
                                    Value::string(l.src),
                                    Value::string(l.rel_type),
                                    Value::string(l.display.unwrap_or_default()),
                                ])
                            })
                            .collect::<Vec<_>>(),
                    )),
                    Err(e) => Err(LispError::internal(format!("kb-links-to: {}", e))),
                }
            } else {
                Ok(Value::list(vec![]))
            }
        },
    );

    // (kb-links-typed ID REL-TYPE) → list of (target display)
    let s = shared.clone();
    vm.register_fn(
        "kb-links-typed",
        "Return links of a specific relationship type from a node",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-links-typed")?;
            let rel_type = arg_string(args, 1, "kb-links-typed")?;
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                match store.links_typed(&id, &rel_type) {
                    Ok(links) => Ok(Value::list(
                        links
                            .into_iter()
                            .map(|l| {
                                Value::list(vec![
                                    Value::string(l.dst),
                                    Value::string(l.display.unwrap_or_default()),
                                ])
                            })
                            .collect::<Vec<_>>(),
                    )),
                    Err(e) => Err(LispError::internal(format!("kb-links-typed: {}", e))),
                }
            } else {
                Ok(Value::list(vec![]))
            }
        },
    );

    // (kb-meta-members ID) → list of (member-id role order)
    let s = shared.clone();
    vm.register_fn(
        "kb-meta-members",
        "Return members of a meta-node",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-meta-members")?;
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                match store.meta_members(&id) {
                    Ok(members) => Ok(Value::list(
                        members
                            .into_iter()
                            .map(|m| {
                                Value::list(vec![
                                    Value::string(m.member_id),
                                    Value::string(m.role),
                                    Value::Int(m.position as i64),
                                ])
                            })
                            .collect::<Vec<_>>(),
                    )),
                    Err(e) => Err(LispError::internal(format!("kb-meta-members: {}", e))),
                }
            } else {
                Ok(Value::list(vec![]))
            }
        },
    );

    // (kb-rel-types) → list of type names
    let s = shared.clone();
    vm.register_fn(
        "kb-rel-types",
        "Return all known relationship type names",
        Arity::Fixed(0),
        move |_args: &[Value]| {
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                match store.known_rel_types() {
                    Ok(types) => Ok(Value::list(
                        types.into_iter().map(Value::string).collect::<Vec<_>>(),
                    )),
                    Err(e) => Err(LispError::internal(format!("kb-rel-types: {}", e))),
                }
            } else {
                Ok(Value::list(vec![]))
            }
        },
    );

    // (kb-get-block ID INDEX) → block text or #f
    let s = shared.clone();
    vm.register_fn(
        "kb-get-block",
        "Get a specific block from a KB node by index",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-get-block")?;
            let index = arg_int(args, 1, "kb-get-block")? as usize;
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                match store.get_block(&id, index) {
                    Ok(Some(block)) => Ok(Value::string(block.content)),
                    Ok(None) => Ok(Value::Bool(false)),
                    Err(_) => Ok(Value::Bool(false)),
                }
            } else {
                Ok(Value::Bool(false))
            }
        },
    );

    // (kb-block-count ID) → number of blocks
    let s = shared.clone();
    vm.register_fn(
        "kb-block-count",
        "Return the number of blocks in a KB node",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-block-count")?;
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                match store.get_blocks(&id) {
                    Ok(blocks) => Ok(Value::Int(blocks.len() as i64)),
                    Err(_) => Ok(Value::Int(0)),
                }
            } else {
                Ok(Value::Int(0))
            }
        },
    );

    // --- Graph-native KB query functions ---
    //
    // These four mirror the `kb_graph`/`kb_neighborhood`/`kb_related`/
    // `kb_shortest_path` MCP tools (`crates/ai/src/tools/kb_tools.rs`,
    // executors in `crates/ai/src/tool_impls/kb.rs`), closing a real
    // human/AI parity gap: those MCP tools previously had zero Scheme
    // primitive counterparts (CLAUDE.md principle #3). Like every other
    // primitive in this file, they read the primary KB's durable store only
    // (`SharedState::kb_store`, synced 1:1 from `Editor.kb.store`) — NOT the
    // federated query layer the MCP executors prefer when available, so
    // results here are scoped to the primary KB even when other KB
    // instances are registered. `kb-graph` and `kb-related` share their
    // core walk/ranking algorithm with the MCP executors via
    // `mae_kb::graph_query` (see that module's docs) rather than
    // reimplementing it; `kb-neighborhood` and `kb-shortest-path` call
    // `KbStore` trait methods directly (no factoring needed — the MCP
    // executors already do the same thing).

    // (kb-graph ID [DEPTH]) → (root depth nodes edges)
    // nodes: list of (id title-or-#f kind-or-#f hop missing?)
    // edges: list of (src . dst)
    let s = shared.clone();
    vm.register_fn(
        "kb-graph",
        "BFS neighborhood around a seed node (primary KB only), up to DEPTH hops (default 1, max 3). Returns (root depth nodes edges): each node is (id title kind hop missing?) — title/kind are #f when missing? is #t (a dangling link target); each edge is (src . dst).",
        Arity::Variadic(1),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-graph")?;
            let depth = if args.len() > 1 {
                (arg_int(args, 1, "kb-graph")? as usize).min(3)
            } else {
                1
            };
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                let backend = mae_kb::graph_query::KbStoreBackend(store.as_ref());
                match mae_kb::graph_query::bfs_neighborhood(&backend, &id, depth) {
                    Ok(result) => {
                        let nodes = Value::list(
                            result
                                .nodes
                                .into_iter()
                                .map(|n| {
                                    Value::list(vec![
                                        Value::string(n.id),
                                        n.title
                                            .map(Value::string)
                                            .unwrap_or(Value::Bool(false)),
                                        n.kind.map(Value::string).unwrap_or(Value::Bool(false)),
                                        Value::Int(n.hop as i64),
                                        Value::Bool(n.missing),
                                    ])
                                })
                                .collect::<Vec<_>>(),
                        );
                        let edges = Value::list(
                            result
                                .edges
                                .into_iter()
                                .map(|(src, dst)| {
                                    Value::cons(Value::string(src), Value::string(dst))
                                })
                                .collect::<Vec<_>>(),
                        );
                        Ok(Value::list(vec![
                            Value::string(result.root),
                            Value::Int(result.depth as i64),
                            nodes,
                            edges,
                        ]))
                    }
                    Err(e) => Err(LispError::internal(format!("kb-graph: {}", e))),
                }
            } else {
                Ok(Value::list(vec![]))
            }
        },
    );

    // (kb-neighborhood ID [DEPTH]) → (root depth nodes edges)
    // nodes: list of (id . title); edges: list of (src dst rel-type)
    let s = shared.clone();
    vm.register_fn(
        "kb-neighborhood",
        "Graph neighborhood around a seed node from the persistent store, up to DEPTH hops (default 2, max 5). Returns (root depth nodes edges): each node is (id . title), each edge is (src dst rel-type). Requires CozoDB backend.",
        Arity::Variadic(1),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-neighborhood")?;
            let depth = if args.len() > 1 {
                (arg_int(args, 1, "kb-neighborhood")? as u32).min(5)
            } else {
                2
            };
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                match store.neighborhood(&id, depth) {
                    Ok(subgraph) => {
                        let nodes = Value::list(
                            subgraph
                                .nodes
                                .into_iter()
                                .map(|(nid, title)| {
                                    Value::cons(Value::string(nid), Value::string(title))
                                })
                                .collect::<Vec<_>>(),
                        );
                        let edges = Value::list(
                            subgraph
                                .edges
                                .into_iter()
                                .map(|(src, dst, rel)| {
                                    Value::list(vec![
                                        Value::string(src),
                                        Value::string(dst),
                                        Value::string(rel),
                                    ])
                                })
                                .collect::<Vec<_>>(),
                        );
                        Ok(Value::list(vec![
                            Value::string(id),
                            Value::Int(depth as i64),
                            nodes,
                            edges,
                        ]))
                    }
                    Err(e) => Err(LispError::internal(format!("kb-neighborhood: {}", e))),
                }
            } else {
                Ok(Value::list(vec![]))
            }
        },
    );

    // (kb-related ID [LIMIT]) → list of (id title kind score)
    let s = shared.clone();
    vm.register_fn(
        "kb-related",
        "Nodes structurally related to ID (primary KB only) — co-citation / bibliographic coupling / shared tags, distinct from lexical search (kb-search). Returns a list of (id title kind score) sorted by relatedness, capped to LIMIT (default 10).",
        Arity::Variadic(1),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-related")?;
            let limit = if args.len() > 1 {
                arg_int(args, 1, "kb-related")? as usize
            } else {
                10
            };
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                let backend = mae_kb::graph_query::KbStoreRelatedBackend(store.as_ref());
                let items = mae_kb::graph_query::related_enriched(&backend, &id, limit);
                Ok(Value::list(
                    items
                        .into_iter()
                        .map(|it| {
                            Value::list(vec![
                                Value::string(it.id),
                                Value::string(it.title),
                                Value::string(it.kind),
                                Value::Float(it.score),
                            ])
                        })
                        .collect::<Vec<_>>(),
                ))
            } else {
                Ok(Value::list(vec![]))
            }
        },
    );

    // (kb-shortest-path FROM TO) → list of node ids
    //
    // NOT a real shortest path: `KbStore::shortest_path` (CozoDB backend) is
    // a Datalog REACHABILITY check capped at depth 10 — it returns only
    // `(FROM TO)` when a path of length <= 10 exists, or the empty list
    // otherwise. It never reconstructs the actual intermediate hops
    // ("full path tracking requires list operations that vary across
    // CozoDB versions", per its own implementation comment). See
    // `shared/kb/src/cozo_store/graph.rs`'s `CozoKbStore::shortest_path`.
    let s = shared.clone();
    vm.register_fn(
        "kb-shortest-path",
        "Reachability check between FROM and TO — NOT a real shortest path. A Datalog BFS capped at depth 10; returns (FROM TO) if a path of that length or shorter exists, else '() (empty list). Does not reconstruct intermediate hops. Requires CozoDB backend.",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let from = arg_string(args, 0, "kb-shortest-path")?;
            let to = arg_string(args, 1, "kb-shortest-path")?;
            let state = s.lock();
            if let Some(ref store) = state.kb_store {
                match store.shortest_path(&from, &to) {
                    Ok(path) => Ok(Value::list(
                        path.into_iter().map(Value::string).collect::<Vec<_>>(),
                    )),
                    Err(e) => Err(LispError::internal(format!("kb-shortest-path: {}", e))),
                }
            } else {
                Ok(Value::list(vec![]))
            }
        },
    );

    // (deprecate-function! OLD-NAME NEW-NAME SINCE-VERSION)
    let s = shared.clone();
    vm.register_fn(
        "deprecate-function!",
        "Register a deprecation warning",
        Arity::Fixed(3),
        move |args: &[Value]| {
            let old_name = arg_string(args, 0, "deprecate-function!")?;
            let new_name = arg_string(args, 1, "deprecate-function!")?;
            let since = arg_string(args, 2, "deprecate-function!")?;
            s.lock()
                .deprecated_functions
                .insert(old_name, (new_name, since));
            Ok(Value::Void)
        },
    );

    // (register-ai-tool! NAME DESCRIPTION HANDLER-FN PERMISSION)
    let s = shared.clone();
    vm.register_fn(
        "register-ai-tool!",
        "Register an AI tool from Scheme",
        Arity::Fixed(4),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "register-ai-tool!")?;
            let desc = arg_string(args, 1, "register-ai-tool!")?;
            let handler = arg_string(args, 2, "register-ai-tool!")?;
            let perm = arg_string(args, 3, "register-ai-tool!")?;
            // Fail loud, at registration time (where the Scheme author will
            // actually see it), rather than letting an unrecognized value
            // through to `scheme_tools_to_definitions` (crates/ai), which
            // used to silently downgrade any unrecognized string to the
            // Write tier — a typo'd permission would then grant MORE access
            // than intended instead of failing safe.
            if !matches!(perm.as_str(), "read" | "readonly" | "write" | "shell" | "privileged") {
                return Err(LispError::user(
                    format!(
                        "register-ai-tool!: invalid permission '{}' (expected: read, readonly, write, shell, privileged)",
                        perm
                    ),
                    vec![],
                ));
            }
            let mut st = s.lock();
            let params = st.pending_ai_tool_params.remove(&name).unwrap_or_default();
            let required = st
                .pending_ai_tool_required
                .remove(&name)
                .unwrap_or_default();
            st.pending_ai_tools.push(mae_core::SchemeToolDef {
                name,
                description: desc,
                params,
                required,
                handler_fn: handler,
                permission: perm,
            });
            Ok(Value::Void)
        },
    );

    // (ai-tool-param! TOOL-NAME PARAM-NAME PARAM-TYPE DESCRIPTION)
    let s = shared.clone();
    vm.register_fn(
        "ai-tool-param!",
        "Add a parameter to an AI tool",
        Arity::Fixed(4),
        move |args: &[Value]| {
            let tool = arg_string(args, 0, "ai-tool-param!")?;
            let pname = arg_string(args, 1, "ai-tool-param!")?;
            let ptype = arg_string(args, 2, "ai-tool-param!")?;
            let pdesc = arg_string(args, 3, "ai-tool-param!")?;
            s.lock()
                .pending_ai_tool_params
                .entry(tool)
                .or_default()
                .push((pname, ptype, pdesc));
            Ok(Value::Void)
        },
    );

    // (ai-tool-require! TOOL-NAME PARAM-NAME)
    let s = shared.clone();
    vm.register_fn(
        "ai-tool-require!",
        "Mark an AI tool parameter as required",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let tool = arg_string(args, 0, "ai-tool-require!")?;
            let pname = arg_string(args, 1, "ai-tool-require!")?;
            s.lock()
                .pending_ai_tool_required
                .entry(tool)
                .or_default()
                .push(pname);
            Ok(Value::Void)
        },
    );

    // (register-splash-art! NAME ART-STRING)
    let s = shared.clone();
    vm.register_fn(
        "register-splash-art!",
        "Register custom splash art",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "register-splash-art!")?;
            let art = arg_string(args, 1, "register-splash-art!")?;
            s.lock().pending_splash_arts.push((name, art, None));
            Ok(Value::Void)
        },
    );

    // (register-splash-art-image! NAME IMAGE-PATH)
    let s = shared.clone();
    vm.register_fn(
        "register-splash-art-image!",
        "Register splash art image",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "register-splash-art-image!")?;
            let path = arg_string(args, 1, "register-splash-art-image!")?;
            let mut st = s.lock();
            let resolved = {
                let p = PathBuf::from(&path);
                if p.is_relative() {
                    if let Some(ref dir) = st.current_module_dir {
                        dir.join(&p)
                    } else {
                        p
                    }
                } else {
                    p
                }
            };
            st.pending_splash_arts
                .push((name, String::new(), Some(resolved)));
            Ok(Value::Void)
        },
    );
}
