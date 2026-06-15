//! mae-daemon — background KB persistence, collaboration, and maintenance service.
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

mod config;
mod handler;
pub mod hygiene;
mod scheduler;

use config::DaemonConfig;
use handler::DaemonState;
use mae_daemon::{collab_handler, doc_store, storage};
use mae_kb::CozoKbStore;
use mae_mcp::broadcast::{EventBroadcaster, SharedBroadcaster};
use scheduler::DaemonScheduler;
use serde_json::{json, Value};
use std::sync::Arc;
use storage::StorageBackend;
use tokio::io::BufReader;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("mae-daemon {VERSION}");
        return;
    }

    if args.iter().any(|a| a == "--check-config") {
        run_check_config();
        return;
    }

    if args.get(1).map(|s| s.as_str()) == Some("doctor") {
        run_doctor();
        return;
    }

    // Keystore management: `mae-daemon keygen [name]`, `mae-daemon keys`.
    if args.get(1).map(|s| s.as_str()) == Some("keygen") {
        std::process::exit(run_keygen(args.get(2).map(|s| s.as_str())));
    }
    if args.get(1).map(|s| s.as_str()) == Some("keys") {
        std::process::exit(run_keys_list());
    }

    // Parse optional CLI overrides: --config, --bind, --data-dir
    let mut config_path: Option<String> = None;
    let mut bind_override: Option<String> = None;
    let mut data_dir_override: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" if i + 1 < args.len() => {
                config_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--bind" if i + 1 < args.len() => {
                bind_override = Some(args[i + 1].clone());
                i += 2;
            }
            "--data-dir" if i + 1 < args.len() => {
                data_dir_override = Some(args[i + 1].clone());
                i += 2;
            }
            _ => i += 1,
        }
    }

    let mut config = if let Some(ref path) = config_path {
        DaemonConfig::load_from(std::path::Path::new(path))
    } else {
        DaemonConfig::load()
    };
    if let Some(ref addr) = bind_override {
        if let Ok(parsed) = addr.parse() {
            config.collab.bind = parsed;
        }
    }
    if let Some(ref dir) = data_dir_override {
        config.data_dir = Some(std::path::PathBuf::from(dir));
    }

    // Initialize tracing
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.log_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!(version = VERSION, "mae-daemon starting");

    // Initialize KB store with SQLite backend
    let data_dir = config.effective_data_dir();
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        tracing::warn!(error = %e, path = %data_dir.display(), "Failed to create data directory");
    }
    let db_path = data_dir.join("daemon-kb.cozo");

    let state = Arc::new(Mutex::new(DaemonState::new()));

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
    let scheduler = DaemonScheduler::new(DaemonConfig::load(), Arc::clone(&state));
    let scheduler_shutdown = shutdown_tx.subscribe();
    let scheduler_handle = tokio::spawn(async move {
        scheduler.run(scheduler_shutdown).await;
    });

    // --- Collab server (absorbed from mae-state-server) ---
    if config.collab.enabled {
        let collab_issues = config.check_collab();
        if !collab_issues.is_empty() {
            for issue in &collab_issues {
                error!(issue = %issue, "collab configuration error");
            }
            // Non-fatal: KB service continues, collab disabled
            warn!("collab service disabled due to config errors");
        } else {
            spawn_collab_server(&config).await;
        }
    } else {
        info!("collab service disabled in config");
    }

    // KB accept loop
    let accept_state = Arc::clone(&state);
    let accept_shutdown = shutdown_tx.subscribe();
    let accept_handle = tokio::spawn(async move {
        accept_loop(listener, accept_state, accept_shutdown).await;
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
async fn spawn_collab_server(config: &DaemonConfig) {
    let collab = &config.collab;

    // Open collab storage
    let collab_data_dir = config.resolve_collab_data_dir();
    let db_path = collab_data_dir.join("state.db");
    let backend = match storage::SqliteBackend::open(&db_path) {
        Ok(b) => Arc::new(b),
        Err(e) => {
            error!(error = %e, path = %db_path.display(), "failed to open collab SQLite");
            warn!("collab service disabled");
            return;
        }
    };

    // Create doc store and broadcaster
    let doc_store = Arc::new(
        doc_store::DocStore::new(backend.clone(), collab.storage.compact_threshold)
            .with_max_documents(collab.sync.max_documents)
            .with_max_wal_entries(collab.storage.max_wal_entries)
            .with_max_document_size(collab.sync.max_document_size_bytes),
    );
    let broadcaster: SharedBroadcaster = Arc::new(std::sync::Mutex::new(EventBroadcaster::new()));

    // Recover documents from storage
    match backend.list_documents().await {
        Ok(docs) => {
            if !docs.is_empty() {
                info!(
                    count = docs.len(),
                    "recovering collab documents from storage"
                );
                for doc_name in &docs {
                    if let Err(e) = doc_store.state_vector(doc_name).await {
                        warn!(doc = %doc_name, error = %e, "recovery failed");
                    }
                }
                info!(count = docs.len(), "collab recovery complete");
            }
        }
        Err(e) => warn!(error = %e, "failed to list collab documents for recovery"),
    }

    // Create auth provider. In "psk" mode the daemon trusts a SET of keys:
    // the trusted-keys keystore plus any legacy psk/psk_command (one unnamed
    // key). A client authenticates by proving knowledge of any trusted key.
    let auth_mode = collab.auth.mode.clone();
    let use_psk = auth_mode == "psk";
    let psk_auth: Option<std::sync::Arc<mae_mcp::auth::PskAuth>> = if use_psk {
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
                Ok(None) => {
                    debug!(path = %path.display(), "no collab keystore present");
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to read collab keystore");
                }
            }
        }

        if keys.is_empty() {
            error!("collab.auth.mode = 'psk' but no keys available (empty keystore and no psk)");
            warn!("collab service disabled");
            return;
        }
        info!(
            auth = "psk",
            trusted_keys = keys.len(),
            "collab authentication configured"
        );
        Some(std::sync::Arc::new(mae_mcp::auth::PskAuth::from_keys(keys)))
    } else {
        info!(auth = %auth_mode, "collab authentication configured");
        None
    };

    // Bind TCP
    let tcp_listener = match tokio::net::TcpListener::bind(&collab.bind).await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            error!(addr = %collab.bind, "collab address already in use");
            warn!("collab service disabled");
            return;
        }
        Err(e) => {
            error!(error = %e, addr = %collab.bind, "failed to bind collab TCP");
            warn!("collab service disabled");
            return;
        }
    };

    let server_start_time = std::time::Instant::now();
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
    tokio::spawn(async move {
        loop {
            match tcp_listener.accept().await {
                Ok((stream, addr)) => {
                    info!(addr = %addr, "collab TCP client connected");
                    let (reader, writer) = stream.into_split();
                    let reader = BufReader::new(reader);
                    let store = Arc::clone(&doc_store);
                    let bc = Arc::clone(&broadcaster);
                    let psk_clone = psk_auth.clone();
                    tokio::spawn(async move {
                        if let Some(ref auth) = psk_clone {
                            collab_handler::handle_client_with_auth(
                                reader,
                                writer,
                                auth.as_ref(),
                                store,
                                bc,
                                server_start_time,
                            )
                            .await;
                        } else {
                            collab_handler::handle_client(
                                reader,
                                writer,
                                store,
                                bc,
                                server_start_time,
                            )
                            .await;
                        }
                    });
                }
                Err(e) => error!(error = %e, "collab TCP accept error"),
            }
        }
    });
}

