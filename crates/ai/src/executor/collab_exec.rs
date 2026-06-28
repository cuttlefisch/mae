//! Collaborative editing AI tool executor.

use mae_core::{CollabIntent, CollabStatus, Editor};
use serde_json::Value;

use crate::types::ToolCall;

pub(super) fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    let result = match call.name.as_str() {
        "collab_status" => execute_collab_status(editor),
        "collab_connect" => execute_collab_connect(editor, &call.arguments),
        "collab_share" => execute_collab_share(editor, &call.arguments),
        "collab_doctor" => execute_collab_doctor(editor),
        "collab_list" => execute_collab_list(editor),
        "collab_discover" => execute_collab_discover(editor),
        "kb_share" => execute_kb_share(editor, &call.arguments),
        "kb_share_p2p" => execute_kb_share_p2p(editor, &call.arguments),
        "kb_join_p2p" => execute_kb_join_p2p(editor, &call.arguments),
        "kb_join" => execute_kb_join(editor, &call.arguments),
        "kb_leave" => execute_kb_leave(editor, &call.arguments),
        "kb_add_member" => execute_kb_add_member(editor, &call.arguments),
        "kb_remove_member" => execute_kb_remove_member(editor, &call.arguments),
        "kb_block_member" => execute_kb_set_block(editor, &call.arguments, true),
        "kb_unblock_member" => execute_kb_set_block(editor, &call.arguments, false),
        "kb_approve" => execute_kb_approve(editor, &call.arguments),
        "kb_set_policy" => execute_kb_set_policy(editor, &call.arguments),
        "kb_set_encryption" => execute_kb_set_encryption(editor, &call.arguments),
        "kb_sharing_status" => execute_kb_sharing_status(editor, &call.arguments),
        "daemon_status" => execute_daemon_status(editor, &call.arguments),
        _ => return None,
    };
    Some(result)
}

fn execute_collab_status(editor: &Editor) -> Result<String, String> {
    let status_str = editor.collab.status.as_str();
    let peer_count = match editor.collab.status {
        CollabStatus::Connected { peer_count } => peer_count,
        _ => 0,
    };
    let address = editor.collab.server_address.clone();
    Ok(serde_json::json!({
        "status": status_str,
        "peer_count": peer_count,
        "synced_docs": editor.collab.synced_docs,
        "server_address": address,
    })
    .to_string())
}

/// AI-peer introspection: this peer's full KB-sharing state (KBs, members +
/// roles, policy, pending requests, my role/epoch, sync status). Read-only,
/// built from local replicas — the same snapshot the `*KB Sharing*` buffer and
/// the `(kb-sharing-status)` Scheme primitive show (CLAUDE.md #3 the AI is a
/// peer, #8 shared computation). Optional `kb_id` scopes to one KB.
fn execute_kb_sharing_status(editor: &Editor, args: &Value) -> Result<String, String> {
    let snapshot = editor.kb_sharing_snapshot();
    if let Some(kb_id) = args.get("kb_id").and_then(|v| v.as_str()) {
        let entry = snapshot.kbs.iter().find(|k| k.id == kb_id);
        return serde_json::to_string(&entry).map_err(|e| e.to_string());
    }
    serde_json::to_string(&snapshot).map_err(|e| e.to_string())
}

/// AI-peer introspection: daemon state + per-feature availability (ADR-035
/// capability model). The SAME data the `(daemon-status)` Scheme primitive and
/// the editor surfaces show (CLAUDE.md #3 the AI is a peer, #7 one model). An
/// optional `feature` id (e.g. "p2p-sharing") scopes to one feature's
/// availability with the why + how-to-fix.
fn execute_daemon_status(editor: &Editor, args: &Value) -> Result<String, String> {
    if let Some(feature) = args.get("feature").and_then(|v| v.as_str()) {
        return Ok(editor.feature_availability_json(feature));
    }
    Ok(editor.daemon_status_json())
}

