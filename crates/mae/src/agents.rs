//! Agent bootstrap system: auto-configure MCP clients in MAE's terminal.
//!
//! When a shell terminal spawns, MAE writes a `.mcp.json` to the project
//! root so that Claude Code (and future MCP-aware agents) automatically
//! discover the editor's tool surface — zero manual `claude mcp add`.
//!
//! The key insight: `MAE_MCP_SOCKET` is inherited from the PTY environment
//! (already injected by the shell spawn code), NOT placed in `.mcp.json`.
//! This makes the file static (no PID) and reusable across MAE restarts.
//!
//! # Adding support for a new AI agent
//!
//! MAE's agent bootstrap is agent-agnostic. The Claude Code implementation
//! serves as the reference. To add a new agent:
//!
//! **Step 1: Register the agent** — Add an [`AgentDef`] to [`builtin_agents()`]:
//! ```ignore
//! AgentDef {
//!     name: "my-agent",
//!     binary: "myagent",
//!     description: "My Agent CLI",
//!     strategy: BootstrapStrategy::McpJson { server_name: "mae-editor" },
//! }
//! ```
//! `McpJson` works for any agent that reads `.mcp.json` (MCP standard).
//!
//! **Step 2: Add a settings writer** (if the agent has its own permission
//! system) — implement `write_<agent>_settings(project_root) -> io::Result<()>`:
//! - Read-merge-write: never clobber the user's existing settings.
//! - Idempotent: skip write if content is unchanged (avoid mtime churn).
//! - Reject invalid existing files: return `Err`, don't overwrite.
//! - Approve all MAE tools at once: don't enumerate 275+ tools individually.
//! - Create parent dirs: the settings directory may not exist yet.
//!
//! See [`write_claude_settings()`] for the canonical implementation.
//!
//! **Step 3: Register in dispatcher** — Add the writer to
//! [`write_agent_settings()`] so it runs on shell spawn.
//!
//! ## Claude Code specifics (not universal)
//!
//! - Wildcard `mcp__mae-editor` in `permissions.allow` approves all tools
//!   from the server. Other agents may use different trust patterns.
//! - Settings live in `{project}/.claude/settings.local.json`. Other agents
//!   use different paths.
//!
//! ## What's universal
//!
//! - `.mcp.json` is the MCP standard — any MCP-aware agent reads it.
//! - `MAE_MCP_SOCKET` env var is inherited by all PTY children.
//! - `mae-mcp-shim` bridges Unix socket ↔ stdio for any agent.

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
///
/// Currently only `McpJson` exists, but this enum is the extension point
/// for agents that use non-standard discovery (e.g. a CLI `add` command
/// instead of a config file). `McpJson` covers any MCP-standard agent —
/// if an agent reads `.mcp.json`, it works out of the box with no new
/// variant needed.
pub enum BootstrapStrategy {
    /// Write an entry into `.mcp.json` in the project root.
    McpJson { server_name: &'static str },
    /// Write an entry into `.gemini/settings.json` in the project root.
    GeminiConfig { server_name: &'static str },
}

/// Return the list of agents MAE knows how to bootstrap.
///
/// Known-compatible agents (tested or expected to work):
/// - **Claude Code** — fully tested, ships with settings writer
/// - **Cline** — reads `.mcp.json` (McpJson strategy works as-is)
/// - **Aider** — reads `.mcp.json` (McpJson strategy works as-is)
/// - **Gemini CLI** — ships with settings writer
///
/// To add a new agent, push an `AgentDef` here and (if needed) add a
/// settings writer in `write_agent_settings()`. See module-level docs.
pub fn builtin_agents() -> Vec<AgentDef> {
    vec![
        AgentDef {
            name: "claude-code",
            binary: "claude",
            description: "Anthropic Claude Code CLI",
            strategy: BootstrapStrategy::McpJson {
                server_name: "mae-editor",
            },
        },
        AgentDef {
            name: "gemini-cli",
            binary: "gemini",
            description: "Google Gemini CLI",
            strategy: BootstrapStrategy::GeminiConfig {
                server_name: "mae-editor",
            },
        },
    ]
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

/// Write Claude Code's `.claude/settings.local.json` to auto-approve MAE tools.
///
/// Uses the `mcp__mae-editor` wildcard pattern which approves all tools from
/// the `mae-editor` MCP server. This is Claude Code's convention — other agents
/// may use different patterns (see module-level docs for adding new agents).
///
/// Read-merge-write: preserves existing entries (Bash permissions, deny list, etc.).
/// Idempotent: skips write if content is unchanged.
pub fn write_claude_settings(project_root: &Path) -> io::Result<()> {
    let claude_dir = project_root.join(".claude");
    std::fs::create_dir_all(&claude_dir)?;
    let settings_path = claude_dir.join("settings.local.json");

    let mut root = if settings_path.exists() {
        let contents = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str::<serde_json::Value>(&contents).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    ".claude/settings.local.json contains invalid JSON, not overwriting: {}",
                    e
                ),
            )
        })?
    } else {
        serde_json::json!({})
    };

    let obj = root.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            ".claude/settings.local.json root is not a JSON object",
        )
    })?;

    // Ensure permissions.allow contains "mcp__mae-editor".
    let perms = obj
        .entry("permissions")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(perms_obj) = perms.as_object_mut() {
        let allow = perms_obj
            .entry("allow")
            .or_insert_with(|| serde_json::json!([]));
        if let Some(allow_arr) = allow.as_array_mut() {
            let entry = serde_json::json!("mcp__mae-editor");
            if !allow_arr.contains(&entry) {
                allow_arr.push(entry);
            }
        }
    }

    // Skip write if unchanged.
    let new_contents = serde_json::to_string_pretty(&root).map_err(io::Error::other)?;
    if settings_path.exists() {
        let existing = std::fs::read_to_string(&settings_path)?;
        if existing == new_contents {
            return Ok(());
        }
    }

    std::fs::write(&settings_path, new_contents)?;
    Ok(())
}

