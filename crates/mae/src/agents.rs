//! Agent bootstrap system: auto-configure MCP clients in MAE's terminal.
//!
//! When a shell terminal spawns, MAE writes a `.mcp.json` to the project
//! root so that Claude Code (and future MCP-aware agents) automatically
//! discover the editor's tool surface — zero manual `claude mcp add`.
//!
//! The key insight: `MAE_MCP_SOCKET` is inherited from the PTY environment
//! (already injected by the shell spawn code), NOT placed in `.mcp.json`.
//! This makes the file static (no PID) and reusable across MAE restarts.

use std::io;
use std::path::{Path, PathBuf};

/// Describes a supported AI agent and how to bootstrap its MCP config.
pub struct AgentDef {
    pub name: &'static str,
    /// Binary name for future agent detection (e.g. `which claude`).
    #[allow(dead_code)]
    pub binary: &'static str,
    pub description: &'static str,
    pub strategy: BootstrapStrategy,
}

/// How to make a given agent discover MAE's MCP tools.
pub enum BootstrapStrategy {
    /// Write an entry into `.mcp.json` in the project root.
    McpJson { server_name: &'static str },
}

/// Return the list of agents MAE knows how to bootstrap.
pub fn builtin_agents() -> Vec<AgentDef> {
    vec![AgentDef {
        name: "claude-code",
        binary: "claude",
        description: "Anthropic Claude Code CLI",
        strategy: BootstrapStrategy::McpJson {
            server_name: "mae-editor",
        },
    }]
}

/// Find `mae-mcp-shim` by looking next to the current executable first,
/// then falling back to bare name (relies on PATH).
pub fn resolve_shim_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("mae-mcp-shim");
        if sibling.exists() {
            return sibling;
        }
    }
    PathBuf::from("mae-mcp-shim")
}

/// Read-merge-write `.mcp.json` in `project_root`.
///
/// - Creates the file if absent.
/// - Merges only `mcpServers.mae-editor`, preserving other entries.
/// - Skips the write if content is unchanged (avoids mtime churn).
/// - Returns `Err` if the existing file contains invalid JSON (never clobbers).
pub fn write_mcp_json(project_root: &Path, shim_path: &Path) -> io::Result<()> {
    let mcp_path = project_root.join(".mcp.json");

    let mut root = if mcp_path.exists() {
        let contents = std::fs::read_to_string(&mcp_path)?;
        serde_json::from_str::<serde_json::Value>(&contents).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(".mcp.json contains invalid JSON, not overwriting: {}", e),
            )
        })?
    } else {
        serde_json::json!({})
    };

    // Ensure mcpServers object exists.
    let servers = root
        .as_object_mut()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                ".mcp.json root is not a JSON object",
            )
        })?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    let servers_obj = servers.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            ".mcp.json mcpServers is not a JSON object",
        )
    })?;

    // Build our entry.
    let our_entry = serde_json::json!({
        "command": shim_path.to_string_lossy(),
    });

    // Check if identical — skip write to avoid mtime churn.
    if let Some(existing) = servers_obj.get("mae-editor") {
        if *existing == our_entry {
            return Ok(());
        }
    }

    servers_obj.insert("mae-editor".to_string(), our_entry);

    let new_contents = serde_json::to_string_pretty(&root).map_err(io::Error::other)?;
    std::fs::write(&mcp_path, new_contents)?;
    Ok(())
}

/// Bootstrap a named agent. Returns a status message on success.
pub fn setup_agent(name: &str, project_root: &Path) -> Result<String, String> {
    let agents = builtin_agents();
    let agent = agents.iter().find(|a| a.name == name).ok_or_else(|| {
        format!(
            "Unknown agent: {}. Use :agent-list to see available agents.",
            name
        )
    })?;

    let shim = resolve_shim_path();

    match &agent.strategy {
        BootstrapStrategy::McpJson { server_name } => {
            write_mcp_json(project_root, &shim)
                .map_err(|e| format!("Failed to write .mcp.json: {}", e))?;
            Ok(format!(
                "Wrote .mcp.json with '{}' server (shim: {})",
                server_name,
                shim.display()
            ))
        }
    }
}