fn execute_collab_connect(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let address = args
        .get("address")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| editor.collab.server_address.clone());
    editor.collab.pending_intent = Some(CollabIntent::Connect {
        address: address.clone(),
    });
    editor.set_status(format!("Connecting to {}...", address));
    Ok(serde_json::json!({
        "action": "connect",
        "address": address,
        "message": format!("Connection intent queued for {}", address),
    })
    .to_string())
}

fn execute_collab_share(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let buffer_name = args
        .get("buffer")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'buffer' parameter")?
        .to_string();
    editor
        .find_buffer_by_name(&buffer_name)
        .ok_or_else(|| format!("No buffer named '{}'", buffer_name))?;
    editor.collab.pending_intent = Some(CollabIntent::ShareBuffer {
        buffer_name: buffer_name.clone(),
    });
    editor.set_status(format!("Sharing buffer: {}", buffer_name));
    Ok(serde_json::json!({
        "action": "share",
        "buffer": buffer_name,
        "message": format!("Share intent queued for buffer '{}'", buffer_name),
    })
    .to_string())
}

fn execute_collab_doctor(editor: &mut Editor) -> Result<String, String> {
    // Return inline diagnostics for AI consumption (structured data, no intent buffer).
    // Also queue intent so the human gets a *Collab Doctor* buffer.
    editor.collab.pending_intent = Some(CollabIntent::Doctor);

    let status_str = editor.collab.status.as_str();
    let connected = matches!(editor.collab.status, CollabStatus::Connected { .. });
    let peer_count = match editor.collab.status {
        CollabStatus::Connected { peer_count } => peer_count,
        _ => 0,
    };
    let address = editor.collab.server_address.clone();

    let mut checks = Vec::new();
    if connected {
        checks.push(serde_json::json!({
            "check": "connection_status",
            "passed": true,
            "detail": format!("{} ({})", status_str, address),
        }));
    } else {
        checks.push(serde_json::json!({
            "check": "server_reachable",
            "passed": false,
            "detail": format!("Cannot reach {}", address),
            "remediation": {
                "start_server": "systemctl --user start mae-daemon",
                "check_listening": "ss -tlnp | grep 9473",
                "firewalld": "sudo firewall-cmd --add-port=9473/tcp --permanent && sudo firewall-cmd --reload",
                "ufw": "sudo ufw allow 9473/tcp",
                "test_connectivity": format!("nc -zv {} {}", address.split(':').next().unwrap_or("127.0.0.1"), address.split(':').next_back().unwrap_or("9473")),
            }
        }));
    }
    checks.push(serde_json::json!({
        "check": "peer_count",
        "passed": true,
        "detail": format!("{} peers", peer_count),
    }));
    checks.push(serde_json::json!({
        "check": "synced_docs",
        "passed": true,
        "detail": format!("{} documents", editor.collab.synced_docs),
    }));
    let psk_configured = !editor.collab.psk.is_empty() || !editor.collab.psk_command.is_empty();
    checks.push(serde_json::json!({
        "check": "authentication",
        "passed": psk_configured,
        "detail": if psk_configured {
            "PSK authentication configured".to_string()
        } else {
            "No authentication configured (trusted LAN mode)".to_string()
        },
    }));

    Ok(serde_json::json!({
        "status": status_str,
        "connected": connected,
        "address": address,
        "checks": checks,
        "all_passed": connected,
    })
    .to_string())
}

fn execute_collab_list(editor: &mut Editor) -> Result<String, String> {
    editor.collab.pending_intent = Some(CollabIntent::ListDocs);
    let synced: Vec<&str> = editor
        .collab
        .synced_buffers
        .iter()
        .map(|s| s.as_str())
        .collect();
    Ok(serde_json::json!({
        "action": "list_docs",
        "synced_buffers": synced,
        "synced_count": editor.collab.synced_docs,
        "message": "List docs intent queued",
    })
    .to_string())
}

