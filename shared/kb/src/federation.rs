//! KB Federation — multi-KB registry and cross-instance operations.
//!
//! CozoDB is the durable source of truth for KB data.
//! Org directories are an import/export format, not the runtime store.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::org::parse_org_multi_result;
use crate::store::KbStoreError;
use crate::{KnowledgeBase, Node};

/// AI-provider residency policy for a KB (ADR-048). A **provider-trust** axis — deliberately
/// not named `LocalOnly` to avoid colliding with [`KbScope::LocalOnly`] below, which is an
/// unrelated **network-locality** axis ("only the primary/local instance participates in this
/// query"). `LocalModelsOnly` here means "no hosted/cloud AI provider may read or write this
/// KB's content, only a locally-classified model (e.g. Ollama) may" — enforced at tool
/// dispatch, not a network property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AiResidency {
    /// No provider restriction (status quo, default — non-breaking for existing KBs).
    #[default]
    Open,
    /// Only a locally-classified AI provider (e.g. Ollama) may read/write this KB's content.
    LocalModelsOnly,
}

/// AI provider names MAE classifies as local (self-hosted). Single source of truth —
/// relocated here from `mae-core` (ADR-061 Phase A) so the daemon workspace, which does not
/// depend on `mae-core` (ADR-014's two-workspace split), can also consult it: enrichment's
/// embedding-provider residency gate must run from `daemon/src/scheduler.rs`'s background
/// sweep, which has no `Editor` and cannot reach `crates/core`. `shared/kb` (`mae-kb`) is the
/// closest common crate both the editor workspace (via `mae-core`) and the daemon workspace
/// already depend on directly, and it already owns `AiResidency` — so the provider-residency
/// predicate now lives right next to the policy enum it decides against. `mae-core::ai_
/// residency` re-exports this rather than duplicating it.
pub const LOCAL_AI_PROVIDERS: &[&str] = &["ollama"];

/// Is `provider` one MAE classifies as local (self-hosted)?
pub fn is_local_provider(provider: &str) -> bool {
    LOCAL_AI_PROVIDERS.contains(&provider)
}

/// Does `residency` permit `provider` to read/write this KB's content? The core
/// provider-vs-residency decision, usable from both the editor (via `mae-core::ai_residency::
/// check_kb_residency`, which already has KB/tool context to gather `residency` from) and the
/// daemon (which can read a `KbInstance`'s `ai_residency` field directly, with no `Editor` in
/// the loop at all). `Open` always permits; `LocalModelsOnly` permits only a locally-classified
/// provider.
pub fn residency_permits_provider(residency: AiResidency, provider: &str) -> bool {
    match residency {
        AiResidency::Open => true,
        AiResidency::LocalModelsOnly => is_local_provider(provider),
    }
}

/// What role a registered `KbInstance` plays (ADR-058 Phase A). Purely descriptive — nothing
/// in the query/storage layer branches on it yet; `KbScope::Project` (Phase C) is the first
/// consumer. `#[serde(default)]` on the `kind` field below means every registry file written
/// before this field existed deserializes every non-primary entry as `UserRegistered` — this is
/// intentionally correct, not a placeholder: see [`KbInstance::effective_kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KbInstanceKind {
    /// The single machine-global primary KB. Never actually stored in a `KbInstance.kind`
    /// field in practice (the primary has no `KbInstance` row of its own — see
    /// `KbRegistry`'s doc comment on `primary_shared`); exists as a variant so
    /// `effective_kind()` has a value to return for the primary case, and so downstream
    /// matches over `KbInstanceKind` are exhaustive without a bolted-on `Option`.
    Primary,
    /// Scoped to a single project root (`project_root`, Phase A/C).
    Project,
    /// The dev-practices/guidance KB (`ai_guidance_kb`) — a distinguished role orthogonal to
    /// project scoping (ADR-057 row 8 / ADR-058's own Decision D: guidance-KB reachability is
    /// untouched by `KbScope::Project`, so this variant matters for *identification*, not for
    /// exclusion from project-scoped queries).
    Guidance,
    /// Any other explicitly user-registered instance (`kb_register`) — the default, and the
    /// only value that existed before this field, preserving today's behavior exactly for
    /// every already-registered non-primary KB.
    #[default]
    UserRegistered,
    /// A hub KB reachable only via ADR-053's live scoped read-through `kb/query.*`
    /// surface (ADR-062 Phase C) — never mirrored locally as a Cozo copy (the Hard Rule:
    /// see `RemoteHubConfig`'s doc comment). Structurally distinct from `shared`/
    /// `collab_id` (ADR-019 push/pull collaborative sync of a *local* store): a
    /// `RemoteHub` instance has no local store at all, only a live connection to a
    /// remote one.
    RemoteHub,
}

/// How a `RemoteHub` instance's bearer token (ADR-052 OAuth2.1) is obtained at call time.
/// A *reference*, never the raw token — mirroring `collab_bridge::resolve_client_credential`'s
/// existing `cmd:`-sentinel-or-keystore-key precedent for peer auth (`crates/mae/src/collab_bridge/mod.rs`)
/// rather than inventing a new storage shape. A reference (not a cached token) is required
/// by ADR-062 Phase D's own verification bar: an expired/revoked token must be caught at
/// the point of use, which means re-resolving it per call, not trusting a value stored here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteHubAuth {
    /// Run an external command at call time and use its stdout (trimmed) as the bearer
    /// token — same convention as collab's `cmd:` credential sentinel.
    Command(String),
    /// Resolve from a named entry in the collab keystore (`mae_mcp::keystore`).
    KeystoreKey(String),
}

/// Connection info for a `RemoteHub`-kind instance (ADR-062 Phase C). `None` for every
/// other kind.
///
/// **Hard rule (ADR-062, following org-roam's own established best practice of never
/// syncing the derived DB itself — see the ADR's Context section):** a `RemoteHub`
/// instance is *always* queried live through this connection info by Phase D's bridging
/// code; nothing here ever seeds a local Cozo copy of the hub's content. This struct
/// describes how to reach the hub on each call, not a cache of what it contains.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteHubConfig {
    /// The hub daemon's OAuth-HTTPS-listener origin (e.g. `https://kb.example.org:8443`).
    pub base_url: String,
    /// The `kb_id` to pass in every `kb/query.*` call — the hub-side collaborative KB
    /// id (`kbc:{kb_id}`), independent of this instance's own local `uuid`.
    pub hub_kb_id: String,
    pub auth: RemoteHubAuth,
}

/// A registered KB instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KbInstance {
    pub uuid: String,
    pub name: String,
    /// Meaningless for a `RemoteHub`-kind instance (no local store) — left as an empty
    /// `PathBuf` (`register_remote_hub` sets it that way) rather than made `Option`, since
    /// every other kind relies on it being a plain, always-present `PathBuf` and 30+
    /// call sites across the codebase already assume that. Don't read this field for a
    /// `RemoteHub` instance; check `kind`/`remote_hub` first.
    pub org_dir: PathBuf,
    /// Meaningless for a `RemoteHub`-kind instance — same empty-`PathBuf` convention as
    /// `org_dir` above.
    pub db_path: PathBuf,
    pub primary: bool,
    pub enabled: bool,
    pub last_import: Option<String>,
    /// Collaborative KB identity (FNV-1a hash of name + creator).
    /// Present only for shared KBs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collab_id: Option<String>,
    /// Whether this KB is shared with peers.
    #[serde(default)]
    pub shared: bool,
    /// Connected peers for this KB.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_peers: Vec<String>,
    /// Last sync timestamp (ISO 8601).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
    /// AI-provider residency policy (ADR-048). `#[serde(default)]` for backward compat with
    /// registry files written before this field existed — they load as `Open`, preserving
    /// today's behavior for every already-registered KB.
    #[serde(default)]
    pub ai_residency: AiResidency,
    /// The project root this instance is scoped to (ADR-058 Phase A), when it's a
    /// `Project`-kind instance. `#[serde(default)]` — absent/`None` for every instance
    /// registered before this field existed, and for non-project-scoped kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    /// This instance's role (ADR-058 Phase A). Prefer `effective_kind()` over reading this
    /// field directly — see that method's doc comment for why.
    #[serde(default)]
    pub kind: KbInstanceKind,
    /// Federated search priority (ADR-062 Phase B). Higher wins when two instances'
    /// results collide on the same node id — replaces the previous implicit "whichever
    /// instance was registered/iterated first" rule with an explicit, user-controllable
    /// one. `#[serde(default)]` — every pre-062 instance defaults to `0` (equal weight),
    /// preserving today's behavior (ties still resolve by iteration order) until a user
    /// deliberately raises an instance's priority.
    #[serde(default)]
    pub priority: u32,
    /// Connection info when `kind == RemoteHub`; `None` otherwise. `#[serde(default,
    /// skip_serializing_if = "Option::is_none")]` — every pre-062 registry entry
    /// round-trips unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_hub: Option<RemoteHubConfig>,
}

