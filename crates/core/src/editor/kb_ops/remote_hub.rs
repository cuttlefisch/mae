//! Registering a remote KB hub (ADR-111 P1): the human-only path.
//!
//! @ai-caution: [permission] This is a trust decision, not an edit. A hub's URL
//! receives every `kb_search` query from then on and its answers flow into the
//! agent's context, so `kb-register-hub` is on `AUTHORIZATION_CHANGE_OPS`
//! (Privileged on every surface). It accepts ONLY keystore-key auth: the
//! registry's `RemoteHubAuth::Command` runs a shell command at query time, so
//! exposing it here would also make registration a shell-exec path. A command
//! auth stays reachable by hand-editing the registry, which only the human does.

use super::super::Editor;
use mae_kb::federation::{KbRegistry, RemoteHubAuth};

impl Editor {
    /// Register a remote hub's KB and start querying it live. Idempotent on
    /// `(base_url, hub_kb_id)`.
    pub fn kb_register_hub(
        &mut self,
        name: &str,
        base_url: &str,
        hub_kb_id: &str,
        keystore_key: &str,
    ) -> Result<String, String> {
        let base_url = validate_hub_url(base_url)?;
        if hub_kb_id.trim().is_empty() || keystore_key.trim().is_empty() {
            return Err("a hub KB id and a keystore key name are both required".into());
        }
        let data_dir = self
            .mae_data_dir()
            .ok_or("cannot determine the MAE data directory")?;
        let _ = std::fs::create_dir_all(&data_dir);
        let (registry, uuid, saved) = KbRegistry::update(&data_dir, |reg| {
            reg.register_remote_hub(
                name.to_string(),
                base_url.clone(),
                hub_kb_id.to_string(),
                RemoteHubAuth::KeystoreKey(keystore_key.to_string()),
            )
        });
        let uuid = uuid?;
        saved.map_err(|e| format!("could not save the KB registry: {e}"))?;
        self.kb.registry = registry;
        self.kb.last_local_registry_write = Some(std::time::Instant::now());
        self.kb.rebuild_query_layer();
        let registered = self
            .kb
            .registry
            .find_by_uuid(&uuid)
            .map(|i| i.name.clone())
            .unwrap_or_else(|| name.to_string());
        Ok(format!(
            "Hub KB '{registered}' registered ({base_url}, kb {hub_kb_id}); token from keystore \
             entry '{keystore_key}'. It is queried live, never copied locally."
        ))
    }
}

/// ADR-111: remote clients reach a hub over HTTPS only. A plain-HTTP URL would
/// send the bearer token in the clear.
fn validate_hub_url(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/').to_string();
    let rest = url.strip_prefix("https://").ok_or_else(|| {
        format!("'{url}' is not an https:// URL — a hub is reached over HTTPS only")
    })?;
    if rest.is_empty() || rest.starts_with('/') {
        return Err(format!("'{url}' has no host"));
    }
    Ok(url)
}