/// Format agent list for display.
pub fn agent_list_display() -> String {
    let agents = builtin_agents();
    agents
        .iter()
        .map(|a| format!("  {} — {}", a.name, a.description))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_builtin_agents_has_claude_code() {
        let agents = builtin_agents();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "claude-code");
        assert_eq!(agents[0].binary, "claude");
        matches!(&agents[0].strategy, BootstrapStrategy::McpJson { server_name } if *server_name == "mae-editor");
    }

    #[test]
    fn test_resolve_shim_path() {
        // Should return a path (either sibling of current exe or bare name).
        let path = resolve_shim_path();
        assert!(path.to_string_lossy().contains("mae-mcp-shim"));
    }

    #[test]
    fn test_write_mcp_json_creates_new() {
        let dir = TempDir::new().unwrap();
        let shim = PathBuf::from("/usr/local/bin/mae-mcp-shim");
        write_mcp_json(dir.path(), &shim).unwrap();

        let contents = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(
            val["mcpServers"]["mae-editor"]["command"],
            "/usr/local/bin/mae-mcp-shim"
        );
    }

    #[test]
    fn test_write_mcp_json_merges_existing() {
        let dir = TempDir::new().unwrap();
        let existing = serde_json::json!({
            "mcpServers": {
                "other-tool": { "command": "other-shim" }
            }
        });
        std::fs::write(
            dir.path().join(".mcp.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        let shim = PathBuf::from("/usr/local/bin/mae-mcp-shim");
        write_mcp_json(dir.path(), &shim).unwrap();

        let contents = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&contents).unwrap();
        // Our entry added.
        assert_eq!(
            val["mcpServers"]["mae-editor"]["command"],
            "/usr/local/bin/mae-mcp-shim"
        );
        // Other entry preserved.
        assert_eq!(val["mcpServers"]["other-tool"]["command"], "other-shim");
    }

    #[test]
    fn test_write_mcp_json_overwrites_own_entry() {
        let dir = TempDir::new().unwrap();
        let existing = serde_json::json!({
            "mcpServers": {
                "mae-editor": { "command": "/old/path/mae-mcp-shim" }
            }
        });
        std::fs::write(
            dir.path().join(".mcp.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        let shim = PathBuf::from("/new/path/mae-mcp-shim");
        write_mcp_json(dir.path(), &shim).unwrap();

        let contents = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(
            val["mcpServers"]["mae-editor"]["command"],
            "/new/path/mae-mcp-shim"
        );
    }

    #[test]
    fn test_write_mcp_json_idempotent() {
        let dir = TempDir::new().unwrap();
        let shim = PathBuf::from("/usr/local/bin/mae-mcp-shim");

        write_mcp_json(dir.path(), &shim).unwrap();
        let mtime1 = std::fs::metadata(dir.path().join(".mcp.json"))
            .unwrap()
            .modified()
            .unwrap();

        // Brief pause to ensure mtime would differ if file were rewritten.
        std::thread::sleep(std::time::Duration::from_millis(50));

        write_mcp_json(dir.path(), &shim).unwrap();
        let mtime2 = std::fs::metadata(dir.path().join(".mcp.json"))
            .unwrap()
            .modified()
            .unwrap();

        assert_eq!(
            mtime1, mtime2,
            "file should not be rewritten when content is unchanged"
        );
    }

    #[test]
    fn test_write_mcp_json_rejects_invalid_json() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mcp.json"), "not valid json {{{").unwrap();

        let shim = PathBuf::from("/usr/local/bin/mae-mcp-shim");
        let result = write_mcp_json(dir.path(), &shim);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid JSON"));

        // Original file untouched.
        let contents = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        assert_eq!(contents, "not valid json {{{");
    }

    #[test]
    fn test_setup_agent_unknown() {
        let dir = TempDir::new().unwrap();
        let result = setup_agent("nonexistent", dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown agent"));
    }

    #[test]
    fn test_setup_agent_claude_code() {
        let dir = TempDir::new().unwrap();
        let result = setup_agent("claude-code", dir.path());
        assert!(result.is_ok());
        assert!(dir.path().join(".mcp.json").exists());
    }
}
