//! mae-daemon — background KB persistence, collaboration, and maintenance service.
//!
//! @stability: stable
//!
//! Provides:
//! - CozoDB with SQLite storage backend (no sled SIGABRT on nightly)
//! - JSON-RPC API over Unix socket for editor KB queries
//! - TCP collab server (CRDT sync, WAL-first persistence, PSK auth)
//! - Background file watching, ingestion, and health checks
//! - Optional: AI hygiene suggestions, embedding generation
//!
//! The daemon is optional — the editor works standalone with local sled-backed
//! CozoDB. The daemon is an upgrade that provides persistent SQLite KB,
//! collaboration, and services that outlive the editor session.

mod checkpoint_cli;
mod cli;
mod config;
#[cfg(test)]
mod config_bind_tests;
mod config_guards;
mod conn_limit;
mod dialer;
pub mod enrichment;
mod handler;
pub mod hygiene;
#[cfg(test)]
mod lazy_fetch_client;
pub mod maintenance;
mod oauth;
mod p2p;
mod projection_stores;
mod scheduler;
mod tenant;
#[cfg(test)]
mod tests;
mod ticket;
mod webview;

use config::DaemonConfig;
use handler::DaemonState;
use mae_daemon::{collab_handler, doc_store, kb_doc_migration, storage};
use mae_kb::CozoKbStore;
use mae_mcp::broadcast::{EventBroadcaster, SharedBroadcaster};
use scheduler::DaemonScheduler;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

const VERSION: &str = env!("CARGO_PKG_VERSION");
use mae_daemon::BUILD_SHA;

/// The collab listener's authentication provider, resolved once at startup and
/// cloned (Arc-backed) per connection.
#[derive(Clone)]
enum CollabAuth {
    /// No authentication (trusted loopback).
    None,
    /// Symmetric pre-shared keys (trusted_keys keystore + legacy psk).
    Psk(Arc<mae_mcp::auth::PskAuth>),
    /// Asymmetric Ed25519, plaintext JSON KeyAuth handshake (tls=false fallback).
    Key {
        identity: Arc<mae_mcp::identity::Identity>,
        authorized: Arc<mae_mcp::identity::AuthorizedKeys>,
    },
    /// Asymmetric Ed25519 over native mTLS (default for key mode) — encryption +
    /// mutual auth + pinning unified in the TLS layer (ADR-017).
    KeyTls {
        acceptor: mae_mcp::tls::TlsAcceptor,
        authorized: Arc<mae_mcp::identity::AuthorizedKeys>,
    },
}

