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
        "kb_join" => execute_kb_join(editor, &call.arguments),
        "kb_leave" => execute_kb_leave(editor, &call.arguments),
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
                "start_server": "systemctl --user start mae-state-server",
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
    let kb_name = args
        .get("kb_name")
        .and_then(|v| v.as_str())
        .unwrap_or(mae_core::KB_DEFAULT_NAME)
        .to_string();

    // Collect node IDs from the named KB.
    let node_ids: Vec<String> = if kb_name == mae_core::KB_DEFAULT_NAME || kb_name == "primary" {
        if let Some(q) = editor.kb.query_layer() {
            q.list_ids(None)
        } else {
            editor.kb.primary.list_ids(None)
        }
    } else if let Some(kb) = editor.kb.instances.get(&kb_name) {
        kb.list_ids(None)
    } else {
        return Err(format!("No KB instance named '{}'", kb_name));
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

fn execute_kb_join(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let kb_id = args
        .get("kb_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required 'kb_id' parameter")?
        .to_string();
    editor.collab.pending_intent = Some(CollabIntent::JoinKb {
        kb_id: kb_id.clone(),
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
            Some(CollabIntent::JoinKb { kb_id }) if kb_id == "work-notes"
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