fn run_check_config() {
    let config = DaemonConfig::load();
    println!("Socket: {}", config.socket.display());
    println!("Data dir: {}", config.effective_data_dir().display());
    println!("Log level: {}", config.log_level);

    // Collab config
    println!("Collab enabled: {}", config.collab.enabled);
    if config.collab.enabled {
        println!("  bind: {}", config.collab.bind);
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
        println!(
            "  collab data_dir: {}",
            config.resolve_collab_data_dir().display()
        );
        println!("  auth.mode: {}", config.collab.auth.mode);
        if config.collab.auth.mode == "psk" {
            if let Some(p) = config.collab.auth.keystore_path() {
                println!(
                    "  auth.keystore: {} ({} key(s))",
                    p.display(),
                    config.collab.auth.keystore_key_count()
                );
            }
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

    println!("Config OK");
}

/// `mae-daemon keygen [name]` — generate a random key, append it to the
/// keystore (creating it 0600), and print it so it can be copied to peers.
fn run_keygen(name: Option<&str>) -> i32 {
    let config = DaemonConfig::load();
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
fn run_keys_list() -> i32 {
    let config = DaemonConfig::load();
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

fn run_doctor() {
    println!("mae-daemon doctor");
    println!("  version: {VERSION}");

    // Check config
    let config = DaemonConfig::load();

    // Check KB data directory
    let data_dir = config.effective_data_dir();
    if data_dir.exists() {
        println!("  kb data_dir: {} (exists)", data_dir.display());
    } else {
        println!("  kb data_dir: {} (will be created)", data_dir.display());
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
        match storage::SqliteBackend::open(&db_path) {
            Ok(_) => println!("  collab sqlite: OK ({})", db_path.display()),
            Err(e) => println!("  collab sqlite: FAILED ({e})"),
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

    println!("  yrs version: 0.22");
}

/// Accept loop: spawn a task per KB client connection.
async fn accept_loop(
    listener: UnixListener,
    state: Arc<Mutex<DaemonState>>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let client_state = Arc::clone(&state);
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, client_state).await {
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
async fn handle_client(
    stream: tokio::net::UnixStream,
    state: Arc<Mutex<DaemonState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    loop {
        let msg = match mae_mcp::read_message(&mut reader).await? {
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
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": e.code(),
                    "message": e.to_string(),
                },
            }),
        };

        let body = serde_json::to_vec(&response)?;
        mae_mcp::write_framed(&mut writer, &body, std::time::Duration::from_secs(5)).await?;
    }
}
