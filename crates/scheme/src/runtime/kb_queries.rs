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