#[tokio::main]
async fn main() {
    // @ai-caution: [multi-instance] Global flags are parsed ONCE, up front, and
    // every subcommand below resolves its config from `cli` — never by calling
    // `DaemonConfig::load()` itself. See `cli.rs` for what that regression cost.
    let cli = cli::Cli::parse(std::env::args());

    if cli.version {
        println!("mae-daemon {VERSION} ({BUILD_SHA})");
        return;
    }

    let config = match cli.resolve_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    };

    if cli.check_config {
        run_check_config(&config);
        return;
    }

    match cli.subcommand.as_deref() {
        Some("doctor") => {
            let other = match &cli.compare_with {
                Some(path) => {
                    if !path.exists() {
                        eprintln!(
                            "Error: --compare-with: config not found: {}",
                            path.display()
                        );
                        std::process::exit(2);
                    }
                    Some(DaemonConfig::load_from(path))
                }
                None => None,
            };
            std::process::exit(run_doctor(&config, other.as_ref()));
        }
        // Symmetric keystore (psk mode): `keygen [name]`, `keys`.
        Some("keygen") => {
            std::process::exit(run_keygen(&config, cli.rest.first().map(|s| s.as_str())))
        }
        Some("keys") => std::process::exit(run_keys_list(&config)),
        // Asymmetric key mode (ADR-017/018): `identity`, `authorized`,
        // `authorize <pubkey-line>` (labels must be unique), `revoke <label|SHA256:fp>`.
        Some("identity") => std::process::exit(run_identity(&config)),
        Some("authorized") => std::process::exit(run_authorized_list(&config)),
        Some("authorize") => std::process::exit(run_authorize(&config, &cli.rest)),
        Some("revoke") => {
            std::process::exit(run_revoke(&config, cli.rest.first().map(|s| s.as_str())))
        }
        // ADR-032 checkpoints: the CRDT-truth backup/rollback artifact. Wired to a
        // real surface because until now it had ZERO production callers (#632) —
        // a rollback mechanism that has only ever run in a unit test is an
        // assumption, not a safety net.
        Some("checkpoint") => {
            std::process::exit(checkpoint_cli::run_checkpoint(&config, &cli.rest).await);
        }
        Some("restore") => {
            std::process::exit(checkpoint_cli::run_restore(&config, &cli.rest).await);
        }
        _ => {}
    }

    // Initialize tracing
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.log_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!(version = VERSION, build = BUILD_SHA, "mae-daemon starting");

    // Initialize KB store with SQLite backend
    let data_dir = config.effective_data_dir();
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        tracing::warn!(error = %e, path = %data_dir.display(), "Failed to create data directory");
    }
    let db_path = data_dir.join("daemon-kb.cozo");

    let state = Arc::new(Mutex::new(DaemonState::new()));

    // ADR-060 Phase C: replace the empty (zero-behavior-change) tenant
    // registry `DaemonState::new()` starts with with the real one built from
    // loaded config. Validated at `--check-config` time (`run_check_config`)
    // and here too — a daemon that fails to start on a config error a
    // separate check command would have caught is the wrong asymmetry, so
    // this validates unconditionally rather than only under `--check-config`.
    let tenant_issues = config.check_tenants();
    if !tenant_issues.is_empty() {
        for issue in &tenant_issues {
            tracing::error!(issue = %issue, "tenant configuration error");
        }
        eprintln!("Error: invalid [[tenant]] configuration (see log for details)");
        std::process::exit(1);
    }
    {
        let mut s = state.lock().await;
        s.tenants = Arc::new(tenant::TenantRegistry::from_config(&config.tenant));
    }

    match CozoKbStore::open_with_engine(&db_path, "sqlite") {
        Ok(store) => {
            let store = Arc::new(store);
            let mut s = state.lock().await;
            s.store = Some(Arc::clone(&store));
            s.rebuild_query_layer();
            tracing::info!(path = %db_path.display(), "KB store opened (SQLite)");
        }
        Err(e) => {
            tracing::error!(error = %e, path = %db_path.display(), "Failed to open KB store");
            eprintln!(
                "Error: failed to open KB store at {}: {e}",
                db_path.display()
            );
            eprintln!("The daemon requires CozoDB with SQLite storage.");
            std::process::exit(1);
        }
    }

    // ADR-060 (issue #460): load federated KB instances from kb-registry.toml
    // and open each one's store — before this, NOTHING did this for the
    // daemon binary, so Phase A-D's whole per-tenant instance-addressing
    // mechanism (instance_stores, per-tenant quotas, tenant-boundary role
    // isolation) was unreachable in a real running daemon process:
    // `state.registry` stayed permanently empty and every `instance_addr`-
    // scoped request resolved to `DaemonError::UnknownInstance`, no matter
    // what was configured on disk. Scoped down from the editor's own
    // federation bootstrap (`crates/mae/src/bootstrap.rs::init_kb_federation`):
    // no org-dir-import fallback (the daemon serves CozoDB stores, it
    // doesn't own org-file editing) and no ADR-020 stranded-instance
    // recovery reconstruction (an editor-specific robustness concern for a
    // human-driven session) — just the core "load the registry, open each
    // enabled non-primary instance's CozoDB store" path, which is what the
    // addressing mechanism actually needs to be reachable. A missing/failed
    // instance store is logged and skipped, not fatal — one bad instance
    // must not prevent the primary store (already open above) from serving.
    {
        let registry = mae_kb::federation::KbRegistry::load(&data_dir);
        let mut s = state.lock().await;
        for inst in &registry.instances {
            if inst.primary || !inst.enabled {
                // The primary is already opened above via --data-dir; opening
                // it a second time here would double-open the same file.
                continue;
            }
            if !inst.db_path.exists() {
                tracing::warn!(
                    name = %inst.name, uuid = %inst.uuid, db = %inst.db_path.display(),
                    "federated KB instance has no on-disk CozoDB store, skipping"
                );
                continue;
            }
            match CozoKbStore::open_with_engine(&inst.db_path, "sqlite") {
                Ok(inst_store) => {
                    tracing::info!(name = %inst.name, uuid = %inst.uuid, "federated KB instance opened");
                    s.instance_stores
                        .insert(inst.uuid.clone(), Arc::new(inst_store));
                }
                Err(e) => {
                    tracing::warn!(
                        name = %inst.name, uuid = %inst.uuid, error = %e,
                        "failed to open federated KB instance, skipping"
                    );
                }
            }
        }
        s.registry = registry;
        s.rebuild_query_layer();
    }

    // Clean stale socket
    let socket_path = &config.socket;
    if socket_path.exists() {
        // Check if another daemon is running
        match tokio::net::UnixStream::connect(socket_path).await {
            Ok(_) => {
                eprintln!(
                    "Error: another daemon is already listening on {}",
                    socket_path.display()
                );
                std::process::exit(1);
            }
            Err(_) => {
                // Stale socket — clean it up
                if let Err(e) = std::fs::remove_file(socket_path) {
                    tracing::warn!(error = %e, path = %socket_path.display(), "Failed to remove stale socket");
                }
            }
        }
    }

    // Ensure socket parent directory exists
    if let Some(parent) = socket_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(error = %e, path = %parent.display(), "Failed to create socket directory");
        }
    }

    let listener = match UnixListener::bind(socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "Error: failed to bind socket {}: {e}",
                socket_path.display()
            );
            std::process::exit(1);
        }
    };
    tracing::info!(socket = %socket_path.display(), "KB listener ready");

    // Shutdown channel
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    // Start scheduler
    // #461: previously called the bare `DaemonConfig::load()` here (hardcoded
    // default path) instead of the already-resolved `config` local -- a
    // `--config <path>` override on the CLI was silently ignored by the
    // scheduler's own config-derived behavior (maintenance/enrichment
    // intervals) while everything else in `main()` correctly honored it.
    let scheduler = DaemonScheduler::new(config.clone(), Arc::clone(&state));
    let scheduler_shutdown = shutdown_tx.subscribe();
    let scheduler_handle = tokio::spawn(async move {
        scheduler.run(scheduler_shutdown).await;
    });

    // --- Collab doc_store (absorbed from mae-state-server) ---
    // ADR-053/Phase G (#382): constructed once here (not inside
    // `spawn_collab_server`) so it can also be shared with the OAuth
    // listener's `kb/query.*` surface below — that surface needs read
    // access to the same `DocStore`, independent of whether the TCP
    // listener's own auth setup succeeds.
    let mut doc_store_for_query: Option<Arc<doc_store::DocStore>> = None;
    // ADR-067 Phase D3: the daemon's own key-mode identity, shared with the
    // OAuth listener below for self-issued-token minting/validation. `None`
    // for psk/none collab auth (no Ed25519 identity exists) or when collab
    // is disabled entirely.
    let mut daemon_identity_for_oauth: Option<Arc<mae_mcp::identity::Identity>> = None;
    if config.collab.enabled {
        let collab_issues = config.check_collab();
        if !collab_issues.is_empty() {
            for issue in &collab_issues {
                error!(issue = %issue, "collab configuration error");
            }
            // Non-fatal: KB service continues, collab disabled
            warn!("collab service disabled due to config errors");
        } else if let Some((doc_store, broadcaster, server_start_time)) =
            init_doc_store(&config).await
        {
            doc_store_for_query = Some(Arc::clone(&doc_store));
            // @ai-caution: [daemon-state] These two go into `DaemonState` HERE,
            // outside `spawn_collab_server`'s auth match — not inside its
            // `"key"` arm, where they lived until #647. They are properties of
            // "the collab server is running", not of how it authenticates.
            //
            // Under psk/none they were left `None` while the collab server ran
            // normally, so `daemon/status` reported `kb_collections: []` and
            // `primary_exists: false` for a daemon that genuinely hosted the
            // primary — and an editor reading those decided never to route KB
            // reads through it (`should_attach_daemon_reads`). `kb/node_crdt`
            // returned `NotReady` for the same reason.
            //
            // ADR-053 hit this and routed around it rather than fixing it:
            // `doc_store_for_query` above exists precisely so the OAuth query
            // surface works "independent of whether the TCP listener's own auth
            // setup succeeds". That was the third such workaround. `owner`
            // stays key-mode-only, correctly — it is a signing identity that
            // only exists in key mode.
            {
                let mut st = state.lock().await;
                st.doc_store = Some(Arc::clone(&doc_store));
                st.broadcaster = Some(broadcaster.clone());
            }
            spawn_projector(&config, Arc::clone(&state), Arc::clone(&doc_store)).await;
            daemon_identity_for_oauth = spawn_collab_server(
                &config,
                Arc::clone(&state),
                doc_store,
                broadcaster,
                server_start_time,
            )
            .await;
        } else {
            warn!("collab service disabled");
        }
    } else {
        info!("collab service disabled in config");
    }

    // --- OAuth 2.1 resource-server listener (ADR-052); kb/query.* (ADR-053/Phase G) ---
    if config.oauth.enabled {
        if config.oauth.canonical_resource_uri.is_empty() || config.oauth.jwks_url.is_empty() {
            error!("oauth.enabled is true but canonical_resource_uri/jwks_url are not set — OAuth listener disabled");
        } else {
            let server_config = oauth::ResourceServerConfig {
                canonical_resource_uri: config.oauth.canonical_resource_uri.clone(),
                principal_claim: config.oauth.principal_claim.clone(),
                jwks_url: config.oauth.jwks_url.clone(),
                issuer: config.oauth.issuer.clone(),
                kb_query_enabled: config.oauth.kb_query_enabled,
                max_request_body_bytes: config.oauth.max_request_body_bytes,
                kb_query_max_body_bytes: config.oauth.kb_query_max_body_bytes,
                kb_query_max_scan_nodes: config.oauth.kb_query_max_scan_nodes,
                kb_query_max_search_results: config.oauth.kb_query_max_search_results,
                webview_enabled: config.oauth.webview_enabled,
            };
            let bind = config.oauth.bind;
            let cert_path = config.oauth.cert_path.clone();
            let key_path = config.oauth.key_path.clone();
            let doc_store = doc_store_for_query.clone();
            let oauth_limiter = conn_limit::ConnLimiter::new(config.oauth.max_connections);
            // ADR-060 Phase C (#456): the OAuth listener charges against the same
            // per-tenant budget as the collab listener, keyed on the token's mapped
            // principal.
            let oauth_quota: Arc<dyn mae_daemon::quota::QuotaCharger> =
                Arc::new(tenant::TenantQuota(Arc::clone(&state.lock().await.tenants)));
            // ADR-067 Phase D3: `None` unless BOTH the operator opted in AND
            // this daemon actually has a key-mode identity to validate
            // against (see `daemon_identity_for_oauth`'s own doc comment) —
            // a `kid: "self"` token on any other daemon just falls through
            // to the ordinary JWKS path (`oauth::handle_request`), where it
            // fails as an unknown key like any other bogus `kid`.
            let self_issue = if config.oauth.self_issued_tokens_enabled {
                daemon_identity_for_oauth.clone().map(|identity| {
                    mae_daemon::oauth_self_issue::SelfIssueConfig {
                        identity,
                        audience: config.oauth.canonical_resource_uri.clone(),
                        ttl_secs: config.oauth.self_issued_token_ttl_secs,
                    }
                })
            } else {
                None
            };
            tokio::spawn(async move {
                if let Err(e) = oauth::run_oauth_listener(
                    server_config,
                    bind,
                    &cert_path,
                    &key_path,
                    doc_store,
                    oauth_limiter,
                    self_issue,
                    oauth_quota,
                )
                .await
                {
                    error!(error = %e, "OAuth listener failed to start");
                }
            });
        }
    } else {
        info!("OAuth resource-server listener disabled in config (default)");
    }

    // KB accept loop
    let accept_state = Arc::clone(&state);
    let accept_shutdown = shutdown_tx.subscribe();
    let kb_socket_limiter = conn_limit::ConnLimiter::new(config.kb_socket.max_connections);
    // Share the counter with `daemon/status` (Arc-backed clone, no new state to
    // keep in sync). Deliberately NOT sourced from the broadcaster, which is
    // only installed under key-mode auth — see `handler::connection_report`.
    state.lock().await.kb_conn = Some(kb_socket_limiter.clone());
    let kb_socket_idle_timeout = std::time::Duration::from_secs(config.kb_socket.idle_timeout_secs);
    let accept_handle = tokio::spawn(async move {
        accept_loop(
            listener,
            accept_state,
            accept_shutdown,
            kb_socket_limiter,
            kb_socket_idle_timeout,
        )
        .await;
    });

    // Wait for shutdown signal (Ctrl-C or SIGTERM)
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl-C, shutting down");
        }
        _ = async {
            #[cfg(unix)]
            {
                let mut sigterm = tokio::signal::unix::signal(
                    tokio::signal::unix::SignalKind::terminate(),
                ).expect("failed to register SIGTERM handler");
                sigterm.recv().await;
            }
            #[cfg(not(unix))]
            {
                std::future::pending::<()>().await;
            }
        } => {
            tracing::info!("Received SIGTERM, shutting down");
        }
    }

    // Broadcast shutdown
    let _ = shutdown_tx.send(());

    // Clean up socket (best-effort at shutdown)
    let _ = std::fs::remove_file(socket_path);

    // Wait for tasks
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let _ = scheduler_handle.await;
        let _ = accept_handle.await;
    })
    .await;

    tracing::info!("mae-daemon stopped");
}

