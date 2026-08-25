//! ADR-062 Phase C/D: `RemoteHubQueryLayer` — a `KbQueryLayer` backed by blocking HTTP
//! calls to ADR-053's `kb/query.*` surface on a remote daemon's OAuth-HTTPS listener.
//! Feature-gated behind `remote-hub` (see `Cargo.toml`) since it pulls a TLS+HTTP client
//! into every consumer of this crate, including the interactive editor GUI/TUI binary.
//!
//! Architecturally consistent with every other `KbQueryLayer` implementor: the trait's
//! methods are synchronous by design (`CozoQueryLayer` blocks on a local Cozo query the
//! same way this blocks on an HTTP round-trip) — this is not a bolt-on hack, it's the same
//! contract with a slower backend. The real new risk is latency, not architecture, which
//! is why every call is timeout-bounded (`DEFAULT_TIMEOUT`, Phase E) and a hung/slow hub
//! degrades only this ONE `FederatedQuery` fan-out participant — `FederatedQuery`'s
//! existing per-instance fan-out (Phase B) already isolates one instance's failure from
//! the rest, so no new "partial result" plumbing is needed at that layer.
//!
//! **Hard rule (ADR-062):** every method here is a live call against `config.base_url` —
//! nothing is cached or mirrored to a local store. A `RemoteHubQueryLayer` holds no
//! persistent content, only connection info.
//!
//! **Known limitation, deliberately scoped out (not silently missing):** `kb/query.get`'s
//! `encryption: "e2e"` branch returns ciphertext a genuine KB member would decrypt with
//! key material from ADR-038/039's editor-side membership machinery, which lives above
//! this crate. This layer does not attempt that decryption — an E2E-encrypted hub node
//! surfaces as "not found" (via `last_outcome()`, not silently) rather than as raw
//! ciphertext masquerading as a title/body. `kb/query.search` already structurally
//! refuses for E2E KBs server-side (see `daemon/src/kb_query.rs::search`), so this only
//! affects `get`.

use crate::federation::{RemoteHubAuth, RemoteHubConfig};
use crate::query::KbQueryLayer;
use crate::store::{KbStoreError, Link, SearchHit};
use crate::{Node, NodeKind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Default per-call timeout (Phase E). Short enough that a hung hub doesn't stall an
/// interactive search noticeably longer than a slow disk read would; long enough not to
/// false-positive on a normal cross-network round-trip.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1500);

/// Hard cap on a single `kb/query.*` response body (translation-boundary hardening) — the
/// daemon's own per-call caps (`max_body_bytes`/`max_scan_nodes`/`max_search_results`)
/// already bound a well-behaved hub's response size; this is this crate's own independent
/// backstop against a malicious or buggy hub, so correctness here never depends on
/// trusting the far end to have applied its own caps honestly.
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Diagnostic snapshot of the last call's outcome. Not part of `KbQueryLayer` — that
/// trait's methods return plain `Option`/`Vec`/`bool` with no room for an error value
/// (see `CozoQueryLayer::get`'s own "log a warning, return `None`" precedent for how
/// every other implementor already handles this) — but observable via `last_outcome()`
/// for callers and tests that need to distinguish "the hub legitimately has nothing"
/// from "the call failed," which the ADR-062 Phase D adversarial bar (an expired/revoked
/// token must produce a clean auth failure, never a silent empty result) requires be
/// distinguishable *somewhere*, even though the trait itself can't carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastOutcome {
    Ok,
    AuthFailed(String),
    Timeout,
    Unreachable(String),
    MalformedResponse(String),
}

pub struct RemoteHubQueryLayer {
    config: RemoteHubConfig,
    client: reqwest::blocking::Client,
    last_outcome: Mutex<LastOutcome>,
    next_id: AtomicU64,
}