fn execute_collab_discover(editor: &mut Editor) -> Result<String, String> {
    editor.collab.pending_intent = Some(CollabIntent::DiscoverPeers);
    editor.set_status("Discovering MAE peers via mDNS...".to_string());
    Ok(serde_json::json!({
        "action": "discover",
        "message": "mDNS discovery intent queued",
    })
    .to_string())
}

fn execute_kb_share(editor: &mut Editor, args: &Value) -> Result<String, String> {
    // Accept both `kb_id` and `kb_name` (the tool previously read only `kb_name`
    // and silently shared the default KB when callers passed `kb_id`).
    let kb_name = args
        .get("kb_id")
        .or_else(|| args.get("kb_name"))
        .and_then(|v| v.as_str())
        .unwrap_or(mae_core::KB_DEFAULT_NAME)
        .to_string();

    // Collect node IDs from the named KB. `instances` is keyed by UUID, so
    // resolve name→uuid via the registry first (a bare name lookup missed).
    let node_ids: Vec<String> = if kb_name == mae_core::KB_DEFAULT_NAME || kb_name == "primary" {
        if let Some(q) = editor.kb.query_layer() {
            q.list_ids(None)
        } else {
            editor.kb.primary.list_ids(None)
        }
    } else {
        let uuid = editor.kb.registry.find(&kb_name).map(|i| i.uuid.clone());
        match uuid
            .and_then(|u| editor.kb.instances.get(&u))
            .or_else(|| editor.kb.instances.get(&kb_name))
        {
            Some(kb) => kb.list_ids(None),
            None => return Err(format!("No KB instance named '{}'", kb_name)),
        }
    };

    let count = node_ids.len();
    editor.collab.pending_intent = Some(CollabIntent::ShareKb {
        kb_name: kb_name.clone(),
        node_ids,
    });
    editor.set_status(format!("Sharing KB '{}' ({} nodes)...", kb_name, count));
    Ok(serde_json::json!({
        "action": "share_kb",
        "kb_name": kb_name,
        "node_count": count,
        "message": format!("KB share intent queued for '{}' ({} nodes)", kb_name, count),
    })
    .to_string())
}