/// Spawn the collab TCP server (absorbed from mae-state-server).
/// Launch the P2P mesh endpoint (ADR-025 / #88).
///
/// Reuses the key-mode daemon `identity` as the iroh node identity (so a peer's
/// `EndpointId` is exactly its `authorized_keys` principal) and the same
/// `authorized` set as the TCP listener's access gate, sharing `doc_store` +
/// `broadcaster` so mesh peers and local clients see one document set.
/// Best-effort: a bad relay config or a bind failure disables the mesh but
/// leaves the rest of the daemon running.
#[allow(clippy::too_many_arguments)]
async fn spawn_p2p_mesh(
    p2p: &config::P2pConfig,
    identity: &mae_mcp::identity::Identity,
    authorized_keys_path: std::path::PathBuf,
    doc_store: Arc<doc_store::DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
    state: Arc<Mutex<DaemonState>>,
    kb_query_limits: mae_daemon::kb_query::KbQueryLimits,
    self_issue: Option<mae_daemon::oauth_self_issue::SelfIssueConfig>,
) {
    let relay_mode = match p2p::relay_mode_from_config(&p2p.relay) {
        Ok(mode) => mode,
        Err(e) => {
            error!(error = %e, "P2P mesh disabled: invalid relay configuration");
            return;
        }
    };
    let endpoint = match p2p::bind_endpoint(identity, relay_mode).await {
        Ok(ep) => ep,
        Err(e) => {
            error!(error = %e, "P2P mesh disabled: failed to bind iroh endpoint");
            return;
        }
    };
    info!(
        fingerprint = %identity.fingerprint(),
        relay = %p2p.relay,
        connection_gate = %p2p.connection_gate,
        authorized = mae_mcp::identity::AuthorizedKeys::load(&authorized_keys_path).len(),
        "P2P mesh endpoint bound (ADR-025); accepting peers"
    );
    // Publish a clone to the control-socket state so `p2p/mint_ticket` can build
    // join tickets; the accept loop below owns the original.
    state.lock().await.p2p_endpoint = Some(endpoint.clone());
    // Background dialer (ADR-025/026): drain `p2p/join_ticket` requests and pull the
    // joined KBs (dial by node-id, verify, anchor, fetch). Shares the doc_store with
    // the accept loop so a pulled KB is immediately served onward.
    tokio::spawn(dialer::run_dialer(
        Arc::clone(&state),
        Arc::clone(&doc_store),
        broadcaster.clone(),
        endpoint.clone(),
    ));
    tokio::spawn(p2p::serve(
        endpoint,
        authorized_keys_path,
        p2p.gate_open(),
        doc_store,
        broadcaster,
        start_time,
        conn_limit::ConnLimiter::new(p2p.max_connections),
        Arc::new(handler::DaemonArtifactStore(Arc::clone(&state))),
        Arc::new(tenant::TenantQuota(Arc::clone(&state.lock().await.tenants))),
        kb_query_limits,
        self_issue,
    ));
}

/// Construct + warm up the collab `DocStore`/`broadcaster` (open storage, hydrate
/// blocklists, recover documents) — split out from `spawn_collab_server` (ADR-053/
/// Phase G, #382) so the OAuth listener's `kb/query.*` surface can share the SAME
/// `DocStore` instance without depending on the TCP listener's own setup/auth/accept
/// logic. Returns `None` on any construction failure (already logged) — same
/// disable-collab-non-fatally behavior as before this split, just observable to the
/// caller instead of an internal early `return`.
async fn init_doc_store(
    config: &DaemonConfig,
) -> Option<(
    Arc<doc_store::DocStore>,
    SharedBroadcaster,
    std::time::Instant,
)> {
    let collab = &config.collab;
    let collab_data_dir = config.resolve_collab_data_dir();
    let db_path = collab_data_dir.join("state.db");
    let backend =
        match storage::SqliteBackend::open_with_pool_size(&db_path, collab.storage.shard_count) {
            Ok(b) => Arc::new(b),
            Err(e) => {
                error!(error = %e, path = %db_path.display(), "failed to open collab SQLite");
                return None;
            }
        };

    let doc_store = Arc::new(
        doc_store::DocStore::new(backend.clone(), collab.storage.compact_threshold)
            .with_max_documents(collab.sync.max_documents)
            .with_max_wal_entries(collab.storage.max_wal_entries)
            .with_max_document_size(collab.sync.max_document_size_bytes)
            .with_max_update_size(collab.sync.max_update_size_bytes),
    );
    // ADR-039 A2 (#162): hydrate the local self-protection blocklist from durable storage
    // so a block set in a prior session is enforced from the first op this session derives.
    doc_store.load_blocklists().await;
    let broadcaster: SharedBroadcaster = Arc::new(std::sync::Mutex::new(EventBroadcaster::new()));
    // Shared by the TCP listener and the P2P mesh so both report the same uptime.
    let server_start_time = std::time::Instant::now();

    // Prepare the doc store from storage before anything serves: migrate legacy
    // addresses, then warm every document. Both live in `kb_doc_migration`, which
    // explains why the ORDER matters and why an un-migratable store must not start.
    if !kb_doc_migration::prepare_doc_store(backend.as_ref(), &doc_store).await {
        return None;
    }

    Some((doc_store, broadcaster, server_start_time))
}