impl KbInstance {
    /// Whether this instance is shared/remote (collaborative). Used by
    /// `KbScope::RemoteOnly` to select only network-backed instances. A `RemoteHub`
    /// instance (ADR-062 Phase C) is definitionally remote — it has no local store at
    /// all — so it counts here alongside the pre-existing ADR-019 push/pull sync markers.
    pub fn is_remote(&self) -> bool {
        self.shared
            || self.collab_id.is_some()
            || !self.remote_peers.is_empty()
            || self.kind == KbInstanceKind::RemoteHub
    }

    /// This instance's role. Simply the stored `kind` field (which serde has already
    /// defaulted to `UserRegistered` for any registry file predating this field — no
    /// migration step needed).
    ///
    /// **Deliberately does NOT special-case `self.primary`**, correcting an assumption an
    /// earlier version of this method made — caught by a real 3-way-concurrent adversarial
    /// test (`kb_init_project_converges_to_one_instance_under_a_three_way_race`) exercising
    /// the exact case where they collide: `primary: bool` (set by `register()` as
    /// `self.instances.is_empty()`, above) means "this was the first-ever `KbInstance` row
    /// registered on this machine" — an artifact of registration *order*, not an alias for
    /// `KbInstanceKind::Primary`. The real, machine-global primary KB has no `KbInstance` row
    /// at all (see `KbRegistry`'s `primary_shared`/`primary_ai_residency` doc comments — its
    /// durable metadata lives on `KbRegistry` directly, precisely because it isn't one of
    /// `instances`). Treating `primary: true` as `effective_kind() == Primary` was therefore
    /// never correct: it silently reclassified the first `Project`/`Guidance`-kind instance
    /// ever registered on a machine back to `Primary`, exactly the scenario the adversarial
    /// test above hits (the very first project a user provisions).
    pub fn effective_kind(&self) -> KbInstanceKind {
        self.kind
    }

    /// Whether this instance participates in `KbScope::Project(root)` (ADR-058 Phase C): a
    /// `Project`-kind instance whose own `project_root` equals `root` exactly. Comparison is
    /// a plain path equality — callers on both the registration side (Phase B) and the
    /// resolution side (Phase C) are expected to pass an already-canonicalized path (the same
    /// discipline `KbRegistry::register`'s own `org_dir` canonicalization already requires),
    /// so two paths naming the same directory via different spellings don't silently fail to
    /// match, and — the sibling risk — two *different* directories don't silently collide.
    pub fn matches_project_root(&self, root: &Path) -> bool {
        self.effective_kind() == KbInstanceKind::Project
            && self.project_root.as_deref() == Some(root)
    }
}

/// Which federated KB instances participate in a search/traversal query.
///
/// This is a query-time selector, not new plumbing (plan decision D4): it
/// filters which of the primary + registered instances contribute results.
/// Parsed from the `kb_search_scope` option / AI-tool `scope` argument.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KbScope {
    /// Primary (local) KB + all enabled instances. Default.
    #[default]
    All,
    /// Only the primary (local) KB.
    LocalOnly,
    /// Only shared/remote (collaborative) instances; skip the primary.
    RemoteOnly,
    /// A single instance addressed by name (matches the primary's name too).
    Named(String),
    /// Only `Project`-kind instances whose `project_root` equals the given, already-resolved
    /// path (ADR-058 Phase C). Excludes the primary — narrowing to "just this project" is the
    /// whole point, mirroring `RemoteOnly`'s existing precedent for scopes whose purpose is
    /// orthogonal to the machine-global primary. Carries the resolved root rather than being
    /// resolved lazily: `KbScope` has no access to `detect_project_root` (that lives in the
    /// `mae-core` crate, a layer above this one) — callers resolve fresh each time via
    /// `Editor::resolve_kb_scope`, so this always reflects the *current* project, never a
    /// stale cached one. `KbScope::parse` deliberately does NOT construct this variant (a bare
    /// string has no project-root context to attach) — see `Editor::resolve_kb_scope`.
    Project(PathBuf),
}

impl KbScope {
    /// Parse a scope token from config / AI-tool input.
    /// `"" | "all"` → All, `"local"` → LocalOnly, `"remote"` → RemoteOnly,
    /// anything else → `Named(<trimmed>)`. Does NOT handle the `"project"` token — that
    /// requires resolving a current project root, which this pure-string parser has no way
    /// to do; use `Editor::resolve_kb_scope` for a token that may be `"project"`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "all" => KbScope::All,
            "local" | "local-only" | "localonly" => KbScope::LocalOnly,
            "remote" | "remote-only" | "remoteonly" => KbScope::RemoteOnly,
            _ => KbScope::Named(s.trim().to_string()),
        }
    }

    /// Canonical token for persistence / display. `Project`'s specific root is intentionally
    /// NOT included — `kb_search_scope`'s persisted value is a general preference ("prefer
    /// project scope"), re-resolved against whatever project is current at each query, not a
    /// snapshot of one specific path.
    pub fn as_token(&self) -> String {
        match self {
            KbScope::All => "all".to_string(),
            KbScope::LocalOnly => "local".to_string(),
            KbScope::RemoteOnly => "remote".to_string(),
            KbScope::Named(n) => n.clone(),
            KbScope::Project(_) => "project".to_string(),
        }
    }
}

/// Registry of all known KB instances. Persisted as TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KbRegistry {
    pub instances: Vec<KbInstance>,
    /// Whether the **primary** KB is shared for collaboration (ADR-019). The
    /// primary KB has no `KbInstance` row, so its durable share marker lives
    /// here — making "is the primary KB syncing?" reconstructable across
    /// restarts instead of depending on a transient in-memory event.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub primary_shared: bool,
    /// Collaborative id the primary KB is shared under (when `primary_shared`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_collab_id: Option<String>,
    /// AI-provider residency policy for the **primary** KB (ADR-048). The primary KB has no
    /// `KbInstance` row, so — mirroring `primary_shared`/`primary_collab_id` above — its
    /// residency policy lives here instead.
    #[serde(default)]
    pub primary_ai_residency: AiResidency,
    /// Project roots (already-canonicalized, matching `Editor::resolve_kb_scope`'s and
    /// `register`'s own canonicalization discipline) the user has explicitly declined
    /// project-KB provisioning for (ADR-058 Phase E) — never re-prompt for these. Lives here,
    /// not in the global `OptionRegistry`, because it's a growing per-project *set*, not a
    /// single preference value; storing it in `KbRegistry` reuses this struct's own
    /// already-proven durable, concurrent-write-safe persistence (`KbRegistry::update`) rather
    /// than needing a second mechanism.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declined_project_provisioning: Vec<PathBuf>,
}

impl KbRegistry {
    /// Whether the user has already declined project-KB provisioning for `root` (ADR-058
    /// Phase E). `root` is expected already-canonicalized (same discipline as
    /// `matches_project_root`) — the caller is responsible for that, this is a plain lookup.
    pub fn has_declined_project(&self, root: &Path) -> bool {
        self.declined_project_provisioning
            .iter()
            .any(|p| p.as_path() == root)
    }

    /// Record a decline for `root` (idempotent — declining an already-declined root is a
    /// no-op, not a duplicate entry).
    pub fn decline_project(&mut self, root: PathBuf) {
        if !self.has_declined_project(&root) {
            self.declined_project_provisioning.push(root);
        }
    }

