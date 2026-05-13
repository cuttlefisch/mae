//! MCP client manager — owns all external MCP server connections.
//!
//! Parses `[[mcp.servers]]` from config.toml, manages lifecycle,
//! and provides a unified interface for tool discovery and dispatch.

use std::collections::HashMap;

use tracing::{info, warn};

use crate::client::{ClientState, ExternalToolInfo, McpClient, McpServerConfig};

/// Manages connections to all configured external MCP servers.
pub struct McpClientManager {
    clients: HashMap<String, McpClient>,
}

impl McpClientManager {
    pub fn new(configs: Vec<McpServerConfig>) -> Self {
        let mut clients = HashMap::new();
        for config in configs {
            if config.enabled {
                let name = config.name.clone();
                clients.insert(name, McpClient::new(config));
            }
        }
        McpClientManager { clients }
    }

    /// Start all auto_start servers.
    pub async fn start_all(&mut self) {
        let names: Vec<String> = self.clients.keys().cloned().collect();
        for name in names {
            if let Some(client) = self.clients.get_mut(&name) {
                if let Err(e) = client.start().await {
                    warn!(server = %name, error = %e, "Failed to start MCP server");
                }
            }
        }
    }

    /// Collect all discovered external tools across all connected servers.
    pub fn external_tools(&self) -> Vec<ExternalToolInfo> {
        self.clients
            .values()
            .flat_map(|c| c.tools().iter().cloned())
            .collect()
    }

    /// Call a tool on a specific server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, String> {
        let client = self
            .clients
            .get(server_name)
            .ok_or_else(|| format!("Unknown MCP server: {}", server_name))?;
        if *client.state() != ClientState::Ready {
            return Err(format!(
                "MCP server '{}' is not ready (state: {})",
                server_name,
                client.state()
            ));
        }
        client.call_tool(tool_name, arguments).await
    }

    /// Force reconnect a server.
    pub async fn reconnect(&mut self, server_name: &str) -> Result<(), String> {
        let client = self
            .clients
            .get_mut(server_name)
            .ok_or_else(|| format!("Unknown MCP server: {}", server_name))?;
        info!(server = %server_name, "MCP reconnecting");
        client.reconnect().await;
        Ok(())
    }

    /// Get status of all servers.
    pub fn status(&self) -> Vec<(String, String, usize)> {
        self.clients
            .iter()
            .map(|(name, client)| {
                (
                    name.clone(),
                    format!("{}", client.state()),
                    client.tools().len(),
                )
            })
            .collect()
    }

    /// Check if any servers are configured.
    pub fn has_servers(&self) -> bool {
        !self.clients.is_empty()
    }

    /// Parse `[[mcp.servers]]` from a TOML config table.
    pub fn parse_configs(config: &toml::Value) -> Vec<McpServerConfig> {
        let mut configs = Vec::new();
        let servers = config
            .get("mcp")
            .and_then(|v| v.get("servers"))
            .and_then(|v| v.as_array());

        if let Some(servers) = servers {
            for server in servers {
                let name = server
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let command = server
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args: Vec<String> = server
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let env: HashMap<String, String> = server
                    .get("env")
                    .and_then(|v| v.as_table())
                    .map(|t| {
                        t.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                let enabled = server
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let auto_start = server
                    .get("auto_start")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                if !name.is_empty() && !command.is_empty() {
                    configs.push(McpServerConfig {
                        name,
                        command,
                        args,
                        env,
                        enabled,
                        auto_start,
                    });
                }
            }
        }
        configs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_config() {
        let config: toml::Value = toml::Value::Table(Default::default());
        let configs = McpClientManager::parse_configs(&config);
        assert!(configs.is_empty());
    }

    #[test]
    fn parse_server_config() {
        let toml_str = r#"
[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/docs"]
enabled = true
auto_start = true

[[mcp.servers]]
name = "github"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "test-token" }
"#;
        let config: toml::Value = toml::from_str(toml_str).unwrap();
        let configs = McpClientManager::parse_configs(&config);
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].name, "filesystem");
        assert_eq!(configs[0].command, "npx");
        assert_eq!(configs[0].args.len(), 3);
        assert!(configs[0].enabled);
        assert_eq!(configs[1].name, "github");
        assert_eq!(configs[1].env.get("GITHUB_TOKEN").unwrap(), "test-token");
    }

    #[test]
    fn parse_disabled_server() {
        let toml_str = r#"
[[mcp.servers]]
name = "disabled"
command = "echo"
args = []
enabled = false
"#;
        let config: toml::Value = toml::from_str(toml_str).unwrap();
        let configs = McpClientManager::parse_configs(&config);
        assert_eq!(configs.len(), 1);
        assert!(!configs[0].enabled);
    }

    #[test]
    fn manager_filters_disabled() {
        let configs = vec![
            McpServerConfig {
                name: "enabled".into(),
                command: "echo".into(),
                args: vec![],
                env: HashMap::new(),
                enabled: true,
                auto_start: true,
            },
            McpServerConfig {
                name: "disabled".into(),
                command: "echo".into(),
                args: vec![],
                env: HashMap::new(),
                enabled: false,
                auto_start: true,
            },
        ];
        let mgr = McpClientManager::new(configs);
        assert_eq!(mgr.status().len(), 1);
        assert!(mgr.has_servers());
    }

    #[test]
    fn empty_manager() {
        let mgr = McpClientManager::new(vec![]);
        assert!(!mgr.has_servers());
        assert!(mgr.external_tools().is_empty());
        assert!(mgr.status().is_empty());
    }
}