/// Wire the CRDT→Cozo projector (ADR-029 B2/B3).
///
/// Until this existed, `Projector` and `DocStore::set_change_feed` had **zero**
/// production callers: `change_tx` was never set, so `emit_change` dropped every
/// event and nothing kept the Cozo projection in step with CRDT writes. A collaborative
/// edit propagated live to connected editors and was simultaneously invisible to
/// `kb/search`, `kb/health`, the webview, and every `kb/query.*` caller — all of which
/// read Cozo.
///
/// Two things are wired here, in this order:
///
/// 1. **`rebuild_kb` per registered KB**, so a daemon that starts with a cold or stale
///    projection heals from CRDT truth immediately rather than serving an empty graph
///    until the first live write happens to arrive.
/// 2. **The change feed**, so subsequent mutations project incrementally.
///
/// A projection failure is logged, never fatal: Cozo is a derived view (ADR-029), so a
/// daemon that cannot project must still serve sync. That is also why this is
/// best-effort per KB — one unreadable collection must not stop the others.
async fn spawn_projector(
    config: &DaemonConfig,
    state: Arc<Mutex<DaemonState>>,
    doc_store: Arc<doc_store::DocStore>,
) {
    if !config.collab.projector_enabled {
        info!("projector disabled in config — cozo projection will not track CRDT writes");
        return;
    }

    let stores = Arc::new(projection_stores::DaemonProjectionStores::new(Arc::clone(
        &state,
    )));
    let projector = Arc::new(mae_daemon::projector::Projector::new(
        Arc::clone(&doc_store),
        stores,
    ));

    // Startup self-heal. Collect the KB ids first so `DaemonState`'s lock is not held
    // across the projection work (ADR-054).
    let kb_ids: Vec<String> = {
        let st = state.lock().await;
        st.registry
            .instances
            .iter()
            .filter(|i| i.enabled)
            // #732: address the collection doc by the KB's MINTED id, not its
            // display name. `rebuild_kb` reads `kbc:{kb_id}`, and every other
            // daemon site (dialer, checkpoint, kb_membership, and
            // `scheduler.rs`'s own `collab_id` lookup) uses the minted id. Using
            // the name meant that for any KB shared after ADR-105 D4 started
            // minting uuids, the startup self-heal read a document that does not
            // exist and silently did nothing -- the failure is downgraded to
            // `debug!` below, so a cold projection just stayed cold until the
            // next live write happened to arrive.
            .map(|i| i.collab_id.clone().unwrap_or_else(|| i.name.clone()))
            .collect()
    };
    for kb_id in kb_ids {
        match projector.rebuild_kb(&kb_id).await {
            Ok(n) => info!(kb = %kb_id, nodes = n, "projector: rebuilt cozo projection from CRDT"),
            Err(e) => {
                // Expected for a KB this daemon holds no `kbc:` doc for (never shared
                // here) — that is not an error, just nothing to project.
                tracing::debug!(kb = %kb_id, error = %e, "projector: no rebuild")
            }
        }
    }

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    doc_store.set_change_feed(tx);
    tokio::spawn(async move {
        // `run` consumes the projector; unwrap the Arc now that setup is done.
        match Arc::try_unwrap(projector) {
            Ok(p) => p.run(rx).await,
            Err(_) => tracing::error!("projector still shared at spawn — change feed not drained"),
        }
    });
    info!("projector wired: cozo projection now tracks CRDT writes");
}