    /// Load registry from `~/.local/share/mae/kb-registry.toml`.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("kb-registry.toml");
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save registry to `~/.local/share/mae/kb-registry.toml`.
    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        let path = data_dir.join("kb-registry.toml");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self).map_err(|e| io::Error::other(e.to_string()))?;
        std::fs::write(&path, content)
    }

    /// Reload-fresh -> mutate -> save, under a cross-process advisory lock
    /// (see `mae_mcp::file_lock::with_locked_update`).
    ///
    /// Multiple `mae` processes commonly run concurrently (one per project
    /// directory). Calling `load()` once at startup and blindly `save()`-ing
    /// a long-held in-memory copy loses concurrent registrations from other
    /// processes — this happened for real (a KB registration was silently
    /// wiped by another process's stale save). `update()` always reloads the
    /// freshest on-disk registry immediately before applying `mutate`, so a
    /// save reflects "current disk state + my change", not "my stale
    /// snapshot + my change". Callers should replace their long-lived
    /// in-memory registry with the returned value so it also absorbs any
    /// concurrent additions from other processes.
    ///
    /// Returns the merged registry, whatever `mutate` returned, and the save
    /// outcome as a separate `Result` — a save failure does NOT discard the
    /// in-memory mutation (matches the pre-existing best-effort persistence
    /// semantics: callers always apply the change in memory, and log rather
    /// than abort on a disk-write failure).
    pub fn update<R>(
        data_dir: &Path,
        mutate: impl FnOnce(&mut Self) -> R,
    ) -> (Self, R, io::Result<()>) {
        let path = data_dir.join("kb-registry.toml");
        mae_mcp::file_lock::with_locked_update(
            &path,
            || Self::load(data_dir),
            mutate,
            |reg| reg.save(data_dir),
        )
    }

    /// Register a new org-roam directory.
    ///
    /// If a `KbDataDir` is provided, the SQLite database is placed in the
    /// standardized `kb/local/{slug}/kb.sqlite` layout. Otherwise falls back
    /// to the legacy `{data_dir}/{uuid}.db` flat layout.
    pub fn register(
        &mut self,
        name: String,
        org_dir: PathBuf,
        data_dir: &Path,
        kb_data_dir: Option<&crate::data_dir::KbDataDir>,
    ) -> Result<String, String> {
        // A system-KB name (`crate::system_kb`) is MAE's own, and answers to
        // exactly one corpus. Refused here rather than at `Editor::kb_register`
        // so every caller is covered — the daemon registers instances too, and
        // an enforcement point one layer up is one an alternate path walks past.
        //
        // This also fixes a real ambiguity rather than merely adding a rule:
        // the duplicate check below matches on `org_dir` and never on `name`,
        // so registering a second `DevPractices` used to append a shadowed row
        // that `find()` would never return. ADR-076 D4's documented "your own
        // registration always wins" therefore held only if you happened to
        // register BEFORE startup auto-registration ran. Reserving the name
        // makes the answer order-independent: to override MAE's practices with
        // your own, register under your own name and point `ai_guidance_kb` at
        // it — an explicit choice that still wins, and one that says so.
        if crate::system_kb::is_reserved_name(&name) {
            return Err(format!(
                "'{name}' is a reserved MAE system KB name. Register your own KB under a \
                 different name, then point `ai_guidance_kb` at it if you want it used as \
                 guidance."
            ));
        }

        // Canonicalize so a symlinked/relative/non-normalized path registers
        // to the same, stable location every time (#303) — otherwise a
        // node's `source_file` (stamped from the walked, canonical-ish path
        // at import time) can drift from what's stored here as `org_dir`,
        // surfacing later as a spurious ENOENT in `help_edit_source`. Falls
        // back to the given path if it doesn't exist yet (canonicalize
        // requires the path to exist) — registration shouldn't hard-fail on
        // a not-yet-existing directory.
        let org_dir = org_dir.canonicalize().unwrap_or(org_dir);

        // Check for existing registration with same path
        if let Some(existing) = self.instances.iter().find(|i| i.org_dir == org_dir) {
            return Ok(existing.uuid.clone());
        }

        // Check for sentinel file with existing UUID
        let uuid = read_sentinel_uuid(&org_dir).unwrap_or_else(generate_uuid);

        let slug = crate::data_dir::slugify(&name);
        let db_path = if let Some(kdd) = kb_data_dir {
            // Standardized layout: kb/local/{slug}/kb.sqlite
            let meta = crate::data_dir::LocalKbMeta {
                name: name.clone(),
                uuid: uuid.clone(),
                created_at: crate::data_dir::chrono_now_iso(),
                node_count: 0,
                org_dir: Some(org_dir.clone()),
            };
            match kdd.init_local_kb(&slug, &meta) {
                Ok(path) => path,
                Err(e) => {
                    tracing::warn!(error = %e, slug, "failed to init local KB dir, using legacy path");
                    data_dir.join(format!("{}.db", uuid))
                }
            }
        } else {
            // Legacy flat layout
            data_dir.join(format!("{}.db", uuid))
        };

        // Write sentinel file (idempotent)
        if let Err(e) = write_sentinel(&org_dir, &uuid, &name) {
            tracing::warn!(error = %e, uuid, "failed to write KB instance sentinel file");
        }

        let instance = KbInstance {
            uuid: uuid.clone(),
            name,
            org_dir,
            db_path,
            primary: self.instances.is_empty(),
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: AiResidency::default(),
            project_root: None,
            kind: KbInstanceKind::default(),
            priority: 0,
            remote_hub: None,
        };
        self.instances.push(instance);
        Ok(uuid)
    }

    /// Register a `RemoteHub`-kind instance (ADR-062 Phase C) — a hub KB reachable only
    /// via ADR-053's live `kb/query.*` surface, never mirrored locally. Unlike `register`,
    /// there's no local directory to canonicalize/sentinel-stamp or local `db_path` to
    /// allocate; `org_dir`/`db_path` are left as empty placeholders (see their doc
    /// comments on `KbInstance`). Idempotent on `(base_url, hub_kb_id)`, mirroring
    /// `register`'s own idempotence-on-`org_dir` behavior — registering the same hub
    /// twice returns the existing uuid rather than creating a duplicate row.
    pub fn register_remote_hub(
        &mut self,
        name: String,
        base_url: String,
        hub_kb_id: String,
        auth: RemoteHubAuth,
    ) -> String {
        if let Some(existing) = self.instances.iter().find(|i| {
            i.kind == KbInstanceKind::RemoteHub
                && i.remote_hub
                    .as_ref()
                    .is_some_and(|r| r.base_url == base_url && r.hub_kb_id == hub_kb_id)
        }) {
            return existing.uuid.clone();
        }

        let uuid = generate_uuid();
        let instance = KbInstance {
            uuid: uuid.clone(),
            name,
            org_dir: PathBuf::new(),
            db_path: PathBuf::new(),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: AiResidency::default(),
            project_root: None,
            kind: KbInstanceKind::RemoteHub,
            priority: 0,
            remote_hub: Some(RemoteHubConfig {
                base_url,
                hub_kb_id,
                auth,
            }),
        };
        self.instances.push(instance);
        uuid
    }

    /// Unregister an instance by name or UUID.
    pub fn unregister(&mut self, name_or_uuid: &str) {
        self.instances
            .retain(|i| i.name != name_or_uuid && i.uuid != name_or_uuid);
    }

    /// Set the AI-residency policy (ADR-048) for the primary/local KB (which has no
    /// `KbInstance` row, so it is addressed by any of [`PRIMARY_NAME_ALIASES`]) or a
    /// registered instance by name/UUID. Returns `false` if `name_or_uuid` names neither —
    /// the caller should treat that as "no such KB", not silently succeed.
    ///
    /// Accepted only `"primary"` until ADR-105 D4, which meant `"default"` — the name
    /// `KB_DEFAULT_NAME` gives the primary everywhere else in MAE, and the one users and
    /// the `kb_set_ai_residency` tool naturally pass — was rejected with "no instance
    /// found matching 'default'". A residency policy that silently declines to apply is
    /// worse than most bugs of this size: ADR-048 exists to keep a sensitive KB away from
    /// hosted models, and the caller was told the KB did not exist.
    pub fn set_ai_residency(&mut self, name_or_uuid: &str, policy: AiResidency) -> bool {
        if crate::kb_identity::PRIMARY_NAME_ALIASES
            .iter()
            .any(|a| name_or_uuid.eq_ignore_ascii_case(a))
        {
            self.primary_ai_residency = policy;
            return true;
        }
        match self.find_mut(name_or_uuid) {
            Some(inst) => {
                inst.ai_residency = policy;
                true
            }
            None => false,
        }
    }

    /// Find an instance by name or UUID.
    pub fn find(&self, name_or_uuid: &str) -> Option<&KbInstance> {
        self.instances
            .iter()
            .find(|i| i.name == name_or_uuid || i.uuid == name_or_uuid)
    }

    /// Find an instance by UUID.
    pub fn find_by_uuid(&self, uuid: &str) -> Option<&KbInstance> {
        self.instances.iter().find(|i| i.uuid == uuid)
    }

    /// Find a mutable instance by name or UUID (ADR-019: the share path stamps
    /// `shared`/`collab_id` durable markers).
    pub fn find_mut(&mut self, name_or_uuid: &str) -> Option<&mut KbInstance> {
        self.instances
            .iter_mut()
            .find(|i| i.name == name_or_uuid || i.uuid == name_or_uuid)
    }

    /// Find a shared instance by its collaborative id (ADR-019: receive-side
    /// routing + reconstruction resolve a `collab_id` back to its instance).
    pub fn find_by_collab_id(&self, collab_id: &str) -> Option<&KbInstance> {
        self.instances
            .iter()
            .find(|i| i.collab_id.as_deref() == Some(collab_id))
    }
}

/// Federated KB — wraps local KB plus imported instances.
#[derive(Debug, Default, Clone)]
pub struct FederatedKb {
    pub local: KnowledgeBase,
    pub instances: HashMap<String, KnowledgeBase>,
    pub registry: KbRegistry,
}

impl FederatedKb {
    pub fn new(local: KnowledgeBase) -> Self {
        FederatedKb {
            local,
            instances: HashMap::new(),
            registry: KbRegistry::default(),
        }
    }

