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
        _ => return None,
    };
    Some(result)
}

fn execute_collab_status(editor: &Editor) -> Result<String, String> {
    let status_str = editor.collab_status.as_str();
    let peer_count = match editor.collab_status {
        CollabStatus::Connected { peer_count } => peer_count,
        _ => 0,
    };
    let address = editor.collab_server_address.clone();
    Ok(serde_json::json!({
        "status": status_str,
        "peer_count": peer_count,
        "synced_docs": editor.collab_synced_docs,
        "server_address": address,
    })
    .to_string())
}

fn execute_collab_connect(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let address = args
        .get("address")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| editor.collab_server_address.clone());
    editor.pending_collab_intent = Some(CollabIntent::Connect {
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
    editor.pending_collab_intent = Some(CollabIntent::ShareBuffer {
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
    editor.pending_collab_intent = Some(CollabIntent::Doctor);

    let status_str = editor.collab_status.as_str();
    let connected = matches!(editor.collab_status, CollabStatus::Connected { .. });
    let peer_count = match editor.collab_status {
        CollabStatus::Connected { peer_count } => peer_count,
        _ => 0,
    };
    let address = editor.collab_server_address.clone();

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
        "detail": format!("{} documents", editor.collab_synced_docs),
    }));
    checks.push(serde_json::json!({
        "check": "authentication",
        "passed": false,
        "detail": "No authentication configured (trusted LAN mode)",
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
            &editor.pending_collab_intent,
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
}
