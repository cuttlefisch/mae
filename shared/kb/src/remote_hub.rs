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
    /// D1: one `kb/query.links` round trip, decoded into `Link`s.
    ///
    /// Returns `(links, truncated)`. The hub reports bare node ids, so `rel_type`
    /// is the generic `"links_to"` and weight/confidence take their defaults --
    /// the wire format carries no typed-edge information yet (ADR-101 would add
    /// it). Stated here rather than silently defaulted, because a caller reading
    /// `rel_type` off a hub link would otherwise believe it was authored.
    fn links_in_direction(&self, id: &str, direction: &str) -> (Vec<Link>, bool) {
        let Some(result) = self.call(
            "kb/query.links",
            serde_json::json!({"node_id": id, "direction": direction}),
        ) else {
            return (Vec::new(), false);
        };
        if let Some(reason) = result.get("unavailable_reason").and_then(|v| v.as_str()) {
            // An E2E KB cannot answer this server-side (ADR-037's key-blind
            // daemon). Surfaced as degraded rather than as an empty list.
            self.set_outcome(LastOutcome::MalformedResponse(reason.to_string()));
            return (Vec::new(), false);
        }
        let truncated = result
            .get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let key = if direction == "to" {
            "links_to"
        } else {
            "links_from"
        };
        let links = result
            .get(key)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|other| {
                        let (src, dst) = if direction == "to" {
                            (other.to_string(), id.to_string())
                        } else {
                            (id.to_string(), other.to_string())
                        };
                        Link {
                            src,
                            dst,
                            rel_type: "links_to".to_string(),
                            display: None,
                            weight: 1.0,
                            confidence: 1.0,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        (links, truncated)
    }

    /// D1: served by `kb/query.titles`, the bulk endpoint this method's previous
    /// refusal explicitly asked for.
    ///
    /// It used to return empty and say so in a comment -- *"an N+1 network-call
    /// performance trap"* -- which was a correct diagnosis of the wrong fix
    /// The shared body behind `todo_nodes` and `agenda`.
    ///
    /// Returns `Node`s carrying only what the hub's agenda endpoint knows —
    /// id/title/tags/todo/priority. Bodies are deliberately not fetched: doing so
    /// would be one network call per matched node, the same N+1 trap
    /// `id_title_pairs` refuses. A caller that needs a body calls `get`.
    fn agenda_impl(&self, filter: &str, value: Option<&str>) -> Vec<Node> {
        let mut params = serde_json::json!({"filter": filter});
        if let Some(v) = value {
            params["value"] = serde_json::Value::String(v.to_string());
        }
        let Some(result) = self.call("kb/query.agenda", params) else {
            return Vec::new();
        };
        if let Some(reason) = result.get("unavailable_reason").and_then(|v| v.as_str()) {
            self.set_outcome(LastOutcome::MalformedResponse(reason.to_string()));
            return Vec::new();
        }
        if truncated(&result) {
            self.set_outcome(LastOutcome::MalformedResponse(format!(
                "agenda '{filter}' hit the hub's max_scan_nodes cap; the result is partial"
            )));
        }
        result
            .get("nodes")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(agenda_node).collect())
            .unwrap_or_default()
    }

    /// (looping `get`). Titles live in the collection manifest the hub already
    /// loads for any gated read, so this is one call and no per-node fetch.
    fn id_title_pairs_impl(&self, prefix: Option<&str>) -> Vec<(String, String)> {
        let mut params = serde_json::Map::new();
        if let Some(p) = prefix {
            params.insert("prefix".into(), serde_json::json!(p));
        }
        let Some(result) = self.call("kb/query.titles", serde_json::Value::Object(params)) else {
            return Vec::new();
        };
        if result
            .get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            self.set_outcome(LastOutcome::MalformedResponse(
                "title listing hit the hub's max_scan_nodes cap; results are partial".to_string(),
            ));
        }
        result
            .get("pairs")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|pair| {
                        let a = pair.as_array()?;
                        Some((
                            a.first()?.as_str()?.to_string(),
                            a.get(1)?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

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

    /// D1: served by `kb/query.links`, which reads the node's own document --
    /// **O(1) in corpus size**, because edges are stored on their source.
    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        Ok(self.links_in_direction(id, "from").0)
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
    ///
    /// ### Two of these can never be closed, and that is a finding, not a TODO
    ///
    /// **`History`** reads `node_versions`, which **ADR-106 makes a LOCAL audit
    /// trail that does not sync** — it lives only in Cozo, appears nowhere in
    /// `shared/sync/`, and is deliberately outside the checkpoint. A hub
    /// therefore has no version history OF ANOTHER PEER'S EDITS to serve. This is
    /// not an endpoint nobody wrote; it is a consequence of where history is
    /// defined to live.
    ///
    /// **`NodeCrdtState`** hands back raw CRDT bytes, which is replication — the
    /// exact thing ADR-067's `QueryOnly` policy exists to withhold. Serving it
    /// here would let a query-only member reconstruct the full local replica they
    /// were restricted from taking, defeating the control rather than completing
    /// it (ADR-085: *not offered* beats offered-and-denied).
    ///
    /// So D1's gate is **six**, not eight. Recording that here rather than
    /// leaving two permanent entries to read as unfinished work — a gap list
    /// nobody can ever empty is a gap list people stop believing.
    fn capabilities(&self) -> crate::capabilities::QueryCapabilities {
        use crate::capabilities::QueryMethod as M;
        crate::capabilities::QueryCapabilities::all_except(&[
            // STRUCTURAL, not unimplemented — see the doc comment above.
            // **This is the whole remaining set.** D1b closed HealthReport,
            // TodoNodes and Agenda; what is left cannot be closed without
            // defeating the ADR that defines it.
            M::History,
            M::NodeCrdtState,
        ])
    }

    /// D1: served by `kb/query.links`. **O(N) server-side and capped** -- edges
    /// live on the source, so finding what points AT a node is a scan (the same
    /// asymmetry #265 records for `links:by_dst` on the Cozo side).
    ///
    /// A truncated scan sets `degraded()`, because a short backlink list reads as
    /// "nothing links here" -- a wrong answer, not a partial one.
    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let (links, truncated) = self.links_in_direction(id, "to");
        if truncated {
            self.set_outcome(LastOutcome::MalformedResponse(
                "backlink scan hit the hub's max_scan_nodes cap; results are partial".to_string(),
            ));
        }
        Ok(links)
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

    /// D1b: derived from the graph endpoint that already exists — the whole
    /// subgraph in one call, then counted locally.
    ///
    /// No new server endpoint, and no N+1: `kb/query.graph` returns nodes AND
    /// edges together, so in-degree is arithmetic over a response the client can
    /// already fetch.
    fn linked_in_degree(&self) -> Result<std::collections::HashMap<String, usize>, KbStoreError> {
        let Some(result) = self.call("kb/query.graph", serde_json::json!({})) else {
            return Ok(std::collections::HashMap::new());
        };
        if result
            .get("edges_truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            // A truncated edge set yields UNDERCOUNTS, which read as "this node is
            // barely linked" — a wrong answer, not a partial one (the same
            // reasoning as the backlink cap in D1a).
            self.set_outcome(LastOutcome::MalformedResponse(
                "graph edges hit the hub's cap; in-degree counts are undercounts".to_string(),
            ));
        }
        let mut out: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for e in result
            .get("edges")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(dst) = e.as_array().and_then(|a| a.get(1)).and_then(|v| v.as_str()) {
                *out.entry(dst.to_string()).or_insert(0) += 1;
            }
        }
        Ok(out)
    }

    /// D1b: nodes sharing a link with `id`, ranked by how many they share.
    ///
    /// Served by `kb/query.neighborhood` (D1a), so again no new endpoint. The
    /// score is a shared-edge count, not the local store's relatedness metric —
    /// stated here rather than passed off as equivalent, the same honesty
    /// `search`'s synthetic score already applies.
    fn related(&self, id: &str, limit: usize) -> Result<Vec<(String, f64)>, KbStoreError> {
        let Some(sub) = self.neighborhood(id, 1)? else {
            return Ok(Vec::new());
        };
        let mut scores: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for (src, dst, _rel) in &sub.edges {
            for other in [src, dst] {
                if other != id {
                    *scores.entry(other.clone()).or_insert(0.0) += 1.0;
                }
            }
        }
        let mut out: Vec<(String, f64)> = scores.into_iter().collect();
        // Deterministic: score desc, then id asc, so two clients agree.
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        out.truncate(limit);
        Ok(out)
    }

    /// D1b: the id namespaces present in this KB, from the bulk title listing.
    fn namespace_prefixes(&self) -> Result<Vec<String>, KbStoreError> {
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (id, _title) in self.id_title_pairs_impl(None) {
            if let Some((prefix, _)) = id.split_once(':') {
                if !prefix.is_empty() {
                    seen.insert(prefix.to_string());
                }
            }
        }
        Ok(seen.into_iter().collect())
    }

    /// D1b: served by `kb/query.health`.
    ///
    /// Scoped honestly — this is **the shape of the corpus the hub holds**, not
    /// the health of the hub process. Orphan and broken-link detection needs the
    /// whole link graph, so the hub withholds both rather than reporting them
    /// from a partial scan: a node whose only backlink lies past the cap is not
    /// an orphan, and saying it is would be a wrong answer rather than a partial
    /// one. A truncated response is surfaced through `degraded()`.
    fn health_report(&self) -> Result<Option<crate::store::HealthReport>, KbStoreError> {
        let Some(result) = self.call("kb/query.health", serde_json::json!({})) else {
            return Ok(None);
        };
        if let Some(reason) = result.get("unavailable_reason").and_then(|v| v.as_str()) {
            self.set_outcome(LastOutcome::MalformedResponse(reason.to_string()));
            return Ok(None);
        }
        if truncated(&result) {
            self.set_outcome(LastOutcome::MalformedResponse(
                "health report hit the hub's max_scan_nodes cap; orphan and broken-link \
                 detection were withheld rather than computed from a partial scan"
                    .to_string(),
            ));
        }
        Ok(Some(crate::store::HealthReport {
            total_nodes: usize_at(&result, "total_nodes"),
            total_links: usize_at(&result, "total_links"),
            namespace_counts: count_map(&result, "namespace_counts"),
            by_kind: count_map(&result, "by_kind"),
            by_rel_type: std::collections::HashMap::new(),
            orphan_ids: string_list(&result, "orphan_ids"),
            broken_links: Vec::new(),
            hub_nodes: hub_pairs(&result),
            by_instance: std::collections::HashMap::new(),
        }))
    }

    /// D1b: served by `kb/query.agenda` with `filter=todo`.
    fn todo_nodes(&self) -> Result<Vec<Node>, KbStoreError> {
        Ok(self.agenda_impl("todo", None))
    }

    /// D1b: served by `kb/query.agenda`.
    ///
    /// **`AgendaFilter::Custom` is not sent**, and that is a decision rather than
    /// an omission: the hub's agenda endpoint is served from the CRDT DocStore,
    /// which has no Datalog engine behind it, and C3 established that arbitrary
    /// Datalog is a privileged capability. ADR-085's rule applies — *not offered*
    /// beats offered-and-denied — so the unsupported filters return empty with
    /// `degraded()` set, never a fabricated result.
    fn agenda(&self, filter: &crate::AgendaFilter) -> Result<Vec<Node>, KbStoreError> {
        use crate::AgendaFilter as F;
        let (name, value) = match filter {
            F::Todo(state) => ("todo", state.clone()),
            F::Priority(p) => ("priority", Some(p.to_string())),
            F::Tag(t) => ("tag", Some(t.clone())),
            F::Orphan => ("orphan", None),
            F::DeadEnd => ("dead-end", None),
            other => {
                self.set_outcome(LastOutcome::MalformedResponse(format!(
                    "agenda filter {other:?} is not served over ADR-053's query surface"
                )));
                return Ok(Vec::new());
            }
        };
        Ok(self.agenda_impl(name, value.as_deref()))
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError> {
        Ok(self.id_title_pairs_impl(prefix))
    }

    /// D1: served by `kb/query.neighborhood`, a real per-node BFS rather than
    /// `kb/query.graph`'s flat whole-KB dump.
    ///
    /// The hub clamps `depth` to 1..=3 and spends ONE `max_scan_nodes` budget
    /// across the whole traversal, so total server work is bounded regardless of
    /// depth -- each hop otherwise runs a backlink scan.
    fn neighborhood(
        &self,
        id: &str,
        depth: u32,
    ) -> Result<Option<crate::store::SubGraph>, KbStoreError> {
        let Some(result) = self.call(
            "kb/query.neighborhood",
            serde_json::json!({"node_id": id, "depth": depth}),
        ) else {
            return Ok(None);
        };
        if let Some(reason) = result.get("unavailable_reason").and_then(|v| v.as_str()) {
            self.set_outcome(LastOutcome::MalformedResponse(reason.to_string()));
            return Ok(None);
        }
        if result
            .get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            self.set_outcome(LastOutcome::MalformedResponse(
                "neighborhood traversal hit the hub's max_scan_nodes cap; the subgraph is partial"
                    .to_string(),
            ));
        }
        let nodes = result
            .get("nodes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|n| {
                        let a = n.as_array()?;
                        Some((
                            a.first()?.as_str()?.to_string(),
                            a.get(1)?.as_str()?.to_string(),
                        ))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let edges = result
            .get("edges")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        let a = e.as_array()?;
                        Some((
                            a.first()?.as_str()?.to_string(),
                            a.get(1)?.as_str()?.to_string(),
                            "links_to".to_string(),
                        ))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(Some(crate::store::SubGraph { nodes, edges }))
    }
}

/// Response helpers shared by the D1b endpoints.
fn truncated(v: &serde_json::Value) -> bool {
    v.get("truncated")
        .and_then(|t| t.as_bool())
        .unwrap_or(false)
}

fn usize_at(v: &serde_json::Value, key: &str) -> usize {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0) as usize
}

fn string_list(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn count_map(v: &serde_json::Value, key: &str) -> std::collections::HashMap<String, usize> {
    v.get(key)
        .and_then(|x| x.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, n)| n.as_u64().map(|n| (k.clone(), n as usize)))
                .collect()
        })
        .unwrap_or_default()
}