    /// Search across local KB and all instances.
    pub fn search(&self, query: &str) -> Vec<(Option<&str>, &Node)> {
        let mut results: Vec<(Option<&str>, &Node)> = Vec::new();

        // Local KB first
        for id in self.local.search(query) {
            if let Some(node) = self.local.get(&id) {
                results.push((None, node));
            }
        }

        // Then each instance
        for (uuid, kb) in &self.instances {
            for id in kb.search(query) {
                if let Some(node) = kb.get(&id) {
                    results.push((Some(uuid.as_str()), node));
                }
            }
        }

        results
    }

    /// Get a node by ID, searching local first then instances.
    pub fn get(&self, id: &str) -> Option<(Option<&str>, &Node)> {
        if let Some(node) = self.local.get(id) {
            return Some((None, node));
        }
        for (uuid, kb) in &self.instances {
            if let Some(node) = kb.get(id) {
                return Some((Some(uuid.as_str()), node));
            }
        }
        None
    }

    /// Get from a specific instance.
    pub fn get_from_instance(&self, uuid: &str, id: &str) -> Option<&Node> {
        self.instances.get(uuid)?.get(id)
    }

    /// Resolve an `eor:` link.
    /// Format: `eor:node-id` (local-first) or `eor:uuid/node-id` (targeted).
    pub fn resolve_eor_link<'a>(&'a self, link: &'a str) -> Option<(Option<&'a str>, &'a Node)> {
        let link = link.strip_prefix("eor:").unwrap_or(link);

        if let Some(slash_pos) = link.find('/') {
            // Targeted: eor:uuid/node-id
            let uuid = &link[..slash_pos];
            let node_id = &link[slash_pos + 1..];
            if let Some(node) = self.get_from_instance(uuid, node_id) {
                return Some((Some(uuid), node));
            }
            return None;
        }

        // Local-first
        self.get(link)
    }

    /// Number of total nodes across all KBs.
    pub fn total_nodes(&self) -> usize {
        self.local.len() + self.instances.values().map(|kb| kb.len()).sum::<usize>()
    }

    /// List instance names and node counts.
    pub fn instance_summary(&self) -> Vec<(String, String, usize, bool)> {
        self.registry
            .instances
            .iter()
            .map(|inst| {
                let count = self
                    .instances
                    .get(&inst.uuid)
                    .map(|kb| kb.len())
                    .unwrap_or(0);
                (inst.uuid.clone(), inst.name.clone(), count, inst.enabled)
            })
            .collect()
    }
}

/// How to ingest an external KB directory.
///
/// @ai-caution: [architecture-debt] `Incremental` skips re-parsing (and
/// re-stamping any per-node metadata a code fix might newly add, e.g. the
/// `source_file` field — see the 2026-07 fix in `org.rs`'s `ingest_org_dir`
/// and this file's `import_org_dir`/`import_org_dir_to_store`) for any file
/// whose content hash hasn't changed since the last import. A node
/// persisted by an older version of this ingestion logic stays exactly as
/// stale as it was until a `Full` reimport runs — restarting the daemon
/// process alone does NOT fix this, since it just reloads the same
/// persisted rows. `Full` is the enum default, but callers reaching this
/// through anything that explicitly requests `Incremental` won't get the
/// fix. Tracked more broadly (the daemon has no way to surface "this
/// instance's persisted data predates the code currently running") in
/// https://github.com/cuttlefisch/mae/issues/323. Also cross-referenced in
/// ROADMAP.md's "Architecture Debt" section.
#[derive(Debug, Clone, Default)]
pub enum IngestMode {
    /// Re-parse all files. Existing nodes updated, deleted files' nodes removed.
    #[default]
    Full,
    /// Only re-parse files whose content hash has changed since last import.
    Incremental,
}

impl IngestMode {
    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "incremental" | "incr" => IngestMode::Incremental,
            _ => IngestMode::Full,
        }
    }
}

/// Import report from ingesting an org directory.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub nodes_imported: usize,
    pub nodes_skipped: usize,
    pub nodes_updated: usize,
    pub nodes_unchanged: usize,
    pub nodes_removed: usize,
    pub links_created: usize,
    pub duplicate_ids: Vec<(String, PathBuf)>,
    pub errors: Vec<(PathBuf, String)>,
    pub path_to_ids: Vec<(std::path::PathBuf, Vec<String>)>,
    pub mode: String,
    pub duration_ms: u64,
}

/// Health metrics computed after ingestion.
#[derive(Debug, Clone, Default)]
pub struct ImportHealth {
    pub total_nodes: usize,
    pub total_links: usize,
    pub orphan_count: usize,
    pub broken_link_count: usize,
    pub broken_link_deleted: usize,
    pub broken_link_malformed: usize,
    pub namespace_counts: HashMap<String, usize>,
}

impl ImportHealth {
    /// Compute health metrics from a freshly-imported KB.
    pub fn from_kb(kb: &KnowledgeBase) -> Self {
        let report = kb.health_report();
        Self {
            total_nodes: report.total_nodes,
            total_links: report.total_links,
            orphan_count: report.orphan_ids.len(),
            broken_link_count: report.broken_links.len(),
            broken_link_deleted: report
                .broken_links
                .iter()
                .filter(|b| b.kind == crate::BrokenLinkKind::DeletedNode)
                .count(),
            broken_link_malformed: report
                .broken_links
                .iter()
                .filter(|b| b.kind == crate::BrokenLinkKind::MalformedId)
                .count(),
            namespace_counts: report.namespace_counts,
        }
    }
}

/// Best-effort re-derivation of a node's source file when the stored,
/// possibly-stale absolute path (`stale_path` — stamped once at import time
/// and never re-validated, #303) no longer resolves on disk, but the owning
/// instance's *current* `org_dir` does. Searches `org_dir` for a `.org` file
/// with the same file name as `stale_path`; returns its path only if the
/// match is unambiguous (more than one file with that name anywhere under
/// `org_dir` is treated as unresolvable — reporting "no source file" is
/// safer than guessing wrong and opening an unrelated file).
pub fn resolve_stale_source_file(org_dir: &Path, stale_path: &Path) -> Option<PathBuf> {
    let file_name = stale_path.file_name()?;
    let mut found: Option<PathBuf> = None;
    for entry in walkdir::WalkDir::new(org_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file()
            && path.extension().and_then(|e| e.to_str()) == Some("org")
            && path.file_name() == Some(file_name)
        {
            if found.is_some() {
                return None; // ambiguous
            }
            found = Some(path.to_path_buf());
        }
    }
    found
}

/// Import an org-roam directory (recursively) into a MAE KB instance.
///
/// Uses `walkdir` to handle nested subdirectories. Skips the sentinel
/// file (`eor-instance.org`) and files without `:ID:` properties.
pub fn import_org_dir(org_dir: &Path) -> (KnowledgeBase, ImportReport, ImportHealth) {
    let mut kb = KnowledgeBase::new();
    let mut report = ImportReport::default();
    let mut seen_ids = std::collections::HashSet::new();
    let mut file_id_map: HashMap<PathBuf, Vec<String>> = HashMap::new();

    let walker = walkdir::WalkDir::new(org_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok());

    for entry in walker {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("org") {
            continue;
        }
        // Skip sentinel file
        if path.file_name().and_then(|n| n.to_str()) == Some("eor-instance.org") {
            continue;
        }

        match std::fs::read_to_string(path) {
            Ok(content) => {
                let nodes = crate::org::parse_org_multi(&content);
                if nodes.is_empty() {
                    report.nodes_skipped += 1;
                } else {
                    for mut node in nodes {
                        node.source_file = Some(path.to_path_buf());
                        report.links_created += node.links().len();
                        if seen_ids.insert(node.id.clone()) {
                            let nid = node.id.clone();
                            kb.insert(node);
                            report.nodes_imported += 1;
                            file_id_map.entry(path.to_path_buf()).or_default().push(nid);
                        } else {
                            report
                                .duplicate_ids
                                .push((node.id.clone(), path.to_path_buf()));
                        }
                    }
                }
            }
            Err(e) => {
                report.errors.push((path.to_path_buf(), e.to_string()));
            }
        }
    }

    report.path_to_ids = file_id_map.into_iter().collect();
    let health = ImportHealth::from_kb(&kb);
    (kb, report, health)
}