/// ADR-067 Phase D3: returns the daemon's own key-mode identity (`None` for
/// psk/none auth, which have no such identity) so the caller can share it
/// with the OAuth listener's self-issued-token support -- the SAME identity
/// already installed as the doc-store signer and, when P2P is enabled, the
/// iroh node identity, never a second independently-loaded one.
async fn spawn_collab_server(
    config: &DaemonConfig,
    state: Arc<Mutex<DaemonState>>,
    doc_store: Arc<doc_store::DocStore>,
    broadcaster: SharedBroadcaster,
    server_start_time: std::time::Instant,
) -> Option<Arc<mae_mcp::identity::Identity>> {
    let collab = &config.collab;
    let collab_data_dir = config.resolve_collab_data_dir();
    // ADR-067 Phase D2: `Copy`, reused for both the P2P mesh listener below
    // and the TCP collab loop further down -- reuses the SAME `[oauth].
    // kb_query_*` limits the OAuth HTTPS listener already uses (config.rs's
    // `#[serde(default)]` gives sane defaults even when `[oauth]` is absent
    // from daemon.toml entirely, so this needs no new config surface to
    // work out of the box; a deployment wanting different limits per
    // transport would need a real config split, not attempted here).
    let kb_query_limits = mae_daemon::kb_query::KbQueryLimits {
        max_body_bytes: config.oauth.kb_query_max_body_bytes,
        max_scan_nodes: config.oauth.kb_query_max_scan_nodes,
        max_search_results: config.oauth.kb_query_max_search_results,
    };

    // Create the auth provider for this listener.
    //   "psk": trust a SET of symmetric keys (keystore + legacy psk/psk_command).
    //   "key": asymmetric Ed25519 — own identity + authorized_keys (ADR-017).
    //   else:  no auth (trusted loopback).
    let auth_mode = collab.auth.mode.clone();
    // ADR-067 Phase D3: set inside the "key" arm below; stays `None` for
    // psk/none auth, which have no Ed25519 identity to self-issue with.
    let mut identity_for_oauth: Option<Arc<mae_mcp::identity::Identity>> = None;
    // The mint-side counterpart: `Some` only when self-issued tokens are
    // BOTH operator-enabled AND this daemon has a key-mode identity to sign
    // with -- threaded into `collab_handler`'s new `kb/query.self_token` RPC
    // the same way `kb_query_limits` already is (ADR-067 Phase D2).
    let mut self_issue_for_collab: Option<mae_daemon::oauth_self_issue::SelfIssueConfig> = None;
    let collab_auth: CollabAuth = match auth_mode.as_str() {
        "psk" => {
            let mut keys: Vec<(Option<String>, String)> = Vec::new();
            // Legacy: psk_command / psk → one unnamed trusted key.
            if let Some(key) = mae_mcp::auth::load_psk(
                collab.auth.psk_command.as_deref(),
                collab.auth.psk.as_deref(),
            )
            .await
            {
                keys.push((None, key));
            }
            // Keystore: every entry is a trusted peer credential.
            if let Some(path) = collab.auth.keystore_path() {
                match mae_mcp::keystore::load_optional(&path) {
                    Ok(Some(ks)) => {
                        if let Some(w) = ks.permission_warning() {
                            warn!("{w}");
                        }
                        for e in ks.entries {
                            keys.push((e.name, e.secret));
                        }
                        info!(path = %path.display(), keys = keys.len(), "loaded collab keystore");
                    }
                    Ok(None) => debug!(path = %path.display(), "no collab keystore present"),
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "failed to read collab keystore")
                    }
                }
            }
            if keys.is_empty() {
                error!(
                    "collab.auth.mode = 'psk' but no keys available (empty keystore and no psk)"
                );
                warn!("collab service disabled");
                return None;
            }
            info!(
                auth = "psk",
                trusted_keys = keys.len(),
                "collab authentication configured"
            );
            CollabAuth::Psk(Arc::new(mae_mcp::auth::PskAuth::from_keys(keys)))
        }
        "key" => {
            let dir = match collab.auth.identity_dir() {
                Some(d) => d,
                None => {
                    error!("collab.auth.mode = 'key' but no identity dir (set XDG_DATA_HOME/HOME)");
                    warn!("collab service disabled");
                    return None;
                }
            };
            let identity = match mae_mcp::identity::Identity::load_or_generate(&dir, "daemon") {
                Ok(id) => Arc::new(id),
                Err(e) => {
                    error!(error = %e, dir = %dir.display(), "failed to load daemon identity");
                    warn!("collab service disabled");
                    return None;
                }
            };
            identity_for_oauth = Some(Arc::clone(&identity));
            if config.oauth.self_issued_tokens_enabled {
                self_issue_for_collab = Some(mae_daemon::oauth_self_issue::SelfIssueConfig {
                    identity: Arc::clone(&identity),
                    audience: config.oauth.canonical_resource_uri.clone(),
                    ttl_secs: config.oauth.self_issued_token_ttl_secs,
                });
            }
            let ak_path = collab
                .auth
                .authorized_keys_path()
                .unwrap_or_else(|| dir.join("authorized_keys"));
            let authorized = mae_mcp::identity::AuthorizedKeys::load(&ak_path);
            if authorized.is_empty() {
                error!(
                    "collab.auth.mode = 'key' but authorized_keys ({}) is empty — no client can \
                     connect (authorize one with: mae-daemon authorize <pubkey-line>)",
                    ak_path.display()
                );
                warn!("collab service disabled");
                return None;
            }
            let authorized = Arc::new(authorized);

            // ADR-026: install the daemon identity as the membership signer, so the
            // mutation handlers sign + append op-log entries for KBs this daemon
            // owns. Only key mode signs; psk/none keep the legacy unsigned path.
            doc_store.set_signer(Arc::clone(&identity));

            // ADR-018 (#73): install the authorized_keys path so `load_collection`
            // can resolve legacy v1 (label-based) collections to fingerprint-anchored
            // v2 automatically on load, preserving v1 access without a re-share.
            doc_store.set_authorized_keys_path(ak_path.clone());

            // ADR-025 §"Driving surfaces": expose the owner identity to the local
            // control socket, so `p2p/share_kb` can ESTABLISH a mesh share
            // (create/widen the collection doc to P2p) without a collab session
            // — the CLI/editor self-sufficient `kb-share-p2p` path. Key mode
            // only, and genuinely so: a P2P share needs the owner-SIGNING
            // identity, and `config.rs` already rejects `p2p.enabled` unless
            // `auth.mode == "key"`, so P2P cannot work in psk/none regardless.
            {
                // doc_store + broadcaster are installed by the caller, for every
                // auth mode (#647). Only the owner identity is key-mode-specific.
                let mut st = state.lock().await;
                st.owner = Some(Arc::clone(&identity));
            }

            // P2P mesh (ADR-025 / #88): reuse this key-mode identity as the iroh
            // node identity and gate inbound peers on the same authorized_keys
            // set, sharing the doc_store + broadcaster with the TCP listener.
            if collab.p2p.enabled {
                // Pass the authorized_keys PATH (not a snapshot): the mesh gate
                // re-reads it per accept so authorize/revoke/approve take effect
                // live (I-10).
                spawn_p2p_mesh(
                    &collab.p2p,
                    &identity,
                    ak_path.clone(),
                    Arc::clone(&doc_store),
                    broadcaster.clone(),
                    server_start_time,
                    Arc::clone(&state),
                    kb_query_limits,
                    self_issue_for_collab.clone(),
                )
                .await;
            }

            if collab.auth.tls {
                // I-10: the verifier reloads `authorized_keys` per handshake
                // (mtime-gated), so `mae-daemon authorize`/`revoke` take effect
                // on the running daemon without a restart. The `authorized`
                // snapshot below is kept only for the startup log + handler
                // principal/label resolution.
                match mae_mcp::tls::server_config_reloading(&identity, &ak_path) {
                    Ok(cfg) => {
                        info!(
                            auth = "key",
                            tls = true,
                            fingerprint = %identity.fingerprint(),
                            authorized = authorized.len(),
                            "collab authentication configured (mTLS)"
                        );
                        CollabAuth::KeyTls {
                            acceptor: mae_mcp::tls::TlsAcceptor::from(cfg),
                            authorized,
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "failed to build TLS server config");
                        warn!("collab service disabled");
                        return None;
                    }
                }
            } else {
                info!(
                    auth = "key",
                    tls = false,
                    fingerprint = %identity.fingerprint(),
                    authorized = authorized.len(),
                    "collab authentication configured (plaintext KeyAuth)"
                );
                CollabAuth::Key {
                    identity,
                    authorized,
                }
            }
        }
        other => {
            info!(auth = %other, "collab authentication configured");
            CollabAuth::None
        }
    };

    // Bind TCP
    let tcp_listener = match tokio::net::TcpListener::bind(&collab.bind).await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            error!(addr = %collab.bind, "collab address already in use");
            warn!("collab service disabled");
            return None;
        }
        Err(e) => {
            error!(error = %e, addr = %collab.bind, "failed to bind collab TCP");
            warn!("collab service disabled");
            return None;
        }
    };

    info!(
        bind = %collab.bind,
        data_dir = %collab_data_dir.display(),
        "collab server started"
    );

    // Spawn background compaction + eviction task
    {
        let compact_interval = collab.sync.compaction_interval_secs;
        let eviction_secs = collab.sync.idle_eviction_secs;
        let store = Arc::clone(&doc_store);
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(compact_interval.max(10)));
            interval.tick().await; // skip first immediate tick
            loop {
                interval.tick().await;

                let names = store.document_names().await;
                for name in &names {
                    if let Err(e) = store.compact_doc(name).await {
                        warn!(doc = %name, error = %e, "background compaction failed");
                    }
                }
                if !names.is_empty() {
                    debug!(count = names.len(), "background compaction complete");
                }

                if eviction_secs > 0 {
                    let evicted = store.evict_idle(eviction_secs).await;
                    if !evicted.is_empty() {
                        debug!(count = evicted.len(), "idle eviction complete");
                    }
                }
            }
        });
    }

    // Spawn TCP accept loop
    // #342: bound the count of concurrently accepted sockets (authenticated or
    // not) — before this, nothing capped how many stalled/never-authenticating
    // connections could accumulate, each parking a task+socket forever.
    // ADR-054: the counter/guard itself now lives in conn_limit::ConnLimiter,
    // shared with the KB Unix-socket and P2P listeners — behavior-identical
    // to the inline version this replaces (same Relaxed ordering, same
    // 0-means-unlimited semantics, same panic-safe RAII decrement).
    let max_connections = collab.max_connections;
    let limiter = conn_limit::ConnLimiter::new(max_connections);
    state.lock().await.collab_conn = Some(limiter.clone());
    // ADR-061 Phase D3: one shared handle, cloned per connection below (same
    // pattern as `doc_store`/`broadcaster`) -- bridges `kb/fetch_artifact` to
    // this daemon's local KB content store.
    let artifact_store: Arc<dyn mae_daemon::artifact_store::ArtifactStore> =
        Arc::new(handler::DaemonArtifactStore(Arc::clone(&state)));
    // ADR-060 Phase C (#456): the same shared-handle-cloned-per-connection pattern,
    // bridging the listeners' quota seam to this daemon's `TenantRegistry`. Snapshot
    // the registry `Arc` here rather than locking `DaemonState` per request —
    // `state.tenants` is assigned once at startup and never replaced.
    let quota_charger: Arc<dyn mae_daemon::quota::QuotaCharger> = {
        let tenants = Arc::clone(&state.lock().await.tenants);
        Arc::new(tenant::TenantQuota(tenants))
    };
    tokio::spawn(async move {
        loop {
            match tcp_listener.accept().await {
                Ok((stream, addr)) => {
                    let Some(guard) = limiter.try_acquire() else {
                        warn!(
                            %addr,
                            max_connections,
                            "collab TCP: connection cap reached, rejecting new connection"
                        );
                        drop(stream); // closes the socket immediately
                        continue;
                    };
                    info!(addr = %addr, "collab TCP client connected");
                    let store = Arc::clone(&doc_store);
                    let bc = Arc::clone(&broadcaster);
                    let auth = collab_auth.clone();
                    let artifacts = Arc::clone(&artifact_store);
                    let quota = Arc::clone(&quota_charger);
                    let self_issue = self_issue_for_collab.clone();
                    tokio::spawn(async move {
                        let _guard = guard;
                        // mTLS path needs the whole stream (cannot pre-split).
                        if let CollabAuth::KeyTls {
                            acceptor,
                            authorized,
                        } = auth
                        {
                            // #342: same deadline as the plaintext auth paths'
                            // handshake (collab_handler::HANDSHAKE_TIMEOUT_SECS) —
                            // an accepted-but-silent TLS handshake would otherwise
                            // park a task+socket forever.
                            let tls_accept = tokio::time::timeout(
                                std::time::Duration::from_secs(
                                    collab_handler::HANDSHAKE_TIMEOUT_SECS,
                                ),
                                acceptor.accept(stream),
                            )
                            .await;
                            match tls_accept {
                                Err(_elapsed) => {
                                    warn!(
                                        %addr,
                                        timeout_secs = collab_handler::HANDSHAKE_TIMEOUT_SECS,
                                        "TLS handshake timed out, dropping connection"
                                    );
                                    return;
                                }
                                Ok(Err(e)) => {
                                    warn!(%addr, error = %e, "TLS handshake failed");
                                    return;
                                }
                                Ok(Ok(tls)) => {
                                    let peer = {
                                        let (_, conn) = tls.get_ref();
                                        // I-10: re-read authorized_keys fresh so the resolved
                                        // LABEL reflects post-startup authorize/revoke (the cert
                                        // verifier is already live); the startup `authorized`
                                        // snapshot would show a stale/fingerprint-only label.
                                        let live = mae_mcp::identity::AuthorizedKeys::load(
                                            authorized.path(),
                                        );
                                        conn.peer_certificates().and_then(|c| {
                                            mae_mcp::tls::peer_identity_from_tls(c, &live)
                                        })
                                    };
                                    let Some(peer) = peer else {
                                        warn!(%addr, "TLS peer cert not resolvable to an identity");
                                        return;
                                    };
                                    info!(%addr, peer = %peer.label, "mTLS client authenticated");
                                    let (r, w) = tokio::io::split(tls);
                                    collab_handler::handle_client_authenticated(
                                        BufReader::new(r),
                                        w,
                                        peer,
                                        store,
                                        bc,
                                        server_start_time,
                                        mae_sync::kb::Transport::Hub,
                                        artifacts,
                                        quota,
                                        kb_query_limits,
                                        self_issue,
                                    )
                                    .await;
                                }
                            }
                            return;
                        }

                        // Plaintext paths (psk / legacy key / none): split the TCP stream.
                        let (reader, writer) = stream.into_split();
                        let reader = BufReader::new(reader);
                        match auth {
                            CollabAuth::Psk(a) => {
                                collab_handler::handle_client_with_auth(
                                    reader,
                                    writer,
                                    a.as_ref(),
                                    store,
                                    bc,
                                    server_start_time,
                                    mae_sync::kb::Transport::Hub,
                                    artifacts,
                                    quota,
                                    kb_query_limits,
                                    self_issue,
                                )
                                .await;
                            }
                            CollabAuth::Key {
                                identity,
                                authorized,
                            } => {
                                let ka = mae_mcp::auth::KeyAuth::server(identity, authorized);
                                collab_handler::handle_client_with_auth(
                                    reader,
                                    writer,
                                    &ka,
                                    store,
                                    bc,
                                    server_start_time,
                                    mae_sync::kb::Transport::Hub,
                                    artifacts,
                                    quota,
                                    kb_query_limits,
                                    self_issue,
                                )
                                .await;
                            }
                            CollabAuth::None => {
                                collab_handler::handle_client(
                                    reader,
                                    writer,
                                    store,
                                    bc,
                                    server_start_time,
                                    mae_sync::kb::Transport::Hub,
                                    artifacts,
                                    quota,
                                    kb_query_limits,
                                    self_issue,
                                )
                                .await;
                            }
                            CollabAuth::KeyTls { .. } => unreachable!("handled above"),
                        }
                    });
                }
                Err(e) => error!(error = %e, "collab TCP accept error"),
            }
        }
    });

    identity_for_oauth
}