/// Write Gemini CLI's `.gemini/settings.json` to auto-approve MAE tools.
///
/// Sets `trust: true` for the `mae-editor` MCP server.
/// Read-merge-write: preserves existing entries.
pub fn write_gemini_settings(project_root: &Path, shim_path: &Path) -> io::Result<()> {
    let gemini_dir = project_root.join(".gemini");
    std::fs::create_dir_all(&gemini_dir)?;
    let settings_path = gemini_dir.join("settings.json");

    let mut root = if settings_path.exists() {
        let contents = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str::<serde_json::Value>(&contents).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    ".gemini/settings.json contains invalid JSON, not overwriting: {}",
                    e
                ),
            )
        })?
    } else {
        serde_json::json!({})
    };

    let obj = root.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            ".gemini/settings.json root is not a JSON object",
        )
    })?;

    // Ensure mcpServers object exists.
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers_obj = servers.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            ".gemini/settings.json mcpServers is not a JSON object",
        )
    })?;

    // Build our entry.
    let our_entry = serde_json::json!({
        "command": shim_path.to_string_lossy(),
        "trust": true,
    });

    // Check if identical — skip write to avoid mtime churn.
    if let Some(existing) = servers_obj.get("mae-editor") {
        if *existing == our_entry {
            return Ok(());
        }
    }

    servers_obj.insert("mae-editor".to_string(), our_entry);

    let new_contents = serde_json::to_string_pretty(&root).map_err(io::Error::other)?;
    std::fs::write(&settings_path, new_contents)?;
    Ok(())
}

/// Write agent-specific settings files for all known agents.
///
/// Called alongside `write_mcp_json()` on shell spawn when
/// `auto_approve_tools` is enabled. Each agent gets its own
/// settings writer — see module docs for how to add new agents.
///
/// The current if/else dispatch pattern scales fine for 3-4 agents.
/// If we grow beyond that, extract a `trait AgentSettings` with a
/// `write_settings(&self, project_root: &Path) -> io::Result<()>` method
/// and store it on `AgentDef`.
pub fn write_agent_settings(project_root: &Path) -> io::Result<()> {
    let shim = resolve_shim_path();
    for agent in builtin_agents() {
        // Future agents: add their settings writer as new branches here.
        if agent.name == "claude-code" {
            write_claude_settings(project_root)?;
        } else if agent.name == "gemini-cli" {
            write_gemini_settings(project_root, &shim)?;
        }
    }
    Ok(())
}