fn hub_pairs(v: &serde_json::Value) -> Vec<(String, usize)> {
    v.get("hub_nodes")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|h| {
                    Some((
                        h.get("id")?.as_str()?.to_string(),
                        h.get("in_degree")?.as_u64()? as usize,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One agenda row from the hub as a `Node`.
fn agenda_node(v: &serde_json::Value) -> Option<Node> {
    let id = v.get("id")?.as_str()?.to_string();
    let title = v
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let mut node = Node::new(id, title, NodeKind::Note, String::new());
    node.tags = v
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    node.todo_state = v
        .get("todo_state")
        .and_then(|t| t.as_str())
        .map(str::to_string);
    node.priority = v
        .get("priority")
        .and_then(|t| t.as_str())
        .and_then(|p| p.chars().next());
    Some(node)
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

    // -----------------------------------------------------------------------
    // D1 -- the four gaps this layer used to declare and now closes.
    // -----------------------------------------------------------------------

    fn rpc_body(result: serde_json::Value) -> String {
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string()
    }

    /// Backlinks: the direction must survive the wire, not just the count.
    ///
    /// A `links_to` edge is stored as `(other -> id)`. Getting that backwards
    /// would still produce one `Link` and pass a length assertion, which is why
    /// this pins `src`/`dst` explicitly.
    #[test]
    fn links_to_reconstructs_the_edge_in_the_right_direction() {
        let addr = spawn_one_shot_mock(
            "HTTP/1.1 200 OK",
            &rpc_body(serde_json::json!({
                "kb_id": "k", "node_id": "b", "encryption": "none",
                "links_from": [], "links_to": ["c"], "truncated": false
            })),
        );
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );
        let links = layer.links_to("b").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].src, "c",
            "the OTHER node is the source of a backlink"
        );
        assert_eq!(links[0].dst, "b");
    }

    /// A hub that truncated its scan must leave the layer visibly degraded.
    ///
    /// This is the difference between "no backlinks" and "we did not finish
    /// looking" -- a caller acts on the first and should not be told it when the
    /// second is true.
    #[test]
    fn a_truncated_backlink_response_marks_the_layer_degraded() {
        let addr = spawn_one_shot_mock(
            "HTTP/1.1 200 OK",
            &rpc_body(serde_json::json!({
                "kb_id": "k", "node_id": "b", "encryption": "none",
                "links_from": [], "links_to": [], "truncated": true
            })),
        );
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );
        let links = layer.links_to("b").unwrap();
        assert!(links.is_empty());
        assert!(
            matches!(layer.last_outcome(), LastOutcome::MalformedResponse(_)),
            "an empty-because-truncated result must not look like an authoritative empty"
        );
    }

    /// An E2E KB's structural refusal must reach the caller as degraded, not as
    /// a confident empty answer.
    #[test]
    fn an_e2e_refusal_is_surfaced_rather_than_read_as_no_links() {
        let addr = spawn_one_shot_mock(
            "HTTP/1.1 200 OK",
            &rpc_body(serde_json::json!({
                "kb_id": "k", "node_id": "b", "encryption": "e2e",
                "links_from": [], "links_to": [], "truncated": false,
                "unavailable_reason": "links are inside the encrypted node document"
            })),
        );
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );
        assert!(layer.links_from("b").unwrap().is_empty());
        assert!(matches!(
            layer.last_outcome(),
            LastOutcome::MalformedResponse(_)
        ));
    }

    /// The bulk title endpoint, decoded into the `(id, title)` pairs the trait
    /// promises -- one call, no N+1.
    #[test]
    fn id_title_pairs_decodes_the_bulk_response() {
        let addr = spawn_one_shot_mock(
            "HTTP/1.1 200 OK",
            &rpc_body(serde_json::json!({
                "kb_id": "k", "encryption": "none",
                "pairs": [["a", "Alpha"], ["b", "Beta"]], "truncated": false
            })),
        );
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );
        let pairs = layer.id_title_pairs(None).unwrap();
        assert_eq!(
            pairs,
            vec![
                ("a".to_string(), "Alpha".to_string()),
                ("b".to_string(), "Beta".to_string())
            ]
        );
    }

    /// The subgraph carries titles and typed-ish edges, and a truncated walk is
    /// flagged.
    #[test]
    fn neighborhood_decodes_nodes_edges_and_flags_truncation() {
        let addr = spawn_one_shot_mock(
            "HTTP/1.1 200 OK",
            &rpc_body(serde_json::json!({
                "kb_id": "k", "root": "a", "encryption": "none",
                "nodes": [["a", "Alpha"], ["b", "Beta"]],
                "edges": [["a", "b"]],
                "truncated": true
            })),
        );
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );
        let sub = layer.neighborhood("a", 1).unwrap().expect("a subgraph");
        assert_eq!(sub.nodes.len(), 2);
        assert_eq!(sub.edges, vec![("a".into(), "b".into(), "links_to".into())]);
        assert!(matches!(
            layer.last_outcome(),
            LastOutcome::MalformedResponse(_)
        ));
    }

    /// The countable gate D1 is measured against: these four are no longer gaps.
    ///
    /// Asserting the CLOSED set rather than the remaining one, so the test does
    /// not have to be edited every time another endpoint lands.
    #[test]
    fn the_four_endpoints_this_change_added_are_no_longer_declared_gaps() {
        use crate::capabilities::QueryMethod as M;
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config("http://127.0.0.1:1".to_string()),
            Duration::from_secs(1),
        );
        let caps = layer.capabilities();
        for m in [M::LinksFrom, M::LinksTo, M::Neighborhood, M::IdTitlePairs] {
            assert!(
                caps.supports(m),
                "{m:?} is served by a real endpoint now and must not be declared a gap"
            );
        }
        // ...and the ones that stay declared are the STRUCTURAL pair, so the set
        // is an honest inventory rather than an optimistic one. (HealthReport and
        // Agenda moved to the supported side in D1b — see the gap-count test.)
        assert!(!caps.supports(M::History));
        assert!(!caps.supports(M::NodeCrdtState));
    }

    // -- D1b: the last three closeable gaps ---------------------------------

    /// `todo_nodes` is `kb/query.agenda` with `filter=todo`, and it must come back
    /// as real `Node`s carrying the fields an agenda is *for* — a row whose
    /// `todo_state` was dropped in translation is indistinguishable from a node
    /// with no todo state.
    #[test]
    fn todo_nodes_decode_with_their_state_priority_and_tags() {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "kb_id": "test-kb", "filter": "todo", "scanned": 2, "truncated": false,
                "nodes": [
                    {"id": "task:1", "title": "Ship it", "kind": "task",
                     "todo_state": "TODO", "priority": "A", "tags": ["release"]},
                    {"id": "task:2", "title": "Later", "kind": "task",
                     "todo_state": "WAITING", "priority": null, "tags": []}
                ]
            }
        })
        .to_string();
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        let nodes = layer.todo_nodes().unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].todo_state.as_deref(), Some("TODO"));
        assert_eq!(nodes[0].priority, Some('A'));
        assert_eq!(nodes[0].tags, vec!["release".to_string()]);
        assert_eq!(nodes[1].todo_state.as_deref(), Some("WAITING"));
        assert_eq!(nodes[1].priority, None);
        assert_eq!(layer.last_outcome(), LastOutcome::Ok);
    }

    /// **A capped agenda that reads as complete is a wrong answer**, not a partial
    /// one: "nothing is due" is what a silently-short list says.
    #[test]
    fn a_truncated_agenda_marks_the_layer_degraded() {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "kb_id": "test-kb", "filter": "todo", "scanned": 500, "truncated": true,
                "nodes": [{"id": "task:1", "title": "One", "kind": null,
                           "todo_state": "TODO", "priority": null, "tags": []}]
            }
        })
        .to_string();
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        let nodes = layer.todo_nodes().unwrap();
        assert_eq!(nodes.len(), 1, "the rows that WERE found still come back");
        assert!(
            layer.degraded(),
            "but the caller must be able to tell the list is partial"
        );
    }

    /// **`Custom` Datalog is never sent to the hub.** C3 established that
    /// arbitrary Datalog is a privileged capability, and this endpoint has no
    /// Datalog engine behind it at all. Refused locally — ADR-085's *not offered*
    /// beats offered-and-denied — so no request is made and no result is invented.
    #[test]
    fn a_custom_datalog_agenda_is_refused_locally_and_never_reaches_the_hub() {
        // Port 1 is unbound: if this made a network call the test would hang or
        // fail on connect. Reaching a clean empty result proves it did not.
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config("http://127.0.0.1:1".to_string()),
            Duration::from_secs(1),
        );

        let out = layer
            .agenda(&crate::AgendaFilter::Custom("?[x] := *nodes{id: x}".into()))
            .unwrap();

        assert!(out.is_empty(), "no result may be fabricated");
        assert!(
            layer.degraded(),
            "and the caller must be told the filter was not served, not handed a              confident empty agenda"
        );
    }

    /// The health report decodes, and the fields the hub CAN answer are populated.
    #[test]
    fn health_report_decodes_counts_and_hub_nodes() {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "kb_id": "test-kb", "total_nodes": 3, "scanned": 3, "total_links": 2,
                "by_kind": {"note": 3},
                "namespace_counts": {"note": 3},
                "orphan_ids": ["note:lonely"],
                "broken_links": [],
                "hub_nodes": [{"id": "note:hub", "in_degree": 2}],
                "truncated": false
            }
        })
        .to_string();
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        let report = layer.health_report().unwrap().expect("a report is served");
        assert_eq!(report.total_nodes, 3);
        assert_eq!(report.total_links, 2);
        assert_eq!(report.orphan_ids, vec!["note:lonely".to_string()]);
        assert_eq!(report.hub_nodes, vec![("note:hub".to_string(), 2)]);
        assert_eq!(report.namespace_counts.get("note"), Some(&3));
        assert!(!layer.degraded());
    }

    /// **A truncated health report withholds orphans rather than inventing them.**
    /// A node whose only backlink lies past the cap is not an orphan, and naming
    /// it one is a confidently wrong answer.
    #[test]
    fn a_truncated_health_report_is_degraded_and_names_no_orphans() {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "kb_id": "test-kb", "total_nodes": 9000, "scanned": 500,
                "total_links": 120, "by_kind": {}, "namespace_counts": {},
                "orphan_ids": [], "broken_links": [], "hub_nodes": [],
                "truncated": true
            }
        })
        .to_string();
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        let report = layer.health_report().unwrap().unwrap();
        assert_eq!(report.total_nodes, 9000);
        assert!(report.orphan_ids.is_empty());
        assert!(
            layer.degraded(),
            "the caller must know the scan did not cover the corpus"
        );
    }

    /// An E2E KB's agenda is structurally unanswerable — the daemon is key-blind
    /// (ADR-037). Surfaced as a refusal, never as an empty agenda.
    #[test]
    fn an_e2e_agenda_refusal_is_surfaced_rather_than_read_as_nothing_due() {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "kb_id": "test-kb", "filter": "todo", "nodes": [], "truncated": false,
                "unavailable_reason": "todo state lives inside the encrypted node document"
            }
        })
        .to_string();
        let addr = spawn_one_shot_mock("HTTP/1.1 200 OK", &body);
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );

        assert!(layer.todo_nodes().unwrap().is_empty());
        assert!(
            layer.degraded(),
            "an empty agenda and 'the server cannot answer' must not look alike"
        );
    }

    /// D1b: in-degree is counted from the graph endpoint's edges — no new server
    /// call, no N+1.
    #[test]
    fn linked_in_degree_counts_incoming_edges_from_the_graph_response() {
        let addr = spawn_one_shot_mock(
            "HTTP/1.1 200 OK",
            &rpc_body(serde_json::json!({
                "kb_id": "k", "encryption": "none",
                "nodes": ["a", "b", "c"],
                "edges": [["a", "c"], ["b", "c"], ["a", "b"]],
                "edges_truncated": false
            })),
        );
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );
        let deg = layer.linked_in_degree().unwrap();
        assert_eq!(deg.get("c"), Some(&2), "c is pointed at twice");
        assert_eq!(deg.get("b"), Some(&1));
        assert_eq!(
            deg.get("a"),
            None,
            "a is only ever a SOURCE -- counting it would invert the edge"
        );
    }

    /// A truncated edge set produces UNDERCOUNTS, which read as "barely linked" —
    /// a wrong answer, not a partial one. It must mark the layer degraded.
    #[test]
    fn a_truncated_edge_set_marks_in_degree_counts_degraded() {
        let addr = spawn_one_shot_mock(
            "HTTP/1.1 200 OK",
            &rpc_body(serde_json::json!({
                "kb_id": "k", "encryption": "none",
                "nodes": ["a"], "edges": [["a", "b"]], "edges_truncated": true
            })),
        );
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );
        let _ = layer.linked_in_degree().unwrap();
        assert!(matches!(
            layer.last_outcome(),
            LastOutcome::MalformedResponse(_)
        ));
    }

    /// Namespaces come from the bulk title listing, deduplicated and sorted.
    #[test]
    fn namespace_prefixes_are_derived_from_node_ids() {
        let addr = spawn_one_shot_mock(
            "HTTP/1.1 200 OK",
            &rpc_body(serde_json::json!({
                "kb_id": "k", "encryption": "none",
                "pairs": [["concept:a", "A"], ["cmd:b", "B"], ["concept:c", "C"], ["nocolon", "D"]],
                "truncated": false
            })),
        );
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config(format!("http://{addr}")),
            Duration::from_secs(5),
        );
        assert_eq!(
            layer.namespace_prefixes().unwrap(),
            vec!["cmd".to_string(), "concept".to_string()],
            "deduplicated, sorted, and an id with no namespace contributes none"
        );
    }

    /// The gate D1 is measured against, stated as a countable set.
    ///
    /// Asserts what is CLOSED and what remains, and — crucially — that the two
    /// structural gaps stay declared. `History` (ADR-106: version history is a
    /// local audit trail that does not sync) and `NodeCrdtState` (serving raw
    /// CRDT bytes IS replication, the thing ADR-067's QueryOnly withholds) can
    /// never be closed. A gap list nobody can empty is a gap list people stop
    /// believing.
    #[test]
    fn the_declared_gap_set_is_down_to_six_and_two_of_those_are_structural() {
        use crate::capabilities::QueryMethod as M;
        let layer = RemoteHubQueryLayer::with_timeout(
            test_config("http://127.0.0.1:1".to_string()),
            Duration::from_secs(1),
        );
        let caps = layer.capabilities();
        for m in [
            M::LinkedInDegree,
            M::Related,
            M::NamespacePrefixes,
            M::HealthReport,
            M::TodoNodes,
            M::Agenda,
        ] {
            assert!(
                caps.supports(m),
                "{m:?} is served now and must not be a gap"
            );
        }
        for m in [M::History, M::NodeCrdtState] {
            assert!(
                !caps.supports(m),
                "{m:?} is STRUCTURALLY unavailable and must stay declared -- \
                 implementing it would defeat ADR-106 or ADR-067 respectively"
            );
        }
        assert_eq!(
            caps.gaps().len(),
            2,
            "D1b closed HealthReport, TodoNodes and Agenda, so the ONLY remaining \
             gaps are the two structural ones — the declared-gap set is now as \
             empty as it can be made. Update this count deliberately, not to make \
             it pass."
        );
    }
}