fn run_check_config(config: &DaemonConfig) {
    // Every resource this instance claims, in one block, so an operator running
    // two instances can diff the two reports and see any collision — including
    // identity/authorized_keys/keystore, which default to a SHARED location
    // regardless of `data_dir` (see `DaemonConfig::instance_paths`).
    println!("Instance resources (must be unique per instance):");
    for (label, value) in config.instance_paths().labelled() {
        println!("  {label:<16} {value}");
    }
    println!("Log level: {}", config.log_level);

    // Collab config
    println!("Collab enabled: {}", config.collab.enabled);
    if config.collab.enabled {
        // bind / collab data_dir / keystore / authorized_keys / identity are
        // reported once above, in the instance-resources block.
        println!("  storage.backend: {}", config.collab.storage.backend);
        println!(
            "  storage.compact_threshold: {}",
            config.collab.storage.compact_threshold
        );
        println!(
            "  sync.heartbeat_interval_secs: {}",
            config.collab.sync.heartbeat_interval_secs
        );
        println!("  sync.max_documents: {}", config.collab.sync.max_documents);
        println!("  auth.mode: {}", config.collab.auth.mode);
        if config.collab.auth.mode == "none" {
            // The default is "none", and DAEMON_ADMIN recommends "key" — so the
            // config that says nothing about auth is the UNAUTHENTICATED one.
            // Say so here rather than letting an operator infer it from silence.
            println!("    ! anyone who can reach this port can sync; set auth.mode = \"key\"");
        }
        if config.collab.auth.mode == "psk" {
            println!("    keys: {}", config.collab.auth.keystore_key_count());
        }
        if config.collab.auth.mode == "key" {
            println!(
                "  auth.tls: {} ({})",
                config.collab.auth.tls,
                if config.collab.auth.tls {
                    "mTLS — encrypted"
                } else {
                    "plaintext JSON KeyAuth"
                }
            );
            if let Some(dir) = config.collab.auth.identity_dir() {
                match mae_mcp::identity::Identity::load_or_generate(&dir, "daemon") {
                    Ok(id) => println!("  auth.identity: {}", id.fingerprint()),
                    Err(e) => println!("  auth.identity: <error: {e}>"),
                }
            }
            println!(
                "  auth.authorized_keys: {} key(s)",
                config.collab.auth.authorized_key_count()
            );
        }

        // [collab.p2p] — the iroh mesh transport (ADR-025).
        println!("  p2p.enabled: {}", config.collab.p2p.enabled);
        if config.collab.p2p.enabled {
            println!("    p2p.relay: {}", config.collab.p2p.relay);
            println!(
                "    p2p.connection_gate: {}",
                config.collab.p2p.connection_gate
            );
        }

        let issues = config.check_collab();
        if !issues.is_empty() {
            eprintln!("Collab configuration issues:");
            for issue in &issues {
                eprintln!("  - {issue}");
            }
            std::process::exit(1);
        }
    }

    // ADR-060 Phase C: [[tenant]] validation, independent of collab.enabled.
    println!("  tenants: {}", config.tenant.len());
    let tenant_issues = config.check_tenants();
    if !tenant_issues.is_empty() {
        eprintln!("Tenant configuration issues:");
        for issue in &tenant_issues {
            eprintln!("  - {issue}");
        }
        std::process::exit(1);
    }

    println!("Config OK");
}