/// Bootstrap a named agent. Returns a status message on success.
///
/// Writes both `.mcp.json` (tool discovery) and agent-specific settings
/// (tool approval) to the project root.
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
            // Also write agent-specific settings for tool approval.
            if let Err(e) = write_agent_settings(project_root) {
                return Err(format!(
                    "Wrote .mcp.json but failed to write agent settings: {}",
                    e
                ));
            }
            Ok(format!(
                "Wrote .mcp.json with '{}' server + agent settings (shim: {})",
                server_name,
                shim.display()
            ))
        }
        BootstrapStrategy::GeminiConfig { server_name } => {
            write_gemini_settings(project_root, &shim)
                .map_err(|e| format!("Failed to write .gemini/settings.json: {}", e))?;
            Ok(format!(
                "Wrote .gemini/settings.json with '{}' server (shim: {})",
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
    fn test_builtin_agents_has_claude_and_gemini() {
        let agents = builtin_agents();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].name, "claude-code");
        assert_eq!(agents[1].name, "gemini-cli");
        assert_eq!(agents[1].binary, "gemini");
        matches!(&agents[1].strategy, BootstrapStrategy::GeminiConfig { server_name } if *server_name == "mae-editor");
    }

    #[test]
    fn test_setup_agent_gemini_cli() {
        let dir = TempDir::new().unwrap();
        let result = setup_agent("gemini-cli", dir.path());
        assert!(result.is_ok());
        assert!(dir.path().join(".gemini/settings.json").exists());
    }

    #[test]
    fn test_write_gemini_settings_creates_new() {
        let dir = TempDir::new().unwrap();
        let shim = PathBuf::from("/usr/local/bin/mae-mcp-shim");
        write_gemini_settings(dir.path(), &shim).unwrap();

        let path = dir.path().join(".gemini/settings.json");
        assert!(path.exists());
        let contents = std::fs::read_to_string(&path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(
            val["mcpServers"]["mae-editor"]["command"],
            "/usr/local/bin/mae-mcp-shim"
        );
        assert_eq!(val["mcpServers"]["mae-editor"]["trust"], true);
    }

    #[test]
    fn test_write_gemini_settings_merges_existing() {
        let dir = TempDir::new().unwrap();
        let gemini_dir = dir.path().join(".gemini");
        std::fs::create_dir_all(&gemini_dir).unwrap();
        let existing = serde_json::json!({
            "mcpServers": {
                "other-tool": { "command": "other-shim" }
            },
            "otherSetting": true
        });
        std::fs::write(
            gemini_dir.join("settings.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        let shim = PathBuf::from("/usr/local/bin/mae-mcp-shim");
        write_gemini_settings(dir.path(), &shim).unwrap();

        let contents = std::fs::read_to_string(gemini_dir.join("settings.json")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&contents).unwrap();
        // Our entry added.
        assert_eq!(
            val["mcpServers"]["mae-editor"]["command"],
            "/usr/local/bin/mae-mcp-shim"
        );
        // Other entry preserved.
        assert_eq!(val["mcpServers"]["other-tool"]["command"], "other-shim");
        // Other settings preserved.
        assert_eq!(val["otherSetting"], true);
    }

    #[test]
    fn test_write_gemini_settings_idempotent() {
        let dir = TempDir::new().unwrap();
        let shim = PathBuf::from("/usr/local/bin/mae-mcp-shim");

        write_gemini_settings(dir.path(), &shim).unwrap();
        let path = dir.path().join(".gemini/settings.json");
        let mtime1 = std::fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        write_gemini_settings(dir.path(), &shim).unwrap();
        let mtime2 = std::fs::metadata(&path).unwrap().modified().unwrap();

        assert_eq!(
            mtime1, mtime2,
            "file should not be rewritten when content is unchanged"
        );
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
        // setup_agent now also writes agent settings.
        assert!(dir.path().join(".claude/settings.local.json").exists());
    }

    #[test]
    fn test_write_claude_settings_creates_new() {
        let dir = TempDir::new().unwrap();
        write_claude_settings(dir.path()).unwrap();

        let path = dir.path().join(".claude/settings.local.json");
        assert!(path.exists());
        let contents = std::fs::read_to_string(&path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let allow = val["permissions"]["allow"].as_array().unwrap();
        assert!(allow.contains(&serde_json::json!("mcp__mae-editor")));
    }

    #[test]
    fn test_write_claude_settings_merges_existing() {
        let dir = TempDir::new().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let existing = serde_json::json!({
            "permissions": {
                "allow": ["Bash(npm test)"],
                "deny": ["Bash(rm -rf /)"]
            },
            "enableAllProjectMcpServers": false
        });
        std::fs::write(
            claude_dir.join("settings.local.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        write_claude_settings(dir.path()).unwrap();

        let contents = std::fs::read_to_string(claude_dir.join("settings.local.json")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let allow = val["permissions"]["allow"].as_array().unwrap();
        // Our entry added.
        assert!(allow.contains(&serde_json::json!("mcp__mae-editor")));
        // Existing entries preserved.
        assert!(allow.contains(&serde_json::json!("Bash(npm test)")));
        // Deny list preserved.
        assert_eq!(val["permissions"]["deny"][0], "Bash(rm -rf /)");
        // Other keys preserved.
        assert_eq!(val["enableAllProjectMcpServers"], false);
    }

    #[test]
    fn test_write_claude_settings_no_duplicate() {
        let dir = TempDir::new().unwrap();
        write_claude_settings(dir.path()).unwrap();
        write_claude_settings(dir.path()).unwrap();

        let path = dir.path().join(".claude/settings.local.json");
        let contents = std::fs::read_to_string(&path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let allow = val["permissions"]["allow"].as_array().unwrap();
        let count = allow
            .iter()
            .filter(|v| *v == &serde_json::json!("mcp__mae-editor"))
            .count();
        assert_eq!(count, 1, "should not duplicate mcp__mae-editor entry");
    }

    #[test]
    fn test_write_claude_settings_idempotent() {
        let dir = TempDir::new().unwrap();
        write_claude_settings(dir.path()).unwrap();
        let path = dir.path().join(".claude/settings.local.json");
        let mtime1 = std::fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        write_claude_settings(dir.path()).unwrap();
        let mtime2 = std::fs::metadata(&path).unwrap().modified().unwrap();

        assert_eq!(
            mtime1, mtime2,
            "file should not be rewritten when content is unchanged"
        );
    }

    #[test]
    fn test_write_claude_settings_rejects_invalid_json() {
        let dir = TempDir::new().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("settings.local.json"), "not valid json").unwrap();

        let result = write_claude_settings(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid JSON"));
    }

    #[test]
    fn test_write_agent_settings_dispatcher() {
        let dir = TempDir::new().unwrap();
        write_agent_settings(dir.path()).unwrap();
        // Claude Code settings should be written.
        assert!(dir.path().join(".claude/settings.local.json").exists());
    }
}