fn execute_kb_share_p2p(editor: &mut Editor, args: &Value) -> Result<String, String> {
    // Same single backend as the `kb-share-p2p` command + `(kb-share-p2p)` Scheme
    // primitive (ADR-025 §"Driving surfaces"): a synchronous daemon control call
    // that mints a shareable join "magnet link". The AI peer gets the ticket back
    // directly, so it can hand it to a collaborator with no CLI step.
    let kb_id = args
        .get("kb_id")
        .or_else(|| args.get("kb_name"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| editor.kb.active_instance_name())
        .unwrap_or_else(|| mae_core::KB_DEFAULT_NAME.to_string());

    let ticket = editor.kb.share_p2p(&kb_id)?;
    editor.set_status(format!("Minted P2P join link for '{kb_id}'"));
    Ok(serde_json::json!({
        "action": "share_kb_p2p",
        "kb_id": kb_id,
        "ticket": ticket,
        "message": format!(
            "P2P join link for '{kb_id}'. Share it with a peer; they run kb_join / `kb-join <ticket>`."
        ),
    })
    .to_string())
}

fn execute_kb_join_p2p(editor: &mut Editor, args: &Value) -> Result<String, String> {
    // Same single backend as the `kb-join-p2p` command + `(kb-join-ticket)` Scheme
    // primitive + `mae kb-join` CLI (ADR-025 §"Driving surfaces"): a synchronous
    // daemon control call that queues a P2P join from a "magnet link". The
    // background dialer then connects + pulls the KB once the owner approves.
    let ticket = args
        .get("ticket")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'ticket' parameter (a mae://join/… link)")?;
    let message = editor.kb.join_p2p(ticket)?;
    editor.set_status("Queued P2P join".to_string());
    Ok(serde_json::json!({
        "action": "join_kb_p2p",
        "message": message,
    })
    .to_string())
}

fn execute_kb_join(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let kb_id = args
        .get("kb_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'kb_id' parameter")?
        .to_string();
    let node_svs = editor.kb_join_node_svs(&kb_id);
    editor.collab.pending_intent = Some(CollabIntent::JoinKb {
        kb_id: kb_id.clone(),
        node_svs,
    });
    editor.set_status(format!("Joining shared KB '{}'...", kb_id));
    Ok(serde_json::json!({
        "action": "join_kb",
        "kb_id": kb_id,
        "message": format!("KB join intent queued for '{}'", kb_id),
    })
    .to_string())
}

fn execute_kb_leave(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let kb_id = args
        .get("kb_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'kb_id' parameter")?
        .to_string();
    editor.collab.pending_intent = Some(CollabIntent::LeaveKb {
        kb_id: kb_id.clone(),
    });
    editor.set_status(format!("Leaving shared KB '{}'...", kb_id));
    Ok(serde_json::json!({
        "action": "leave_kb",
        "kb_id": kb_id,
        "message": format!("KB leave intent queued for '{}' (local copy preserved)", kb_id),
    })
    .to_string())
}

fn execute_kb_add_member(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let kb_id = args
        .get("kb_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'kb_id' parameter")?
        .to_string();
    let member = args
        .get("member")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'member' parameter (peer fingerprint)")?
        .to_string();
    // Default matches the `:kb-member-add` command (role optional → editor).
    let role = args
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("editor")
        .to_string();
    editor.collab.pending_intent = Some(CollabIntent::KbAddMember {
        kb_id: kb_id.clone(),
        member: member.clone(),
        role: role.clone(),
    });
    editor.set_status(format!("Adding '{member}' to KB '{kb_id}' as {role}..."));
    Ok(serde_json::json!({
        "action": "kb_add_member",
        "kb_id": kb_id,
        "member": member,
        "role": role,
        "message": format!("Membership change queued: '{member}' → {role} on KB '{kb_id}' (owner-only; applied by the daemon)"),
    })
    .to_string())
}

fn execute_kb_remove_member(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let kb_id = args
        .get("kb_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'kb_id' parameter")?
        .to_string();
    let member = args
        .get("member")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'member' parameter (peer fingerprint)")?
        .to_string();
    editor.collab.pending_intent = Some(CollabIntent::KbRemoveMember {
        kb_id: kb_id.clone(),
        member: member.clone(),
    });
    editor.set_status(format!("Removing '{member}' from KB '{kb_id}'..."));
    Ok(serde_json::json!({
        "action": "kb_remove_member",
        "kb_id": kb_id,
        "member": member,
        "message": format!("Membership removal queued: '{member}' from KB '{kb_id}' (owner-only; applied by the daemon)"),
    })
    .to_string())
}

/// Add/remove a principal on a KB's LOCAL self-protection blocklist (ADR-039 A2, #162).
/// Local-only to this daemon (never propagated); NOT owner-gated. `block` selects
/// kb_block_member vs kb_unblock_member.
fn execute_kb_set_block(editor: &mut Editor, args: &Value, block: bool) -> Result<String, String> {
    let kb_id = args
        .get("kb_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'kb_id' parameter")?
        .to_string();
    let member = args
        .get("member")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'member' parameter (peer fingerprint)")?
        .to_string();
    editor.collab.pending_intent = Some(CollabIntent::KbSetBlock {
        kb_id: kb_id.clone(),
        member: member.clone(),
        blocked: block,
    });
    let verb = if block { "Blocking" } else { "Unblocking" };
    editor.set_status(format!(
        "{verb} '{member}' on KB '{kb_id}' (local self-protection)..."
    ));
    Ok(serde_json::json!({
        "action": if block { "kb_block_member" } else { "kb_unblock_member" },
        "kb_id": kb_id,
        "member": member,
        "blocked": block,
        "message": format!(
            "Local {} queued: '{member}' on KB '{kb_id}' (LOCAL-only self-protection — not propagated to peers; applied by the daemon)",
            if block { "block" } else { "unblock" }
        ),
    })
    .to_string())
}

/// Approve a pending join request as `role` (owner-only, ADR-018). Use
/// `kb_sharing_status` first to read the pending requests' fingerprints.
fn execute_kb_approve(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let kb_id = args
        .get("kb_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'kb_id' parameter")?
        .to_string();
    let principal = args
        .get("member")
        .or_else(|| args.get("principal"))
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'member' parameter (pending peer fingerprint)")?
        .to_string();
    let role = args
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("editor")
        .to_string();
    editor.collab.pending_intent = Some(CollabIntent::KbApprove {
        kb_id: kb_id.clone(),
        principal: principal.clone(),
        role: role.clone(),
    });
    editor.set_status(format!(
        "Approving '{principal}' for KB '{kb_id}' as {role}..."
    ));
    Ok(serde_json::json!({
        "action": "kb_approve",
        "kb_id": kb_id,
        "member": principal,
        "role": role,
        "message": format!("Approval queued: '{principal}' → {role} on KB '{kb_id}' (owner-only; applied by the daemon)"),
    })
    .to_string())
}

/// Set a KB's join policy: restrictive | invite | permissive (owner-only, ADR-018).
fn execute_kb_set_policy(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let kb_id = args
        .get("kb_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'kb_id' parameter")?
        .to_string();
    let policy = args
        .get("policy")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'policy' parameter (restrictive|invite|permissive)")?
        .to_string();
    if !matches!(policy.as_str(), "restrictive" | "invite" | "permissive") {
        return Err(format!(
            "Invalid policy '{policy}' (expected restrictive, invite, or permissive)"
        ));
    }
    editor.collab.pending_intent = Some(CollabIntent::KbSetPolicy {
        kb_id: kb_id.clone(),
        policy: policy.clone(),
    });
    editor.set_status(format!("Setting KB '{kb_id}' join policy to {policy}..."));
    Ok(serde_json::json!({
        "action": "kb_set_policy",
        "kb_id": kb_id,
        "policy": policy,
        "message": format!("Policy change queued: KB '{kb_id}' → {policy} (owner-only; applied by the daemon)"),
    })
    .to_string())
}

fn execute_kb_set_encryption(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let kb_id = args
        .get("kb_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'kb_id' parameter")?
        .to_string();
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("e2e")
        .to_string();
    if mode != "e2e" {
        return Err(format!(
            "Invalid mode '{mode}' (only 'e2e' is supported; encryption is one-way)"
        ));
    }
    editor.collab.pending_intent = Some(CollabIntent::KbSetEncryption {
        kb_id: kb_id.clone(),
        mode: mode.clone(),
    });
    editor.set_status(format!("Enabling E2E encryption on KB '{kb_id}'..."));
    Ok(serde_json::json!({
        "action": "kb_set_encryption",
        "kb_id": kb_id,
        "mode": mode,
        "message": format!("E2E encryption queued for KB '{kb_id}' (owner-only, one-way; the owner generates + distributes the content key, the daemon stays key-blind)"),
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCall;
    use serde_json::json;

    fn make_call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: name.to_string(),
            arguments: args,
        }
    }

    #[test]
    fn collab_status_returns_off_by_default() {
        let mut editor = Editor::new();
        let call = make_call("collab_status", json!({}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "off");
    }

    #[test]
    fn collab_connect_sets_intent() {
        let mut editor = Editor::new();
        let call = make_call("collab_connect", json!({"address": "10.0.0.5:9473"}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["address"], "10.0.0.5:9473");
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::Connect { address }) if address == "10.0.0.5:9473"
        ));
    }

    #[test]
    fn collab_share_validates_buffer() {
        let mut editor = Editor::new();
        let call = make_call("collab_share", json!({"buffer": "nonexistent"}));
        let result = dispatch(&mut editor, &call).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn unknown_tool_returns_none() {
        let mut editor = Editor::new();
        let call = make_call("unknown_tool", json!({}));
        assert!(dispatch(&mut editor, &call).is_none());
    }

    #[test]
    fn kb_approve_and_set_policy_queue_intents() {
        let mut editor = Editor::new();
        let r = dispatch(
            &mut editor,
            &make_call(
                "kb_approve",
                json!({"kb_id": "team", "member": "SHA256:carol"}),
            ),
        )
        .unwrap()
        .unwrap();
        let parsed: Value = serde_json::from_str(&r).unwrap();
        assert_eq!(parsed["action"], "kb_approve");
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::KbApprove { kb_id, principal, role })
                if kb_id == "team" && principal == "SHA256:carol" && role == "editor"
        ));

        dispatch(
            &mut editor,
            &make_call(
                "kb_set_policy",
                json!({"kb_id": "team", "policy": "permissive"}),
            ),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::KbSetPolicy { kb_id, policy })
                if kb_id == "team" && policy == "permissive"
        ));

        // An invalid policy is rejected.
        let bad = dispatch(
            &mut editor,
            &make_call("kb_set_policy", json!({"kb_id": "team", "policy": "bogus"})),
        )
        .unwrap();
        assert!(bad.is_err());
    }

    #[test]
    fn kb_sharing_status_returns_snapshot_json() {
        // P0: the AI peer introspects KB-sharing state via the same snapshot the
        // human sees. With no shared KBs the snapshot is well-formed and empty.
        let mut editor = Editor::new();
        let result = dispatch(&mut editor, &make_call("kb_sharing_status", json!({})))
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert!(parsed.get("connection").is_some());
        assert!(parsed["kbs"].as_array().unwrap().is_empty());

        // Seed an owner replica → the tool reports the KB with the owner member.
        let coll = mae_sync::kb::KbCollectionDoc::new_owned("Team", "mefp", "me");
        editor.collab.local_fingerprint = "mefp".to_string();
        editor
            .collab
            .kb_collection_state
            .insert("team".to_string(), coll.encode_state());

        let scoped = dispatch(
            &mut editor,
            &make_call("kb_sharing_status", json!({"kb_id": "team"})),
        )
        .unwrap()
        .unwrap();
        let kb: Value = serde_json::from_str(&scoped).unwrap();
        assert_eq!(kb["id"], "team");
        assert_eq!(kb["role_of_me"], "owner");
        assert!(kb["is_owner"].as_bool().unwrap());
    }

    #[test]
    fn collab_list_sets_intent() {
        let mut editor = Editor::new();
        let call = make_call("collab_list", json!({}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["action"], "list_docs");
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::ListDocs)
        ));
    }

    #[test]
    fn collab_discover_sets_intent() {
        let mut editor = Editor::new();
        let call = make_call("collab_discover", json!({}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["action"], "discover");
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::DiscoverPeers)
        ));
    }

    #[test]
    fn kb_share_default_name() {
        let mut editor = Editor::new();
        let call = make_call("kb_share", json!({}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["action"], "share_kb");
        assert_eq!(parsed["kb_name"], "default");
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::ShareKb { kb_name, .. }) if kb_name == "default"
        ));
    }

    #[test]
    fn kb_share_with_name() {
        let mut editor = Editor::new();
        let call = make_call("kb_share", json!({"kb_name": "primary"}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["kb_name"], "primary");
    }

    #[test]
    fn kb_share_unknown_instance_errors() {
        let mut editor = Editor::new();
        let call = make_call("kb_share", json!({"kb_name": "nonexistent"}));
        let result = dispatch(&mut editor, &call).unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No KB instance"));
    }

    #[test]
    fn kb_join_requires_kb_id() {
        let mut editor = Editor::new();
        let call = make_call("kb_join", json!({}));
        let result = dispatch(&mut editor, &call).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn kb_join_sets_intent() {
        let mut editor = Editor::new();
        let call = make_call("kb_join", json!({"kb_id": "work-notes"}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["kb_id"], "work-notes");
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::JoinKb { kb_id, .. }) if kb_id == "work-notes"
        ));
    }

    #[test]
    fn kb_leave_requires_kb_id() {
        let mut editor = Editor::new();
        let call = make_call("kb_leave", json!({}));
        let result = dispatch(&mut editor, &call).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn kb_leave_sets_intent() {
        let mut editor = Editor::new();
        let call = make_call("kb_leave", json!({"kb_id": "work-notes"}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["action"], "leave_kb");
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::LeaveKb { kb_id }) if kb_id == "work-notes"
        ));
    }

    #[test]
    fn kb_add_member_sets_intent_with_role() {
        let mut editor = Editor::new();
        let call = make_call(
            "kb_add_member",
            json!({"kb_id": "collabtest", "member": "SHA256:bob", "role": "viewer"}),
        );
        let parsed: Value =
            serde_json::from_str(&dispatch(&mut editor, &call).unwrap().unwrap()).unwrap();
        assert_eq!(parsed["role"], "viewer");
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::KbAddMember { kb_id, member, role })
                if kb_id == "collabtest" && member == "SHA256:bob" && role == "viewer"
        ));
    }

    #[test]
    fn kb_add_member_defaults_role_to_editor() {
        let mut editor = Editor::new();
        let call = make_call(
            "kb_add_member",
            json!({"kb_id": "collabtest", "member": "SHA256:bob"}),
        );
        dispatch(&mut editor, &call).unwrap().unwrap();
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::KbAddMember { role, .. }) if role == "editor"
        ));
    }

    #[test]
    fn kb_add_member_requires_member() {
        let mut editor = Editor::new();
        let call = make_call("kb_add_member", json!({"kb_id": "collabtest"}));
        assert!(dispatch(&mut editor, &call).unwrap().is_err());
    }

    #[test]
    fn kb_remove_member_sets_intent() {
        let mut editor = Editor::new();
        let call = make_call(
            "kb_remove_member",
            json!({"kb_id": "collabtest", "member": "SHA256:bob"}),
        );
        dispatch(&mut editor, &call).unwrap().unwrap();
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::KbRemoveMember { kb_id, member })
                if kb_id == "collabtest" && member == "SHA256:bob"
        ));
    }

    #[test]
    fn collab_doctor_psk_configured() {
        let mut editor = Editor::new();
        editor.collab.psk = "secret-key".to_string();
        let call = make_call("collab_doctor", json!({}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        let auth_check = parsed["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["check"] == "authentication")
            .unwrap();
        assert_eq!(auth_check["passed"], true);
        assert!(auth_check["detail"].as_str().unwrap().contains("PSK"));
    }

    #[test]
    fn collab_doctor_psk_unconfigured() {
        let mut editor = Editor::new();
        let call = make_call("collab_doctor", json!({}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        let auth_check = parsed["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["check"] == "authentication")
            .unwrap();
        assert_eq!(auth_check["passed"], false);
    }

    #[test]
    fn collab_doctor_psk_command_configured() {
        let mut editor = Editor::new();
        editor.collab.psk_command = "pass show mae/psk".to_string();
        let call = make_call("collab_doctor", json!({}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        let auth_check = parsed["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["check"] == "authentication")
            .unwrap();
        assert_eq!(auth_check["passed"], true);
    }

    #[test]
    fn all_collab_kb_tools_defined() {
        let tools = crate::tools::ai_specific_tools(&mae_core::OptionRegistry::new());
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        for expected in [
            "collab_status",
            "collab_connect",
            "collab_share",
            "collab_doctor",
            "collab_list",
            "collab_discover",
            "kb_share",
            "kb_join",
            "kb_leave",
        ] {
            assert!(
                names.contains(&expected),
                "missing tool definition: {}",
                expected
            );
        }
    }
}