/// Import an org-roam directory directly into a CozoDB store.
///
/// Unlike `import_org_dir`, this writes nodes directly to CozoDB (no
/// intermediate in-memory KB). Supports full and incremental modes.
///
/// Returns a report and also populates an in-memory KB for the caller
/// to use as a read cache.
pub fn import_org_dir_to_store(
    org_dir: &Path,
    store: &crate::CozoKbStore,
    mode: &IngestMode,
) -> Result<(KnowledgeBase, ImportReport), KbStoreError> {
    use crate::store::KbStore;
    use sha2::{Digest, Sha256};

    let start = std::time::Instant::now();
    let mut kb = KnowledgeBase::new();
    let mut report = ImportReport {
        mode: format!("{mode:?}"),
        ..Default::default()
    };
    let mut seen_ids = std::collections::HashSet::new();

    let walker = walkdir::WalkDir::new(org_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok());

    // Track which files we visit (for detecting deletions in Full mode).
    let mut visited_files = std::collections::HashSet::new();

    for entry in walker {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("org") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("eor-instance.org") {
            continue;
        }

        let file_path_str = path.to_string_lossy().to_string();
        visited_files.insert(file_path_str.clone());

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                report.errors.push((path.to_path_buf(), e.to_string()));
                continue;
            }
        };

        // Compute content hash for change detection.
        let content_hash = hex::encode(Sha256::digest(content.as_bytes()));

        // In incremental mode, skip files whose content hasn't changed.
        if matches!(mode, IngestMode::Incremental) {
            if let Ok(Some(stored_hash)) = store.get_source_file_hash(&file_path_str) {
                if stored_hash == content_hash {
                    // Content unchanged — load existing node IDs into in-memory KB.
                    if let Ok(node_ids) = store.get_source_file_node_ids(&file_path_str) {
                        for id in &node_ids {
                            if let Ok(Some(node)) = store.get_node(id) {
                                seen_ids.insert(id.clone());
                                kb.insert(node);
                            }
                        }
                    }
                    report.nodes_unchanged += 1;
                    continue;
                }
            }
        }

        // Parse with typed-link support (ADR-030: rel/weight/confidence are
        // encoded inline in each link's `?query`, so no rel-type table is needed).
        let parse_result = parse_org_multi_result(&content);
        if parse_result.nodes.is_empty() {
            report.nodes_skipped += 1;
            continue;
        }

        let mut file_node_ids = Vec::new();
        // The source-file hash is per-file (constant across this file's nodes),
        // so look it up ONCE instead of re-querying per heading. Capturing it
        // before the loop also fixes a miscount: an insert mid-loop could make
        // later nodes of a brand-new file look like updates.
        let file_already_known = matches!(
            store.get_source_file_hash(&file_path_str),
            Ok(Some(ref h)) if !h.is_empty()
        );
        for mut node in parse_result.nodes {
            node.source_file = Some(path.to_path_buf());
            report.links_created += node.links().len();

            if seen_ids.insert(node.id.clone()) {
                // #265: write to CozoDB FIRST and tolerate a single bad node — do NOT
                // `?`-abort the whole import. A failure at node k of N used to leave the
                // persistent store partially populated (no rollback) while the caller
                // silently swapped to an unpersisted in-memory copy; instead record the
                // error and keep importing the rest. Only track the node (dedup id, kb
                // mirror, counts) once it actually persisted.
                let node_id = node.id.clone();
                // Snapshot whatever this ingest is about to overwrite, so a
                // clobber is undoable via `kb_restore`. No-op when the node is
                // new or its content is unchanged, which is the overwhelmingly
                // common case for a re-ingest.
                if let Err(e) =
                    store.insert_node_with_history(&node, "replaced by org-directory ingest")
                {
                    report.errors.push((
                        path.to_path_buf(),
                        format!("node '{node_id}': persist failed: {e}"),
                    ));
                    seen_ids.remove(&node_id);
                    // links were counted optimistically at parse time; undo this node's.
                    report.links_created = report.links_created.saturating_sub(node.links().len());
                    continue;
                }
                file_node_ids.push(node_id);
                kb.insert(node);

                // Update vs new is a per-file property (captured before the loop).
                if file_already_known {
                    report.nodes_updated += 1;
                } else {
                    report.nodes_imported += 1;
                }
            } else {
                report
                    .duplicate_ids
                    .push((node.id.clone(), path.to_path_buf()));
            }
        }

        // Wire typed links to CozoDB.
        for (src_id, link) in &parse_result.typed_links {
            if let Err(e) = store.add_typed_link(src_id, &link.target, &link.rel_type, 1.0) {
                tracing::debug!(src = %src_id, dst = %link.target, rel = %link.rel_type, error = %e, "typed link insert failed");
            }
        }

        // Wire transclusions to meta_members.
        for (order, (meta_id, member_id, role)) in parse_result.transclusions.iter().enumerate() {
            if let Err(e) = store.add_meta_member(meta_id, member_id, order as i32, role) {
                tracing::debug!(meta = %meta_id, member = %member_id, error = %e, "meta_member insert failed");
            }
        }

        // Record source file metadata for incremental reimport.
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        store.record_source_file(&file_path_str, &content_hash, mtime, &file_node_ids)?;

        report.path_to_ids.push((path.to_path_buf(), file_node_ids));
    }

    // In full mode, detect deleted files and remove their nodes.
    if matches!(mode, IngestMode::Full) {
        if let Ok(tracked_files) = store.list_source_files() {
            for (tracked_path, _, _) in tracked_files {
                if !visited_files.contains(&tracked_path) {
                    // File was deleted — remove its nodes.
                    if let Ok(removed_ids) = store.remove_source_file(&tracked_path) {
                        report.nodes_removed += removed_ids.len();
                    }
                }
            }
        }
    }

    report.duration_ms = start.elapsed().as_millis() as u64;
    Ok((kb, report))
}

/// Read UUID from sentinel file in org directory.
fn read_sentinel_uuid(org_dir: &Path) -> Option<String> {
    let sentinel = org_dir.join("eor-instance.org");
    if !sentinel.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&sentinel).ok()?;
    for line in content.lines() {
        if let Some(id) = line.strip_prefix(":ID:") {
            return Some(id.trim().to_string());
        }
    }
    None
}

/// Write sentinel file to org directory (idempotent).
fn write_sentinel(org_dir: &Path, uuid: &str, name: &str) -> io::Result<()> {
    let sentinel = org_dir.join("eor-instance.org");
    if sentinel.exists() {
        return Ok(()); // Don't overwrite
    }
    let content = format!(
        ":PROPERTIES:\n:ID: {}\n:END:\n#+title: {} (MAE KB Instance)\n\nThis file marks this directory as a MAE KB instance.\nIt is safe to delete — MAE will recreate it on next registration.\n",
        uuid, name
    );
    std::fs::write(&sentinel, content)
}

/// Generate a random UUID (RFC 4122 version 4, 122 bits of OS entropy).
///
/// Mints KB instance ids and — since ADR-105 D4 — KB collab ids, which are
/// signed into every membership op and are write-once for the life of the KB. A
/// collision is therefore not a cosmetic problem: two KBs sharing an id share a
/// collection, a membership list, and a node-document namespace.
///
/// **This used to derive the id from a nanosecond clock and 16 bits of pid, with
/// no randomness at all**, so uniqueness came entirely from the clock advancing
/// between calls.
///
/// That is not a theoretical weakness — it **collided in practice, on macOS**, and
/// CI caught it the first time the uniqueness property was asserted:
/// `generate_uuid collided within one process`. macOS's `SystemTime::now()` is
/// coarser than Linux's, so consecutive mints landed in the same tick. Twenty
/// thousand mints had measured clean on Linux, which made the whole risk look
/// cross-machine and remote; it was neither. Per CLAUDE.md principle #13, a
/// property that holds on one developer's platform and not the other is not a
/// property.
///
/// Its trailing 48-bit field was dead as well: `ts >> 64` is zero for
/// nanoseconds-since-epoch until the year 2554, so every id ended in
/// `000000000000` and the real entropy was ~80 bits at best, concentrated in a
/// value two machines can easily agree on.
///
/// `rand::random` is the version-stable top-level API (the idiom already used in
/// `mae-sync`'s `content_crypto`), backed by the OS CSPRNG.
///
/// Probability is only the first of three layers, because "very unlikely" is not
/// a failure mode anyone can debug. See [`KbRegistry::collab_id_for_share`] for
/// the local uniqueness check and the daemon's `kb_share_ownership` for what
/// happens when two machines collide anyway.
pub fn generate_uuid() -> String {
    let mut b = rand::random::<[u8; 16]>();
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let h = |s: &[u8]| s.iter().map(|x| format!("{x:02x}")).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        h(&b[0..4]),
        h(&b[4..6]),
        h(&b[6..8]),
        h(&b[8..10]),
        h(&b[10..16])
    )
}

