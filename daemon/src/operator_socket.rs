//! The operator's socket: `mae-daemon kb member add|remove|list` against a
//! RUNNING daemon, so a membership change (a directory sync, an admin's
//! command) no longer needs the daemon stopped and every editor disconnected.
//!
//! **Who can reach it.** A Unix socket in a private directory of the daemon's
//! data directory: the directory `0700`, the socket `0600`, so only the
//! account the daemon runs as, and never another for even an instant (the
//! socket is born under the process umask before it is narrowed). It serves the membership
//! commands and nothing else. The KB socket the editor talks to does not serve
//! them, and no MCP tool or Scheme primitive calls them (CLAUDE.md principle
//! #16: a control that bounds who may read a KB is not on the agent's
//! surface).
//!
//! **What a change does.** The same `operator_change_membership` the offline
//! command runs, under the same checks (the daemon owns the KB, the store
//! signs as it, a real key fingerprint, not the owner), signed as the daemon,
//! with the collection update broadcast to every session subscribed to it, so
//! connected editors see it and a removed member's next request is refused.
//!
//! @ai-caution: [security] Never add a method here that an agent should not
//! have, and never serve these methods on any other socket or listener.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mae_mcp::broadcast::SharedBroadcaster;
use serde_json::{json, Value};
use tokio::io::BufReader;
use tokio::net::{UnixListener, UnixStream};

use crate::collab_handler::{
    operator_change_membership_live, operator_list_members, OperatorMemberOp,
};
use crate::doc_store::DocStore;
use mae_sync::kb::Role;

/// The private directory, in the daemon's data directory, that holds it.
pub const DIR_NAME: &str = "operator";
/// The socket's file name in that directory.
pub const FILE_NAME: &str = "operator.sock";

/// The largest request the socket reads: a command is a few hundred bytes.
const MAX_REQUEST: usize = 64 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Where a daemon with this data directory listens.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR_NAME).join(FILE_NAME)
}

/// `dir`, created `0700` or narrowed to it; refused if it is a symlink or not
/// a directory, so the socket's directory is never one another account chose.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match std::fs::symlink_metadata(dir) {
        Ok(m) if !m.file_type().is_dir() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} is not a directory", dir.display()),
            ))
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        Err(e) => return Err(e),
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Bind the socket at `path`, owner-only. A leftover file is removed only when
/// nothing answers on it; a live daemon's socket is never taken over.
pub async fn bind(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        private_dir(dir)?;
    }
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("{} is in use by a running daemon", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Serve `listener` until the task is dropped: one request per connection.
pub async fn serve(
    listener: UnixListener,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                // Out of descriptors, say: pause rather than spin.
                tracing::warn!(error = %e, "operator socket: accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let (store, bc) = (Arc::clone(&doc_store), broadcaster.clone());
        tokio::spawn(async move {
            if let Err(e) = answer(stream, &store, &bc).await {
                tracing::warn!(error = %e, "operator socket: a request failed");
            }
        });
    }
}

async fn answer(
    stream: UnixStream,
    store: &DocStore,
    bc: &SharedBroadcaster,
) -> std::io::Result<()> {
    let (r, mut w) = stream.into_split();
    let mut r = BufReader::new(r);
    let Some(body) = tokio::time::timeout(
        IO_TIMEOUT,
        mae_mcp::read_message_limited(&mut r, MAX_REQUEST),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "request"))??
    else {
        return Ok(());
    };
    let reply = match serde_json::from_str::<Value>(&body) {
        Ok(req) => {
            let id = req.get("id").cloned().unwrap_or(Value::Null);
            match handle(store, bc, &req).await {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err(message) => json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32603, "message": message}}),
            }
        }
        Err(_) => json!({"jsonrpc": "2.0", "id": null,
            "error": {"code": -32700, "message": "not JSON"}}),
    };
    mae_mcp::write_framed(&mut w, reply.to_string().as_bytes(), IO_TIMEOUT).await
}

/// One operator command. The identity it signs as is the daemon's own.
async fn handle(store: &DocStore, bc: &SharedBroadcaster, req: &Value) -> Result<Value, String> {
    let method = req["method"].as_str().unwrap_or_default();
    let p = &req["params"];
    let text = |k: &str| p[k].as_str().map(str::to_string);
    let kb_id = text("kb_id").ok_or("kb_id is required")?;
    let identity = || {
        store
            .signer()
            .cloned()
            .ok_or("this daemon has no signing identity (collab.auth.mode = \"key\")")
    };
    match method {
        "member.add" => {
            let member = text("member").ok_or("member is required")?;
            let role = match p["role"].as_str() {
                Some("viewer") => Role::Viewer,
                Some("editor") => Role::Editor,
                _ => return Err("role must be viewer or editor".into()),
            };
            let label = text("label").unwrap_or_default();
            let op = OperatorMemberOp::Add {
                member: &member,
                role,
                label: &label,
            };
            let id: Arc<mae_mcp::identity::Identity> = identity()?;
            operator_change_membership_live(store, bc, &id, &kb_id, op).await
        }
        "member.remove" => {
            let member = text("member").ok_or("member is required")?;
            let op = OperatorMemberOp::Remove { member: &member };
            let id: Arc<mae_mcp::identity::Identity> = identity()?;
            operator_change_membership_live(store, bc, &id, &kb_id, op).await
        }
        "member.list" => {
            let (owner, members) = operator_list_members(store, &kb_id).await?;
            let members: Vec<Value> = members
                .into_iter()
                .map(|(fp, role)| json!({"member": fp, "role": role}))
                .collect();
            Ok(json!({"owner": owner, "members": members}))
        }
        other => Err(format!("unknown operator method '{other}'")),
    }
}

/// Send one command to the daemon listening at `path`: `Ok(None)` only when
/// no daemon is there (no socket, or nothing accepting on it), so the caller
/// can work offline. Any other failure (permission, a full backlog) is an
/// error, never a silent fallback.
pub async fn request(path: &Path, method: &str, params: Value) -> Result<Option<Value>, String> {
    let stream = match UnixStream::connect(path).await {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None)
        }
        Err(e) => return Err(format!("operator socket {}: {e}", path.display())),
    };
    let (r, mut w) = stream.into_split();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    mae_mcp::write_framed(&mut w, body.to_string().as_bytes(), IO_TIMEOUT)
        .await
        .map_err(|e| format!("operator socket: {e}"))?;
    let mut r = BufReader::new(r);
    let reply = tokio::time::timeout(
        IO_TIMEOUT,
        mae_mcp::read_message_limited(&mut r, MAX_REQUEST),
    )
    .await
    .map_err(|_| "operator socket: no answer".to_string())?
    .map_err(|e| format!("operator socket: {e}"))?
    .ok_or("operator socket: closed without an answer")?;
    let v: Value = serde_json::from_str(&reply).map_err(|e| format!("operator socket: {e}"))?;
    match v.get("error") {
        Some(e) => Err(e["message"].as_str().unwrap_or("refused").to_string()),
        None => Ok(Some(v["result"].clone())),
    }
}