/// `mae-daemon keygen [name]` — generate a random key, append it to the
/// keystore (creating it 0600), and print it so it can be copied to peers.
fn run_keygen(config: &DaemonConfig, name: Option<&str>) -> i32 {
    let path = match config.collab.auth.keystore_path() {
        Some(p) => p,
        None => {
            eprintln!("error: cannot resolve keystore path (set XDG_DATA_HOME or HOME)");
            return 1;
        }
    };
    let secret = mae_mcp::keystore::generate_secret();
    match mae_mcp::keystore::add_key(&path, name, &secret) {
        Ok(count) => {
            let label = name
                .map(|n| format!("'{n}'"))
                .unwrap_or_else(|| "unnamed".into());
            println!("Added {label} key to {}", path.display());
            println!("Keystore now holds {count} key(s).");
            println!();
            println!("Trusted-keys line (this host already trusts it):");
            match name {
                Some(n) => println!("  {n} {secret}"),
                None => println!("  {secret}"),
            }
            println!();
            println!("To let a peer connect, copy the EXACT line above into its keystore");
            println!("(same path: {}).", path.display());
            println!("The secret is symmetric — both sides must hold the identical line.");
            0
        }
        Err(e) => {
            eprintln!("error: failed to add key to {}: {e}", path.display());
            1
        }
    }
}

/// `mae-daemon keys` — list the names (and fingerprints) of trusted keys.
fn run_keys_list(config: &DaemonConfig) -> i32 {
    let path = match config.collab.auth.keystore_path() {
        Some(p) => p,
        None => {
            eprintln!("error: cannot resolve keystore path");
            return 1;
        }
    };
    match mae_mcp::keystore::load_optional(&path) {
        Ok(Some(ks)) => {
            if let Some(w) = ks.permission_warning() {
                eprintln!("warning: {w}");
            }
            println!("Trusted keys in {} ({}):", path.display(), ks.len());
            for e in &ks.entries {
                // Show a short fingerprint, never the secret itself.
                let fp: String = e.secret.chars().take(8).collect();
                println!("  {:<16} {}…", e.name.as_deref().unwrap_or("(unnamed)"), fp);
            }
            0
        }
        Ok(None) => {
            println!("No keystore at {} (run: mae-daemon keygen)", path.display());
            0
        }
        Err(e) => {
            eprintln!("error: failed to read keystore {}: {e}", path.display());
            1
        }
    }
}

/// `mae-daemon identity` — print this daemon's Ed25519 public key + fingerprint
/// (generating the keypair if absent). Share the fingerprint out-of-band so
/// clients can verify the TOFU prompt.
fn run_identity(config: &DaemonConfig) -> i32 {
    let dir = match config.collab.auth.identity_dir() {
        Some(d) => d,
        None => {
            eprintln!("error: cannot resolve identity dir (set XDG_DATA_HOME or HOME)");
            return 1;
        }
    };
    match mae_mcp::identity::Identity::load_or_generate_reporting(&dir, "daemon") {
        Ok((id, newly_generated)) => {
            println!("Daemon identity ({}):", dir.join("id_ed25519").display());
            println!("  fingerprint: {}", id.fingerprint());
            println!("  public key:  {}", id.public().to_line());
            if newly_generated {
                // KL1 (SECURITY_REVIEW §6.1): make first creation loud + actionable for the
                // operator — losing this key loses access to every shared KB (no recovery).
                eprintln!();
                eprintln!("  ⚠ {}", mae_mcp::identity::IDENTITY_BACKUP_ADVISORY);
            }
            0
        }
        Err(e) => {
            eprintln!("error: failed to load/generate identity: {e}");
            1
        }
    }
}

/// `mae-daemon authorized` — list trusted client public keys.
fn run_authorized_list(config: &DaemonConfig) -> i32 {
    let path = match config.collab.auth.authorized_keys_path() {
        Some(p) => p,
        None => {
            eprintln!("error: cannot resolve authorized_keys path");
            return 1;
        }
    };
    let ak = mae_mcp::identity::AuthorizedKeys::load(&path);
    println!(
        "Authorized client keys in {} ({}):",
        path.display(),
        ak.len()
    );
    for pk in ak.entries() {
        println!(
            "  {:<16} {}",
            pk.label.as_deref().unwrap_or("(unlabeled)"),
            pk.fingerprint()
        );
    }
    0
}

/// `mae-daemon authorize <pubkey-line>` — add a client public key line
/// (`mae-ed25519 <b64> <label>`) to authorized_keys.
fn run_authorize(config: &DaemonConfig, rest: &[String]) -> i32 {
    if rest.is_empty() {
        eprintln!("usage: mae-daemon authorize <mae-ed25519 <b64> [label]>");
        eprintln!("   or: mae-daemon authorize --from-ssh-pub <path/to/id_ed25519.pub> [label]");
        return 2;
    }
    // --from-ssh-pub <file> [label]: import an OpenSSH Ed25519 PUBLIC key (only
    // the public half — never a private key) as a trusted peer.
    let pk = if rest[0] == "--from-ssh-pub" {
        let file = match rest.get(1) {
            Some(f) => f,
            None => {
                eprintln!("usage: mae-daemon authorize --from-ssh-pub <file> [label]");
                return 2;
            }
        };
        let line = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {file}: {e}");
                return 1;
            }
        };
        let label = rest.get(2).cloned();
        match mae_mcp::identity::PublicKey::from_ssh_line(line.trim(), label) {
            Some(pk) => pk,
            None => {
                eprintln!("error: {file} is not an ssh-ed25519 public key");
                return 1;
            }
        }
    } else {
        let line = rest.join(" ");
        match mae_mcp::identity::PublicKey::from_line(&line) {
            Some(pk) => pk,
            None => {
                eprintln!("error: not a valid key line (expected 'mae-ed25519 <b64> [label]')");
                return 1;
            }
        }
    };
    let path = match config.collab.auth.authorized_keys_path() {
        Some(p) => p,
        None => {
            eprintln!("error: cannot resolve authorized_keys path");
            return 1;
        }
    };
    let fp = pk.fingerprint();
    let label = pk.label.clone().unwrap_or_default();
    let (_ak, add_result) = mae_mcp::identity::AuthorizedKeys::update(&path, |ak| ak.add(pk));
    match add_result {
        Ok(()) => {
            println!("Authorized {label} ({fp}) → {}", path.display());
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Distinguish a re-authorize of the same key (benign) from a label
            // collision with a DIFFERENT key (rejected — labels must be unique).
            let msg = e.to_string();
            if msg.contains("label") {
                eprintln!("error: {msg}");
                eprintln!("  pick a unique label, or `mae-daemon revoke <label>` first.");
                1
            } else {
                println!("Already authorized: {fp}");
                0
            }
        }
        Err(e) => {
            eprintln!("error: failed to authorize: {e}");
            1
        }
    }
}

/// `mae-daemon revoke <label>` — remove authorized client key(s) by label.
fn run_revoke(config: &DaemonConfig, target: Option<&str>) -> i32 {
    let target = match target {
        Some(l) => l,
        None => {
            eprintln!("usage: mae-daemon revoke <label|SHA256:fingerprint>");
            return 2;
        }
    };
    let path = match config.collab.auth.authorized_keys_path() {
        Some(p) => p,
        None => {
            eprintln!("error: cannot resolve authorized_keys path");
            return 1;
        }
    };
    // Revoke by fingerprint (the precise, unambiguous identity — ADR-018) or by a
    // now-unique label.
    let by_fp = target.starts_with("SHA256:");
    let (_ak, result) = mae_mcp::identity::AuthorizedKeys::update(&path, |ak| {
        if by_fp {
            ak.revoke_by_fingerprint(target)
        } else {
            ak.revoke(target)
        }
    });
    match result {
        Ok(0) => {
            println!("No authorized key matching '{target}'");
            0
        }
        Ok(n) => {
            println!("Revoked {n} key(s) matching '{target}'");
            0
        }
        Err(e) => {
            eprintln!("error: failed to revoke: {e}");
            1
        }
    }
}