/// Parse an `eor:` link into (optional_uuid, node_id).
pub fn parse_eor_link(link: &str) -> (Option<&str>, &str) {
    let link = link.strip_prefix("eor:").unwrap_or(link);
    if let Some(slash_pos) = link.find('/') {
        (Some(&link[..slash_pos]), &link[slash_pos + 1..])
    } else {
        (None, link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeKind;

    // ADR-061 Phase A: provider-residency gate, relocated from mae-core so
    // the daemon (which has no Editor and doesn't depend on mae-core) can
    // also consult it for enrichment's embedding-provider check.

    #[test]
    fn is_local_provider_recognizes_ollama_only() {
        assert!(is_local_provider("ollama"));
        assert!(!is_local_provider("claude"));
        assert!(!is_local_provider("openai"));
        assert!(!is_local_provider("gemini"));
        assert!(!is_local_provider("deepseek"));
        assert!(!is_local_provider(""));
    }

    #[test]
    fn residency_permits_provider_open_allows_any_provider() {
        assert!(residency_permits_provider(AiResidency::Open, "claude"));
        assert!(residency_permits_provider(AiResidency::Open, "ollama"));
        assert!(residency_permits_provider(AiResidency::Open, "anything"));
    }

    #[test]
    fn residency_permits_provider_local_models_only_denies_hosted_provider() {
        // This is the exact adversarial case ADR-061 Phase A's own
        // Verification names: "a hosted-provider configuration pointed at
        // a LocalModelsOnly-residency KB must be rejected."
        assert!(!residency_permits_provider(
            AiResidency::LocalModelsOnly,
            "claude"
        ));
        assert!(!residency_permits_provider(
            AiResidency::LocalModelsOnly,
            "openai"
        ));
        assert!(!residency_permits_provider(
            AiResidency::LocalModelsOnly,
            "gemini"
        ));
    }

    #[test]
    fn residency_permits_provider_local_models_only_allows_ollama() {
        assert!(residency_permits_provider(
            AiResidency::LocalModelsOnly,
            "ollama"
        ));
    }

    #[test]
    fn kb_scope_parse_tokens() {
        assert_eq!(KbScope::parse(""), KbScope::All);
        assert_eq!(KbScope::parse("all"), KbScope::All);
        assert_eq!(KbScope::parse("ALL"), KbScope::All);
        assert_eq!(KbScope::parse("local"), KbScope::LocalOnly);
        assert_eq!(KbScope::parse("local-only"), KbScope::LocalOnly);
        assert_eq!(KbScope::parse("remote"), KbScope::RemoteOnly);
        assert_eq!(KbScope::parse("MyNotes"), KbScope::Named("MyNotes".into()));
        // Round-trip through the canonical token.
        assert_eq!(
            KbScope::parse(&KbScope::RemoteOnly.as_token()),
            KbScope::RemoteOnly
        );
        assert_eq!(
            KbScope::parse(&KbScope::Named("Work".into()).as_token()),
            KbScope::Named("Work".into())
        );
    }

    #[test]
    fn kb_instance_is_remote() {
        let mut inst = KbInstance {
            uuid: "u".into(),
            name: "n".into(),
            org_dir: PathBuf::from("/tmp/n"),
            db_path: PathBuf::from("/tmp/n.db"),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: AiResidency::default(),
            project_root: None,
            kind: KbInstanceKind::default(),
            priority: 0,
            remote_hub: None,
        };
        assert!(!inst.is_remote(), "plain local import is not remote");
        inst.shared = true;
        assert!(inst.is_remote(), "shared instance is remote");
        inst.shared = false;
        inst.remote_peers.push("peer1".into());
        assert!(inst.is_remote(), "instance with peers is remote");
    }

    /// ADR-058 Phase A adversarial test: a registry file written by pre-058 `mae` (no
    /// `project_root`/`kind` fields at all, and predating several other already-`#[serde(default)]`
    /// fields too, to genuinely simulate an old file rather than a hand-picked convenient one)
    /// must deserialize unchanged — every pre-058 field preserved, the two new fields defaulting
    /// to `None`/`UserRegistered` — and re-serializing that result must be *stable*: parsing the
    /// re-serialized TOML again produces the identical `KbRegistry`, not a value that drifts on
    /// a second round-trip.
    #[test]
    fn pre_058_registry_toml_round_trips_unchanged_and_stably() {
        let pre_058_toml = r#"
primary_shared = false

[[instances]]
uuid = "abc-123"
name = "Work"
org_dir = "/home/user/notes"
db_path = "/home/user/.local/share/mae/kb/work.cozo"
primary = false
enabled = true
"#;
        let registry: KbRegistry =
            toml::from_str(pre_058_toml).expect("pre-058 registry TOML must still parse");
        assert_eq!(registry.instances.len(), 1);
        let inst = &registry.instances[0];

        // Every pre-058 field is preserved exactly.
        assert_eq!(inst.uuid, "abc-123");
        assert_eq!(inst.name, "Work");
        assert_eq!(inst.org_dir, PathBuf::from("/home/user/notes"));
        assert_eq!(
            inst.db_path,
            PathBuf::from("/home/user/.local/share/mae/kb/work.cozo")
        );
        assert!(!inst.primary);
        assert!(inst.enabled);
        assert_eq!(inst.last_import, None);
        assert_eq!(inst.ai_residency, AiResidency::Open);

        // The two new fields default correctly for a pre-058 entry.
        assert_eq!(inst.project_root, None);
        assert_eq!(inst.kind, KbInstanceKind::UserRegistered);
        assert_eq!(inst.effective_kind(), KbInstanceKind::UserRegistered);

        // Stable re-serialization: round-tripping the *deserialized* value again must
        // produce byte-identical TOML, not merely an equivalent-but-differently-shaped one.
        let reserialized = toml::to_string_pretty(&registry).expect("re-serializing must succeed");
        let reparsed: KbRegistry =
            toml::from_str(&reserialized).expect("re-serialized TOML must itself parse");
        let reparsed_inst = &reparsed.instances[0];
        assert_eq!(reparsed_inst.uuid, inst.uuid);
        assert_eq!(reparsed_inst.project_root, inst.project_root);
        assert_eq!(reparsed_inst.kind, inst.kind);
        assert_eq!(reparsed_inst.ai_residency, inst.ai_residency);

        // A third round-trip must be a fixed point — no further drift.
        let reserialized2 =
            toml::to_string_pretty(&reparsed).expect("second re-serialization must succeed");
        assert_eq!(
            reserialized, reserialized2,
            "re-serialization must be a stable fixed point, not drift on repeated round-trips"
        );
    }

    /// ADR-062 Phase C: `register_remote_hub` produces a well-formed `RemoteHub`
    /// instance, is idempotent on `(base_url, hub_kb_id)` (mirroring `register`'s own
    /// idempotence-on-`org_dir` contract — calling it twice for the same hub must not
    /// create a duplicate registry row), and a different hub at the same base_url (a
    /// multi-tenant hub daemon serving several `kb_id`s) registers as a distinct instance.
    #[test]
    fn register_remote_hub_is_idempotent_per_hub_and_distinguishes_kb_id() {
        let mut reg = KbRegistry::default();
        let uuid1 = reg.register_remote_hub(
            "Team Hub".to_string(),
            "https://kb.example.org:8443".to_string(),
            "team-notes".to_string(),
            RemoteHubAuth::KeystoreKey("team-hub-token".to_string()),
        );
        assert!(!uuid1.is_empty());
        assert_eq!(reg.instances.len(), 1);

        let inst = reg
            .find_by_uuid(&uuid1)
            .expect("registered instance must be findable");
        assert_eq!(inst.kind, KbInstanceKind::RemoteHub);
        assert!(
            inst.is_remote(),
            "a RemoteHub instance must report is_remote() == true"
        );
        assert_eq!(inst.org_dir, PathBuf::new());
        assert_eq!(inst.db_path, PathBuf::new());
        let hub = inst
            .remote_hub
            .as_ref()
            .expect("remote_hub config must be populated");
        assert_eq!(hub.base_url, "https://kb.example.org:8443");
        assert_eq!(hub.hub_kb_id, "team-notes");
        assert_eq!(
            hub.auth,
            RemoteHubAuth::KeystoreKey("team-hub-token".to_string())
        );

        // Same hub registered again: idempotent, no duplicate row.
        let uuid1_again = reg.register_remote_hub(
            "Team Hub (renamed locally)".to_string(),
            "https://kb.example.org:8443".to_string(),
            "team-notes".to_string(),
            RemoteHubAuth::Command("op read op://vault/team-hub-token".to_string()),
        );
        assert_eq!(
            uuid1, uuid1_again,
            "same (base_url, hub_kb_id) must not duplicate"
        );
        assert_eq!(reg.instances.len(), 1);

        // A different kb_id on the SAME hub daemon is a genuinely distinct instance.
        let uuid2 = reg.register_remote_hub(
            "Team Hub — Archive".to_string(),
            "https://kb.example.org:8443".to_string(),
            "team-archive".to_string(),
            RemoteHubAuth::KeystoreKey("team-hub-token".to_string()),
        );
        assert_ne!(uuid1, uuid2);
        assert_eq!(reg.instances.len(), 2);
    }

    /// ADR-062 Phase C: a `RemoteHub` instance (including the new nested
    /// `RemoteHubConfig`/`RemoteHubAuth` enum) must round-trip through TOML exactly like
    /// every other instance, and a pre-062 registry with no `remote_hub` field at all
    /// (predating this ADR) must still deserialize cleanly with `remote_hub: None` —
    /// same backward-compatibility contract `pre_058_registry_toml_round_trips_unchanged_and_stably`
    /// already proves for the ADR-058 fields.
    #[test]
    fn remote_hub_instance_round_trips_through_toml_and_pre_062_files_still_parse() {
        let mut reg = KbRegistry::default();
        reg.register_remote_hub(
            "Team Hub".to_string(),
            "https://kb.example.org:8443".to_string(),
            "team-notes".to_string(),
            RemoteHubAuth::Command("op read op://vault/team-hub-token".to_string()),
        );

        let toml_str = toml::to_string_pretty(&reg).expect("serialize");
        let reparsed: KbRegistry = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(reparsed.instances.len(), 1);
        let inst = &reparsed.instances[0];
        assert_eq!(inst.kind, KbInstanceKind::RemoteHub);
        let hub = inst.remote_hub.as_ref().unwrap();
        assert_eq!(hub.base_url, "https://kb.example.org:8443");
        assert_eq!(hub.hub_kb_id, "team-notes");
        assert_eq!(
            hub.auth,
            RemoteHubAuth::Command("op read op://vault/team-hub-token".to_string())
        );

        // Pre-062 file: no `remote_hub` key at all for a plain non-hub instance.
        let pre_062_toml = r#"
[[instances]]
uuid = "abc-123"
name = "Work"
org_dir = "/home/user/notes"
db_path = "/home/user/.local/share/mae/kb/work.cozo"
primary = false
enabled = true
"#;
        let pre_062: KbRegistry =
            toml::from_str(pre_062_toml).expect("pre-062 registry TOML must still parse");
        assert_eq!(pre_062.instances[0].remote_hub, None);
        assert_eq!(pre_062.instances[0].kind, KbInstanceKind::UserRegistered);
    }

    /// A user (or an AI peer, via the `kb_register` MCP tool) must not be able
    /// to claim a name MAE's own corpora answer to.
    ///
    /// The oracle is the registry's *contents*, not just the returned `Err`: a
    /// refusal that still appended a row would leave exactly the shadowed
    /// duplicate this reservation exists to prevent — `find()` returns the
    /// first match, so a second `DevPractices` row was previously unreachable
    /// but permanently present.
    #[test]
    fn a_reserved_system_kb_name_cannot_be_registered_and_leaves_no_row() {
        let mut reg = KbRegistry::default();
        let tmp = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();

        for reserved in ["DevPractices", "MaePractices", "manual", "ADR"] {
            let err = reg
                .register(
                    reserved.to_string(),
                    tmp.path().to_path_buf(),
                    data.path(),
                    None,
                )
                .expect_err("a system-KB name must be refused");
            assert!(err.contains("reserved"), "{err}");
        }
        assert!(
            reg.instances.is_empty(),
            "a refused registration must not append a row: {:?}",
            reg.instances.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    /// Case-insensitively, because the reservation is worthless if
    /// `devpractices` slips past the check and then resolves to the system
    /// corpus at read time (`system_kb::find` is itself case-insensitive).
    #[test]
    fn reservation_is_case_insensitive() {
        let mut reg = KbRegistry::default();
        let tmp = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        assert!(reg
            .register(
                "devpractices".to_string(),
                tmp.path().to_path_buf(),
                data.path(),
                None
            )
            .is_err());
    }

    /// The half that matters more: reserving names blocks `kb_register`, so
    /// over-reserving locks users out of names MAE has no claim to. A
    /// near-miss ("Practices" is not "MaePractices") must still register.
    #[test]
    fn an_unreserved_name_still_registers_including_near_misses() {
        let data = tempfile::tempdir().unwrap();
        for name in ["Practices", "MyDevPractices", "adr-notes", "FieldJournal"] {
            let mut reg = KbRegistry::default();
            let tmp = tempfile::tempdir().unwrap();
            let uuid = reg
                .register(
                    name.to_string(),
                    tmp.path().to_path_buf(),
                    data.path(),
                    None,
                )
                .unwrap_or_else(|e| panic!("{name} must be registrable: {e}"));
            assert!(!uuid.is_empty());
            assert!(reg.find(name).is_some(), "{name} must be findable");
        }
    }

    #[test]
    fn registry_register_and_find() {
        let mut reg = KbRegistry::default();
        let tmp = std::env::temp_dir().join("mae-test-fed-1");
        let _ = std::fs::create_dir_all(&tmp);
        let data = std::env::temp_dir().join("mae-test-fed-data");
        let _ = std::fs::create_dir_all(&data);

        let uuid = reg
            .register("Test".to_string(), tmp.clone(), &data, None)
            .unwrap();
        assert!(!uuid.is_empty());
        assert!(reg.find("Test").is_some());
        assert!(reg.find(&uuid).is_some());

        // Idempotent
        let uuid2 = reg
            .register("Test2".to_string(), tmp.clone(), &data, None)
            .unwrap();
        assert_eq!(uuid, uuid2);
        assert_eq!(reg.instances.len(), 1);

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn registry_unregister() {
        let mut reg = KbRegistry::default();
        let tmp = std::env::temp_dir().join("mae-test-fed-2");
        let _ = std::fs::create_dir_all(&tmp);
        let data = std::env::temp_dir().join("mae-test-fed-data-2");
        let _ = std::fs::create_dir_all(&data);

        let _ = reg.register("Test".to_string(), tmp.clone(), &data, None);
        assert_eq!(reg.instances.len(), 1);
        reg.unregister("Test");
        assert_eq!(reg.instances.len(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&data);
    }

    /// Regression test for the incident that motivated `KbRegistry::update`:
    /// a KB registration was silently wiped when a second, independently
    /// loaded in-memory registry saved over it. Simulates two long-lived
    /// `mae` processes, each holding its own stale snapshot from before
    /// either one registered anything, both going through `update()`.
    #[test]
    fn concurrent_update_preserves_both_writers_instances() {
        let data = tempfile::TempDir::new().unwrap();
        let org_a = tempfile::TempDir::new().unwrap();
        let org_b = tempfile::TempDir::new().unwrap();

        // Both "processes" load a snapshot before either one writes anything.
        let _stale_a = KbRegistry::load(data.path());
        let _stale_b = KbRegistry::load(data.path());

        // "Process A" registers via the locked-update path.
        let (_, uuid_a, saved_a) = KbRegistry::update(data.path(), |reg| {
            reg.register(
                "A".to_string(),
                org_a.path().to_path_buf(),
                data.path(),
                None,
            )
            .expect("'A' is not a reserved system-KB name")
        });
        saved_a.unwrap();

        // "Process B" registers next — even though ITS OWN in-memory copy
        // (_stale_b) never saw A's write, update() reloads fresh internally,
        // so A's registration must survive.
        let (final_reg, uuid_b, saved_b) = KbRegistry::update(data.path(), |reg| {
            reg.register(
                "B".to_string(),
                org_b.path().to_path_buf(),
                data.path(),
                None,
            )
            .expect("'B' is not a reserved system-KB name")
        });
        saved_b.unwrap();

        assert!(
            final_reg.find(&uuid_a).is_some(),
            "A's registration must survive B's save"
        );
        assert!(final_reg.find(&uuid_b).is_some());

        // Re-load independently from disk to prove it's actually persisted,
        // not just true of the in-memory return value.
        let reloaded = KbRegistry::load(data.path());
        assert_eq!(
            reloaded.instances.len(),
            2,
            "on-disk file must contain BOTH instances, not just B"
        );
    }

    /// ADR-058 Phase B adversarial test: 3 REAL concurrent threads (not the sequential
    /// simulation above) racing to `register()` the SAME `org_dir` via `KbRegistry::update`
    /// must converge to exactly one registry entry, not three duplicates.
    ///
    /// This test caught two real bugs during ADR-058 Phase B's implementation, both fixed
    /// alongside it: (1) `mae_mcp::file_lock::acquire_lock` used a non-atomic
    /// read-then-write check, letting multiple threads believe they'd each acquired the lock
    /// within the same microsecond window — fixed via `OpenOptions::create_new` (atomic
    /// `O_CREAT|O_EXCL`); (2) `KbInstance::effective_kind()` incorrectly treated `primary:
    /// true` (which `register()` sets whenever `self.instances.is_empty()` — an artifact of
    /// *registration order*, not an alias for `KbInstanceKind::Primary`) as overriding the
    /// stored `kind`, silently reclassifying the very first project a user ever provisions.
    /// Both were verified to make this exact test fail before their fixes landed.
    #[test]
    fn kb_registry_register_converges_under_a_three_way_race() {
        let shared_tmp = tempfile::tempdir().unwrap();
        let data_dir = shared_tmp.path().join("data");
        let org_dir = shared_tmp.path().join("project").join(".mae-kb");
        std::fs::create_dir_all(&org_dir).unwrap();
        let canonical_org_dir = org_dir.canonicalize().unwrap();

        let handles: Vec<_> = (0..3)
            .map(|_| {
                let data_dir = data_dir.clone();
                let canonical_org_dir = canonical_org_dir.clone();
                std::thread::spawn(move || {
                    let (_, uuid, saved) = KbRegistry::update(&data_dir, |reg| {
                        reg.register(
                            "project".to_string(),
                            canonical_org_dir.clone(),
                            &data_dir,
                            None,
                        )
                        .expect("'project' is not a reserved system-KB name")
                    });
                    saved.unwrap();
                    uuid
                })
            })
            .collect();

        let uuids: std::collections::HashSet<String> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            uuids.len(),
            1,
            "all 3 concurrent registrations of the same org_dir must converge on the SAME \
             uuid, got {uuids:?}"
        );

        let reloaded = KbRegistry::load(&data_dir);
        let matching: Vec<_> = reloaded
            .instances
            .iter()
            .filter(|i| i.org_dir == canonical_org_dir)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "the on-disk registry must have exactly one entry for this org_dir, not \
             duplicates: {matching:?}"
        );
    }

    #[test]
    fn federated_search_local_first() {
        let mut local = KnowledgeBase::new();
        local.insert(Node::new(
            "test-node",
            "Test Node",
            NodeKind::Note,
            "content",
        ));

        let fed = FederatedKb::new(local);
        let results = fed.search("test");
        assert_eq!(results.len(), 1);
        assert!(results[0].0.is_none()); // from local
    }

    #[test]
    fn federated_search_across_instances() {
        let mut local = KnowledgeBase::new();
        local.insert(Node::new("local-node", "Local", NodeKind::Note, "local"));

        let mut instance = KnowledgeBase::new();
        instance.insert(Node::new("remote-node", "Remote", NodeKind::Note, "remote"));

        let mut fed = FederatedKb::new(local);
        fed.instances.insert("inst-1".to_string(), instance);

        let results = fed.search("node");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn federated_get_local_first() {
        let mut local = KnowledgeBase::new();
        local.insert(Node::new("shared-id", "Local Version", NodeKind::Note, ""));

        let mut instance = KnowledgeBase::new();
        instance.insert(Node::new("shared-id", "Remote Version", NodeKind::Note, ""));

        let mut fed = FederatedKb::new(local);
        fed.instances.insert("inst-1".to_string(), instance);

        let result = fed.get("shared-id").unwrap();
        assert!(result.0.is_none()); // local wins
        assert_eq!(result.1.title, "Local Version");
    }

    #[test]
    fn resolve_eor_link_local_first() {
        let mut local = KnowledgeBase::new();
        local.insert(Node::new("my-node", "Node", NodeKind::Note, ""));

        let fed = FederatedKb::new(local);
        let result = fed.resolve_eor_link("eor:my-node");
        assert!(result.is_some());
        assert!(result.unwrap().0.is_none());
    }

    #[test]
    fn resolve_eor_link_targeted() {
        let local = KnowledgeBase::new();
        let mut instance = KnowledgeBase::new();
        instance.insert(Node::new("target", "Target", NodeKind::Note, ""));

        let mut fed = FederatedKb::new(local);
        fed.instances.insert("uuid-123".to_string(), instance);

        let result = fed.resolve_eor_link("eor:uuid-123/target");
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, Some("uuid-123"));
    }

    #[test]
    fn resolve_eor_link_not_found() {
        let fed = FederatedKb::new(KnowledgeBase::new());
        assert!(fed.resolve_eor_link("eor:nonexistent").is_none());
    }

    #[test]
    fn parse_eor_link_formats() {
        assert_eq!(parse_eor_link("eor:node-id"), (None, "node-id"));
        assert_eq!(
            parse_eor_link("eor:uuid/node-id"),
            (Some("uuid"), "node-id")
        );
        assert_eq!(parse_eor_link("node-id"), (None, "node-id"));
    }

    #[test]
    fn total_nodes_count() {
        let mut local = KnowledgeBase::new();
        local.insert(Node::new("a", "A", NodeKind::Note, ""));
        local.insert(Node::new("b", "B", NodeKind::Note, ""));

        let mut instance = KnowledgeBase::new();
        instance.insert(Node::new("c", "C", NodeKind::Note, ""));

        let mut fed = FederatedKb::new(local);
        fed.instances.insert("i1".to_string(), instance);
        assert_eq!(fed.total_nodes(), 3);
    }

    #[test]
    fn import_org_dir_populates_source_file() {
        let tmp = std::env::temp_dir().join("mae-test-source-file");
        let _ = std::fs::create_dir_all(&tmp);
        std::fs::write(
            tmp.join("note.org"),
            ":PROPERTIES:\n:ID: src-test-1\n:END:\n#+title: Source Test\n\nBody.\n",
        )
        .unwrap();
        let (kb, report, _health) = import_org_dir(&tmp);
        assert!(kb.get("src-test-1").is_some());
        let node = kb.get("src-test-1").unwrap();
        assert!(
            node.source_file.is_some(),
            "source_file should be populated"
        );
        assert!(node.source_file.as_ref().unwrap().ends_with("note.org"));
        // path_to_ids populated
        assert!(!report.path_to_ids.is_empty());
        let ids_for_note: Vec<_> = report
            .path_to_ids
            .iter()
            .filter(|(p, _)| p.ends_with("note.org"))
            .collect();
        assert!(!ids_for_note.is_empty());
        assert!(ids_for_note[0].1.contains(&"src-test-1".to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sentinel_roundtrip() {
        let tmp = std::env::temp_dir().join("mae-test-sentinel");
        let _ = std::fs::create_dir_all(&tmp);
        let _ = std::fs::remove_file(tmp.join("eor-instance.org"));

        assert!(read_sentinel_uuid(&tmp).is_none());
        write_sentinel(&tmp, "test-uuid-123", "MyKB").unwrap();
        assert_eq!(read_sentinel_uuid(&tmp), Some("test-uuid-123".to_string()));

        // Idempotent — doesn't overwrite
        write_sentinel(&tmp, "different-uuid", "Other").unwrap();
        assert_eq!(read_sentinel_uuid(&tmp), Some("test-uuid-123".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ADR-062 Phase A verification. The ADR's own numeric target is derived from
    /// org-roam's documented scaling cliff (org-roam#2474/#1752/#2241 — multi-second to
    /// multi-minute operations at ~3,000 *nodes*). That comparison is about per-node
    /// content operations inside one KB store, not lookups against `KbRegistry` — which
    /// holds metadata for *registered KB instances* (whole separate CozoDB stores a user
    /// explicitly registered: one per project plus a handful of second-brain KBs), a
    /// population that is architecturally bounded several orders of magnitude smaller than
    /// KB-node counts. This test proves that bound empirically rather than asserting it:
    /// even a synthetic 5,000-instance registry (already an unrealistic size — no real
    /// user registers thousands of distinct KB stores) resolves a worst-case `find_by_uuid`
    /// lookup in single-digit microseconds, nowhere near a user-observable latency budget.
    /// A regression that made this scan accidentally quadratic (e.g. a nested lookup added
    /// inside the loop) would blow this bound and fail loudly.
    #[test]
    fn registry_find_by_uuid_stays_well_under_budget_at_thousands_of_instances() {
        fn synth(n: usize) -> KbRegistry {
            let mut reg = KbRegistry::default();
            for i in 0..n {
                reg.instances.push(KbInstance {
                    uuid: format!("uuid-{i:06}"),
                    name: format!("kb-name-{i:06}"),
                    org_dir: PathBuf::from(format!("/tmp/bench-kb-{i}")),
                    db_path: PathBuf::from(format!("/tmp/bench-kb-{i}.db")),
                    primary: false,
                    enabled: true,
                    last_import: None,
                    collab_id: None,
                    shared: false,
                    remote_peers: Vec::new(),
                    last_sync: None,
                    ai_residency: AiResidency::default(),
                    project_root: None,
                    kind: KbInstanceKind::default(),
                    priority: 0,
                    remote_hub: None,
                });
            }
            reg
        }

        for n in [500usize, 2000, 5000] {
            let reg = synth(n);
            // Worst case: the target is the last element, forcing a full scan.
            let target = format!("uuid-{:06}", n - 1);
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                assert!(reg.find_by_uuid(std::hint::black_box(&target)).is_some());
            }
            let per_call = start.elapsed() / 1000;
            assert!(
                per_call.as_micros() < 500,
                "find_by_uuid at n={n} took {per_call:?}/call, expected well under 500us \
                 (org-roam's own cliff is measured in seconds/minutes at comparable node \
                 counts — a registry lookup regressing to even 1ms would still be 1000x \
                 under that bar, so 500us catches a real regression without being flaky)"
            );
        }
    }
}