impl RemoteHubQueryLayer {
    pub fn new(config: RemoteHubConfig) -> Self {
        Self::with_timeout(config, DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(config: RemoteHubConfig, timeout: Duration) -> Self {
        Self::build(config, timeout, false)
    }

    /// Identical to `with_timeout`, except the client does NOT validate the hub's TLS
    /// certificate chain. **Test-only** — a production `base_url` must always be served
    /// by a CA-issued cert; this exists solely so an e2e test can point a real
    /// `RemoteHubQueryLayer` at a daemon serving a locally-generated self-signed cert
    /// (`rcgen`, matching `daemon/tests/oauth_e2e.rs`'s own test-cert convention) without
    /// silently weakening the production TLS trust model to make that possible. Anyone
    /// tempted to call this outside a test should use `with_timeout` and a real cert
    /// instead.
    pub fn with_timeout_and_insecure_tls_for_testing(
        config: RemoteHubConfig,
        timeout: Duration,
    ) -> Self {
        Self::build(config, timeout, true)
    }

    fn build(
        config: RemoteHubConfig,
        timeout: Duration,
        danger_accept_invalid_certs: bool,
    ) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .danger_accept_invalid_certs(danger_accept_invalid_certs)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            config,
            client,
            last_outcome: Mutex::new(LastOutcome::Ok),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn last_outcome(&self) -> LastOutcome {
        self.last_outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set_outcome(&self, outcome: LastOutcome) {
        if let Ok(mut guard) = self.last_outcome.lock() {
            *guard = outcome;
        }
    }

    /// Resolve the bearer token per call — never cached on `self`, so an expired/revoked
    /// token (or a rotated keystore entry) is re-resolved, and re-fails, on every single
    /// call. Matches `crates/mae::collab_bridge::resolve_client_credential`'s existing
    /// `cmd:`-sentinel-or-keystore-key precedent for peer auth (that code lives above this
    /// crate in the dependency graph and can't be reused directly, but the same reference-
    /// not-secret shape is deliberately mirrored here).
    fn resolve_token(&self) -> Result<String, String> {
        match &self.config.auth {
            RemoteHubAuth::Command(cmd) => {
                let output = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .output()
                    .map_err(|e| format!("auth command failed to run: {e}"))?;
                if !output.status.success() {
                    return Err(format!(
                        "auth command exited with {}: {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
                let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if token.is_empty() {
                    return Err("auth command produced an empty token".to_string());
                }
                Ok(token)
            }
            RemoteHubAuth::KeystoreKey(name) => {
                let path = mae_mcp::keystore::default_keystore_path().ok_or_else(|| {
                    "no keystore path resolvable (HOME/XDG_DATA_HOME unset)".to_string()
                })?;
                let ks = mae_mcp::keystore::load(&path)
                    .map_err(|e| format!("keystore load failed: {e}"))?;
                ks.find(name)
                    .map(|e| e.secret.clone())
                    .ok_or_else(|| format!("no keystore entry named '{name}'"))
            }
        }
    }

    /// Make one `kb/query.<method>` JSON-RPC call over HTTP and return the parsed
    /// `result` value, or `None` on any failure. `last_outcome` is always set first so a
    /// failure is observable, never silently indistinguishable from "the hub legitimately
    /// returned nothing."
    fn call(&self, method: &str, mut params: serde_json::Value) -> Option<serde_json::Value> {
        let token = match self.resolve_token() {
            Ok(t) => t,
            Err(e) => {
                self.set_outcome(LastOutcome::AuthFailed(e));
                return None;
            }
        };
        if let Some(obj) = params.as_object_mut() {
            obj.insert(
                "kb_id".to_string(),
                serde_json::Value::String(self.config.hub_kb_id.clone()),
            );
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let resp = match self
            .client
            .post(&self.config.base_url)
            .bearer_auth(&token)
            .json(&body)
            .send()
        {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                self.set_outcome(LastOutcome::Timeout);
                return None;
            }
            Err(e) => {
                self.set_outcome(LastOutcome::Unreachable(e.to_string()));
                return None;
            }
        };

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let desc = resp
                .json::<serde_json::Value>()
                .ok()
                .and_then(|v| {
                    v.get("error_description")
                        .and_then(|d| d.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "bearer token rejected".to_string());
            self.set_outcome(LastOutcome::AuthFailed(desc));
            return None;
        }
        if !resp.status().is_success() {
            self.set_outcome(LastOutcome::Unreachable(format!("HTTP {}", resp.status())));
            return None;
        }

        let bytes = match resp.bytes() {
            Ok(b) if b.len() > MAX_RESPONSE_BYTES => {
                self.set_outcome(LastOutcome::MalformedResponse(format!(
                    "response exceeded {MAX_RESPONSE_BYTES}-byte cap"
                )));
                return None;
            }
            Ok(b) => b,
            Err(e) => {
                self.set_outcome(LastOutcome::Unreachable(e.to_string()));
                return None;
            }
        };

        let rpc: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                self.set_outcome(LastOutcome::MalformedResponse(e.to_string()));
                return None;
            }
        };

        if let Some(err) = rpc.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("hub returned an error")
                .to_string();
            self.set_outcome(LastOutcome::MalformedResponse(msg));
            return None;
        }

        match rpc.get("result").cloned() {
            Some(result) => {
                self.set_outcome(LastOutcome::Ok);
                Some(result)
            }
            None => {
                self.set_outcome(LastOutcome::MalformedResponse(
                    "response had neither 'result' nor 'error'".to_string(),
                ));
                None
            }
        }
    }
}

impl KbQueryLayer for RemoteHubQueryLayer {
    fn degraded(&self) -> bool {
        !matches!(self.last_outcome(), LastOutcome::Ok)
    }

    fn get(&self, id: &str) -> Option<Node> {
        let result = self.call("kb/query.get", serde_json::json!({"node_id": id}))?;
        if result.get("encryption").and_then(|e| e.as_str()) == Some("e2e") {
            // See module doc comment: E2E decrypt-on-read is out of scope for this
            // layer. Recorded distinctly so it never masquerades as "not found for an
            // unrelated reason."
            self.set_outcome(LastOutcome::MalformedResponse(
                "E2E-encrypted hub content is not yet supported by RemoteHubQueryLayer".to_string(),
            ));
            return None;
        }
        let title = result.get("title")?.as_str()?.to_string();
        let body = result.get("body")?.as_str().unwrap_or("").to_string();
        let tags: Vec<String> = result
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let mut node = Node::new(id, title, NodeKind::Note, body);
        node.tags = tags;
        Some(node)
    }

    fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    // NOTE ON THE `Result` CONTRACT (ADR-086 read-side twin, see `query.rs` module doc):
    // every method below still returns `Ok(...)` even on a network/transport failure.
    // This is deliberate, NOT the defect the rest of this crate's `KbQueryLayer`
    // implementors were fixed for: `RemoteHubQueryLayer` already has its own,
    // separately-designed and separately-tested "timeout-and-continue" graceful
    // degradation contract (ADR-062 Phase E) — a failure here is recorded via
    // `set_outcome`/`last_outcome()` and surfaced through `degraded()`, which
    // `FederatedQuery` already polls after every fan-out round. Turning every hub
    // hiccup into an `Err` would regress that already-correct, already-adversarially-
    // tested behavior (see `n_way_blended_query_with_a_hung_hub_bounds_latency_to_the_
    // slowest_source_and_flags_partial` below) in favor of a contract this layer was
    // never meant to have. `Result` in the signature is kept purely for trait-object
    // compatibility with every other `KbQueryLayer` implementor.
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError> {
        let Some(result) = self.call(
            "kb/query.search",
            serde_json::json!({"query": query, "limit": limit}),
        ) else {
            return Ok(Vec::new());
        };
        let Some(results) = result.get("results").and_then(|r| r.as_array()) else {
            self.set_outcome(LastOutcome::MalformedResponse(
                "search response missing 'results' array".to_string(),
            ));
            return Ok(Vec::new());
        };
        // The hub's `kb/query.search` response carries no numeric relevance score (see
        // `daemon/src/kb_query.rs::search` — it returns `{id, title, excerpt}`, ranked by
        // scan order, not scored). A monotonically decreasing synthetic score preserves
        // the hub's own rank order when merged with local FTS-scored hits by
        // `FederatedQuery::search`'s score-descending sort — but these synthetic values
        // are NOT comparable in magnitude to a real BM25-style local score, only in
        // relative order among themselves. Documented here rather than silently treated
        // as an equivalent score.
        Ok(results
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let id = r.get("id")?.as_str()?.to_string();
                let score = 1.0 - (i as f64 * 1e-6);
                Some(SearchHit { id, score })
            })
            .take(limit)
            .collect())
    }

    fn links_from(&self, _id: &str) -> Result<Vec<Link>, KbStoreError> {
        // ADR-053's surface has no links_from/links_to endpoint (only get/search/graph) —
        // structurally empty rather than a partial/best-effort attempt via kb/query.graph
        // (whole-KB, no per-node filtering), matching the trait's own documented default
        // for layers that don't implement link traversal.
        Ok(Vec::new())
    }

    /// The gaps are real, not defensive: ADR-053's `kb/query.*` surface has
    /// **five methods** (`capabilities`, `get`, `search`, `graph`,
    /// `my_wrapped_key`) against this trait's seventeen. What is declared here
    /// is what that surface can genuinely answer — everything else returns
    /// empty today, and a caller had no way to tell that from a real answer.
    ///
    /// `id_title_pairs` is a gap by DESIGN rather than by omission: the graph
    /// endpoint returns bare ids, and fetching each title would be, in this
    /// file's own words, "an N+1 network-call performance trap". Closing it
    /// needs a bulk endpoint (D1a), not a loop here.
    ///
    /// **Shrinking this set to empty is the objective definition of "network
    /// parity"** — the gate D1 is measured against.
    fn capabilities(&self) -> crate::capabilities::QueryCapabilities {
        use crate::capabilities::QueryMethod as M;
        crate::capabilities::QueryCapabilities::all_except(&[
            M::LinksFrom,
            M::LinksTo,
            M::IdTitlePairs,
            M::HealthReport,
            M::Neighborhood,
            M::Related,
            M::LinkedInDegree,
            M::TodoNodes,
            M::Agenda,
            M::History,
            M::NamespacePrefixes,
            M::NodeCrdtState,
        ])
    }

    fn links_to(&self, _id: &str) -> Result<Vec<Link>, KbStoreError> {
        Ok(Vec::new())
    }

    fn list_ids(&self, _prefix: Option<&str>) -> Result<Vec<String>, KbStoreError> {
        let Some(result) = self.call("kb/query.graph", serde_json::json!({})) else {
            return Ok(Vec::new());
        };
        Ok(result
            .get("nodes")
            .and_then(|n| n.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn health_report(&self) -> Result<Option<crate::store::HealthReport>, KbStoreError> {
        // No health endpoint on ADR-053's surface; a hub's health is the hub operator's
        // concern, not something a read-through client can meaningfully report.
        Ok(None)
    }

    fn id_title_pairs(&self, _prefix: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError> {
        // ADR-053's `kb/query.graph` returns bare node ids, no titles (no bulk
        // id+title endpoint exists on this surface) — deliberately NOT implemented via
        // `list_ids` + a `get()` call per id: for a hub with thousands of nodes that
        // would be an N+1 network-call performance trap hidden behind an innocuous-
        // looking "list all node titles" call, exactly the kind of silent scaling cliff
        // ADR-062's own org-roam-grounded Context section warns against. Empty here
        // (same graceful-degrade contract `related`'s trait default already uses for
        // capabilities a layer doesn't support), not a slow best-effort attempt.
        Ok(Vec::new())
    }

    fn neighborhood(
        &self,
        _id: &str,
        _depth: u32,
    ) -> Result<Option<crate::store::SubGraph>, KbStoreError> {
        // No BFS/neighborhood endpoint on ADR-053's surface (`kb/query.graph` is a flat,
        // undepthed whole-KB dump, not a per-node BFS) — not supported.
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};

    /// Minimal synchronous raw-TCP mock HTTP/1.1 server (no framework needed — same
    /// "hand-rolled beats pulling in a service stack for something this simple"
    /// rationale `daemon/tests/oauth_e2e.rs::spawn_mock_jwks_server` already uses).
    /// Serves exactly ONE request with the given raw response body, then the listener
    /// thread exits — sufficient for these one-call-per-test scenarios.
    fn spawn_one_shot_mock(status_line: &str, body: &str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let status_line = status_line.to_string();
        let body = body.to_string();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });
        addr
    }

    fn test_config(base_url: String) -> RemoteHubConfig {
        RemoteHubConfig {
            base_url,
            hub_kb_id: "test-kb".to_string(),
            auth: RemoteHubAuth::Command("echo test-token".to_string()),
        }
    }

    /// A well-formed `kb/query.get` response round-trips to a correct `Node`.
    #[test]
    fn get_parses_a_well_formed_response_into_a_node() {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "kb_id": "test-kb", "node_id": "note:1", "encryption": "none",
                "title": "Hello", "body": "World", "body_truncated": false,
                "tags": ["a", "b"], "links": []
            }
        })
        .to_string();
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        let node = layer
            .get("note:1")
            .expect("must parse a well-formed response");
        assert_eq!(node.title, "Hello");
        assert_eq!(node.body, "World");
        assert_eq!(node.tags, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(layer.last_outcome(), LastOutcome::Ok);
    }

    /// A well-formed `kb/query.search` response translates to `SearchHit`s in the hub's
    /// own rank order (the hub returns no numeric score — see `search`'s doc comment).
    #[test]
    fn search_parses_results_preserving_hub_rank_order() {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "kb_id": "test-kb",
                "results": [
                    {"id": "note:first", "title": "First", "excerpt": "..."},
                    {"id": "note:second", "title": "Second", "excerpt": "..."}
                ],
                "scanned": 2
            }
        })
        .to_string();
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        let hits = layer.search("anything", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "note:first");
        assert_eq!(hits[1].id, "note:second");
        assert!(
            hits[0].score > hits[1].score,
            "rank order must be preserved via a monotonically decreasing synthetic score"
        );
    }

    /// Translation-boundary hardening (ADR-062 Phase D adversarial test): a malformed
    /// (non-JSON) response body must be rejected, never panic or silently return a
    /// default-constructed `Node`.
    #[test]
    fn malformed_response_body_is_rejected_at_the_translation_boundary() {
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", "this is not json at all {{{");
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        let node = layer.get("note:1");
        assert!(node.is_none());
        assert!(matches!(
            layer.last_outcome(),
            LastOutcome::MalformedResponse(_)
        ));
    }

    /// Translation-boundary hardening: a well-formed JSON-RPC envelope whose `result`
    /// object is missing required fields (a hostile/buggy hub sending a schema-valid-
    /// looking but incomplete payload) must be rejected, not silently produce a
    /// half-populated `Node` with empty/garbage title.
    #[test]
    fn schema_incomplete_result_is_rejected_not_silently_half_populated() {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"kb_id": "test-kb", "node_id": "note:1", "encryption": "none"}
            // title/body deliberately absent
        })
        .to_string();
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        assert!(layer.get("note:1").is_none());
    }

    /// Translation-boundary hardening: an oversized response body must be rejected
    /// before being buffered/parsed, never trusted just because it arrived with a 200.
    #[test]
    fn oversized_response_is_rejected_before_parsing() {
        // One giant string value comfortably over MAX_RESPONSE_BYTES.
        let huge_body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "kb_id": "test-kb", "node_id": "note:1", "encryption": "none",
                "title": "x", "body": "y".repeat(MAX_RESPONSE_BYTES + 1024),
                "body_truncated": false, "tags": [], "links": []
            }
        })
        .to_string();
        assert!(huge_body.len() > MAX_RESPONSE_BYTES);
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &huge_body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        assert!(layer.get("note:1").is_none());
        assert!(matches!(
            layer.last_outcome(),
            LastOutcome::MalformedResponse(_)
        ));
    }

    /// A JSON-RPC `error` response (the hub's own dispatch-layer rejection — e.g. a
    /// KB the hub doesn't have) must degrade gracefully, not panic, and must be
    /// observable via `last_outcome` as distinct from a transport/parse failure only in
    /// that it carries the hub's own message.
    #[test]
    fn jsonrpc_error_response_degrades_gracefully() {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "error": {"code": -32000, "message": "no such KB 'test-kb'"}
        })
        .to_string();
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        assert!(layer.get("note:1").is_none());
        if let LastOutcome::MalformedResponse(msg) = layer.last_outcome() {
            assert!(msg.contains("no such KB"));
        } else {
            panic!("expected the hub's JSON-RPC error message to surface via last_outcome");
        }
    }

    /// E2E-encrypted hub content is explicitly unsupported (module doc comment) — must
    /// surface as "not found" via a distinguishable outcome, never as raw ciphertext
    /// masquerading as a plaintext title/body.
    #[test]
    fn e2e_encrypted_node_is_not_silently_treated_as_plaintext() {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "kb_id": "test-kb", "node_id": "note:1", "encryption": "e2e",
                "ciphertext_b64": "not-real-plaintext-obviously"
            }
        })
        .to_string();
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        let node = layer.get("note:1");
        assert!(
            node.is_none(),
            "E2E ciphertext must never be surfaced as a plaintext Node"
        );
        assert!(matches!(
            layer.last_outcome(),
            LastOutcome::MalformedResponse(_)
        ));
    }

    /// A connection to a genuinely unreachable address must fail cleanly (not hang past
    /// the configured timeout, not panic).
    #[test]
    fn unreachable_hub_fails_cleanly_within_the_timeout() {
        // Port 1 is reserved and nothing listens there in any normal test environment;
        // combined with a short timeout this proves the failure path, not a real hang.
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config("http://127.0.0.1:1".to_string()),
            Duration::from_millis(500),
        );
        let start = std::time::Instant::now();
        assert!(layer.get("note:1").is_none());
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "must fail well within a bounded time, not hang"
        );
        assert!(matches!(
            layer.last_outcome(),
            LastOutcome::Unreachable(_) | LastOutcome::Timeout
        ));
    }

    /// Accepts a connection and then never responds — simulates a genuinely hung hub
    /// (not merely unreachable/refused), the specific failure shape ADR-062 Phase E's
    /// "N-way blended query with a deliberately-hung RemoteHub" test names.
    fn spawn_hanging_mock() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((_stream, _)) = listener.accept() {
                // Hold the connection open, write nothing, forever (until the client's
                // own timeout gives up and drops it).
                std::thread::sleep(Duration::from_secs(30));
            }
        });
        addr
    }

    /// ADR-062 Phase E's own named adversarial test: an N-way blended query (primary +
    /// 2 local federated instances + 1 deliberately-hung `RemoteHub`) must have the other
    /// 3 sources return within the local-only latency budget, with `last_query_was_partial`
    /// set — proving `FederatedQuery::search`'s concurrent fan-out (not a sequential loop)
    /// actually bounds total latency by the SLOWEST source, not the SUM of all sources. A
    /// naive sequential implementation would make this test's own timing assertion fail
    /// (total latency would include the full hung-hub timeout ON TOP OF the other sources'
    /// near-instant local latency, not overlapping with it).
    #[test]
    fn n_way_blended_query_with_a_hung_hub_bounds_latency_to_the_slowest_source_and_flags_partial()
    {
        use crate::query::{FederatedQuery, InMemoryQueryLayer, KbQueryLayer as _};
        use crate::{KnowledgeBase, Node, NodeKind};
        use std::sync::Arc;

        let mut primary_kb = KnowledgeBase::new();
        primary_kb.insert(Node::new("p:1", "Primary Widget", NodeKind::Note, "widget"));
        let primary = Arc::new(InMemoryQueryLayer::new(primary_kb));

        let mut federated = FederatedQuery::new(primary);

        let mut local_a = KnowledgeBase::new();
        local_a.insert(Node::new("a:1", "Local A Widget", NodeKind::Note, "widget"));
        federated.add_instance(
            "local-a".into(),
            10,
            Arc::new(InMemoryQueryLayer::new(local_a)),
        );

        let mut local_b = KnowledgeBase::new();
        local_b.insert(Node::new("b:1", "Local B Widget", NodeKind::Note, "widget"));
        federated.add_instance(
            "local-b".into(),
            5,
            Arc::new(InMemoryQueryLayer::new(local_b)),
        );

        let hung_addr = spawn_hanging_mock();
        let hub_timeout = Duration::from_millis(400);
        let hung_hub = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{hung_addr}")),
            hub_timeout,
        );
        federated.add_instance("hung-hub".into(), 1, Arc::new(hung_hub));

        let start = std::time::Instant::now();
        let hits = federated.search("widget", 10).unwrap();
        let elapsed = start.elapsed();

        // The other 3 sources' real content must still be present.
        let ids: std::collections::HashSet<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert!(ids.contains("p:1"));
        assert!(ids.contains("a:1"));
        assert!(ids.contains("b:1"));

        // Bounded by the slowest source (~hub_timeout), NOT the sum of all sources' own
        // latencies. A generous multiplier (3x) absorbs scheduling jitter on a loaded CI
        // box while still clearly falsifying "the whole call serialized on the hung hub
        // plus every other source's own overhead."
        assert!(
            elapsed < hub_timeout * 3,
            "expected concurrent fan-out to bound latency near the hung hub's own timeout \
             ({hub_timeout:?}), got {elapsed:?} — looks like the fan-out serialized instead \
             of running in parallel"
        );

        assert!(
            federated.last_query_was_partial(),
            "a hung RemoteHub source must set the partial-result flag"
        );

        // Recovery: unblock nothing (the hung server stays hung for its own thread's
        // lifetime), but a fresh query against a federation with NO degraded source must
        // clear the flag — proving it's per-call, never a stuck-degraded state.
        let federated_healthy = FederatedQuery::new(Arc::new(InMemoryQueryLayer::new({
            let mut kb = KnowledgeBase::new();
            kb.insert(Node::new("h:1", "Healthy", NodeKind::Note, "widget"));
            kb
        })));
        federated_healthy.search("widget", 10).unwrap();
        assert!(
            !federated_healthy.last_query_was_partial(),
            "a federation with no degraded source must not report partial results"
        );
    }
}