/// Returns a process exit code: non-zero iff `--compare-with` found a resource
/// the two instances share. Everything else `doctor` reports is advisory — this
/// one is a hard "these two cannot both run", so it must be scriptable in a
/// deploy gate rather than something an operator has to eyeball.
fn run_doctor(config: &DaemonConfig, compare_with: Option<&DaemonConfig>) -> i32 {
    println!("mae-daemon doctor");
    println!("  version: {VERSION} ({BUILD_SHA})");

    // Which instance is this? Print it first — every line below describes THIS
    // instance's resources, and on a host running staging + production the only
    // way to know which one you just diagnosed is to see the paths.
    println!("  instance resources:");
    for (label, value) in config.instance_paths().labelled() {
        println!("    {label:<16} {value}");
    }

    let data_dir = config.effective_data_dir();
    if !data_dir.exists() {
        println!("    (kb data_dir does not exist yet — it will be created)");
    }

    // Check collab
    if config.collab.enabled {
        if config.collab.auth.mode == "psk" {
            if let Some(p) = config.collab.auth.keystore_path() {
                let n = config.collab.auth.keystore_key_count();
                println!("  collab keystore: {} ({n} key(s))", p.display());
                if let Some(w) = mae_mcp::keystore::permission_warning(&p) {
                    println!("    ! {w}");
                }
            }
        }
        let issues = config.check_collab();
        if issues.is_empty() {
            println!("  collab config: OK");
        } else {
            println!("  collab config: {} issue(s)", issues.len());
            for issue in &issues {
                println!("    - {issue}");
            }
        }

        // Check collab storage
        let collab_data_dir = config.resolve_collab_data_dir();
        let db_path = collab_data_dir.join("state.db");
        match storage::SqliteBackend::open_with_pool_size(
            &db_path,
            config.collab.storage.shard_count,
        ) {
            Ok(_) => println!(
                "  collab sqlite: OK ({}, {} shard(s))",
                db_path.display(),
                config.collab.storage.shard_count
            ),
            Err(e) => println!("  collab sqlite: FAILED ({e})"),
        }

        if config.collab.auth.mode == "none" {
            println!("  collab auth: none — the port is UNAUTHENTICATED");
            println!("    ! set [collab.auth] mode = \"key\" before exposing it off-host");
        } else {
            println!("  collab auth: {}", config.collab.auth.mode);
        }

        // Check port
        match std::net::TcpListener::bind(config.collab.bind) {
            Ok(_) => println!("  collab port {}: available", config.collab.bind.port()),
            Err(e) => println!(
                "  collab port {}: {} ({})",
                config.collab.bind.port(),
                e,
                config.collab.bind
            ),
        }
    } else {
        println!("  collab: disabled");
    }

    // The OAuth listener claims a second port; on a two-instance host it
    // collides just as readily as the collab one, so check it the same way.
    if config.oauth.enabled {
        match std::net::TcpListener::bind(config.oauth.bind) {
            Ok(_) => println!("  oauth port {}: available", config.oauth.bind.port()),
            Err(e) => println!("  oauth port {}: {e}", config.oauth.bind.port()),
        }
    } else {
        println!("  oauth: disabled");
    }

    println!("  yrs version: 0.22");

    let Some(other) = compare_with else {
        return 0;
    };
    let conflicts = config
        .instance_paths()
        .conflicts_with(&other.instance_paths());
    if conflicts.is_empty() {
        println!("  side-by-side: OK — shares no resource with the compared instance");
        return 0;
    }
    println!("  side-by-side: {} SHARED resource(s):", conflicts.len());
    for c in &conflicts {
        println!("    ! {c}");
    }
    println!(
        "    These two instances cannot both run as configured. Note that \
         identity_dir/authorized_keys/keystore do NOT follow data_dir — set them \
         explicitly per instance (see docs/DAEMON_ADMIN.md §1)."
    );
    1
}

/// Accept loop: spawn a task per KB client connection.
///
/// `max_connections` is enforced via `limiter` (ADR-054) — this socket is
/// local/filesystem-permissions-only trust (SECURITY.md), so there is no
/// per-principal/per-IP identity to sub-limit against, only a total cap,
/// same shape as the collab TCP listener's own `#342` fix.
async fn accept_loop(
    listener: UnixListener,
    state: Arc<Mutex<DaemonState>>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    limiter: conn_limit::ConnLimiter,
    idle_timeout: std::time::Duration,
) {
    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let Some(guard) = limiter.try_acquire() else {
                            // `current()` is the ACTIVE count, not the cap — the
                            // field was named `max_connections`, so this log line
                            // reported the wrong quantity under the right label.
                            tracing::warn!(
                                active = limiter.current(),
                                max_connections = limiter.max(),
                                "KB socket: connection cap reached, rejecting new connection"
                            );
                            drop(stream);
                            continue;
                        };
                        let client_state = Arc::clone(&state);
                        tokio::spawn(async move {
                            let _guard = guard;
                            if let Err(e) = handle_client(stream, client_state, idle_timeout).await {
                                tracing::debug!(error = %e, "Client disconnected");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Accept failed");
                    }
                }
            }
        }
    }
}

/// Handle a single KB client connection using Content-Length framed JSON-RPC.
///
/// `idle_timeout` (0 = disabled) bounds how long the server waits for the
/// *next* request on an already-open connection — `DaemonClient` keeps one
/// persistent connection open for a whole editor session and transparently
/// reconnects on I/O error, so a server-side idle-close here is self-healing
/// from the client's perspective, not a hard failure (verified by
/// `kb_socket_connection_limit_tests.rs`).
async fn handle_client(
    stream: tokio::net::UnixStream,
    state: Arc<Mutex<DaemonState>>,
    idle_timeout: std::time::Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    loop {
        let read_fut = mae_mcp::read_message(&mut reader);
        let msg = if idle_timeout.is_zero() {
            read_fut.await?
        } else {
            match tokio::time::timeout(idle_timeout, read_fut).await {
                Ok(result) => result?,
                Err(_) => {
                    tracing::debug!(
                        idle_timeout_secs = idle_timeout.as_secs(),
                        "KB socket: closing idle connection"
                    );
                    return Ok(());
                }
            }
        };
        let msg = match msg {
            Some(msg) => msg,
            None => return Ok(()), // Client disconnected
        };

        let request: Value = serde_json::from_str(&msg)?;
        let id = request.get("id").cloned();
        let method = request["method"].as_str().unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        // Handle shutdown request
        if method == "daemon/shutdown" {
            let response = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"shutting_down": true}
            });
            let body = serde_json::to_vec(&response)?;
            mae_mcp::write_framed(&mut writer, &body, std::time::Duration::from_secs(5)).await?;
            return Ok(());
        }

        let response = match handler::dispatch(method, params, &state).await {
            Ok(result) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }),
            Err(e) => {
                // `DaemonError::Internal` wraps raw internal error text (I/O errors,
                // storage-backend errors, etc.) that may reveal paths or internal
                // state — log it server-side and send clients a generic message.
                // Other variants (InvalidParams, NotReady, MethodNotFound) are
                // already client-safe, actionable messages by construction.
                let client_message = if let handler::DaemonError::Internal(_) = &e {
                    error!(error = %e, method, "internal error handling daemon request");
                    "internal error".to_string()
                } else {
                    e.to_string()
                };
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": e.code(),
                        "message": client_message,
                    },
                })
            }
        };

        let body = serde_json::to_vec(&response)?;
        mae_mcp::write_framed(&mut writer, &body, std::time::Duration::from_secs(5)).await?;
    }
}
