//! mae-state-server — MAE collaborative state server.
//!
//! Manages CRDT document state over TCP (and optionally Unix sockets).
//! Uses yrs (YATA algorithm) for conflict-free collaborative editing
//! with WAL-based SQLite persistence.
//!
//! ## Security
//!
//! Supports `auth.mode = "none"` (default, backward compatible) or
//! `auth.mode = "psk"` (mutual HMAC-SHA256 authentication).
//! SSH key exchange planned for v0.12.0.

mod cli;
mod config;

use mae_state_server::{auth, doc_store, handler, storage};

use std::sync::Arc;

use mae_mcp::broadcast::{EventBroadcaster, SharedBroadcaster};
use storage::StorageBackend;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

#[tokio::main]
async fn main() {
    let args = cli::parse_args();

    match args.command {
        cli::Command::Version => {
            println!("mae-state-server {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        cli::Command::CheckConfig => {
            run_check_config();
            return;
        }
        cli::Command::Doctor => {
            run_doctor();
            return;
        }
        cli::Command::Start(start_args) => {
            run_server(start_args).await;
        }
    }
}

fn run_check_config() {
    let config = match config::ServerConfig::load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    let issues = config.check();
    if issues.is_empty() {
        println!("Configuration OK");
        println!("  bind: {}", config.bind);
        println!("  storage.backend: {}", config.storage.backend);
        println!(
            "  storage.compact_threshold: {}",
            config.storage.compact_threshold
        );
        println!(
            "  sync.heartbeat_interval_secs: {}",
            config.sync.heartbeat_interval_secs
        );
        println!("  sync.max_documents: {}", config.sync.max_documents);
        println!("  data_dir: {}", config.resolve_data_dir().display());
        println!("  auth.mode: {}", config.auth.mode);
    } else {
        eprintln!("Configuration issues:");
        for issue in &issues {
            eprintln!("  - {issue}");
        }
        std::process::exit(1);
    }
}

fn run_doctor() {
    println!("mae-state-server doctor");
    println!("  version: {}", env!("CARGO_PKG_VERSION"));

    // Check config.
    let config = config::ServerConfig::load(None).unwrap_or_default();
    let issues = config.check();
    if issues.is_empty() {
        println!("  config: OK");
    } else {
        println!("  config: {} issue(s)", issues.len());
        for issue in &issues {
            println!("    - {issue}");
        }
    }

    // Check data directory.
    let data_dir = config.resolve_data_dir();
    if data_dir.exists() {
        println!("  data_dir: {} (exists)", data_dir.display());
    } else {
        println!("  data_dir: {} (will be created)", data_dir.display());
    }

    // Check SQLite.
    let db_path = data_dir.join("state.db");
    match storage::SqliteBackend::open(&db_path) {
        Ok(_) => println!("  sqlite: OK ({})", db_path.display()),
        Err(e) => println!("  sqlite: FAILED ({e})"),
    }

    // Check port.
    match std::net::TcpListener::bind(config.bind) {
        Ok(_) => println!("  port {}: available", config.bind.port()),
        Err(e) => println!("  port {}: {} ({})", config.bind.port(), e, config.bind),
    }

    println!("  yrs version: 0.22");
}

async fn run_server(start_args: cli::StartArgs) {
    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load config, with CLI overrides.
    let mut config = config::ServerConfig::load(start_args.config.as_ref()).unwrap_or_else(|e| {
        error!(error = %e, "failed to load config, using defaults");
        config::ServerConfig::default()
    });

    config.bind = start_args.bind;
    if let Some(unix) = start_args.unix_socket {
        config.unix_socket = Some(unix);
    }
    if let Some(data_dir) = start_args.data_dir {
        config.storage.data_dir = Some(data_dir);
    }
    config.storage.compact_threshold = start_args.compact_threshold;

    // Validate.
    let issues = config.check();
    if !issues.is_empty() {
        for issue in &issues {
            error!(issue = %issue, "configuration error");
        }
        std::process::exit(1);
    }

    // Open storage.
    let data_dir = config.resolve_data_dir();
    let db_path = data_dir.join("state.db");
    let backend = match storage::SqliteBackend::open(&db_path) {
        Ok(b) => Arc::new(b),
        Err(e) => {
            error!(error = %e, path = %db_path.display(), "failed to open SQLite");
            std::process::exit(1);
        }
    };

    // Create doc store and broadcaster.
    let doc_store = Arc::new(
        doc_store::DocStore::new(backend.clone(), config.storage.compact_threshold)
            .with_max_documents(config.sync.max_documents)
            .with_max_wal_entries(config.storage.max_wal_entries)
            .with_max_document_size(config.sync.max_document_size_bytes),
    );
    let broadcaster: SharedBroadcaster = Arc::new(std::sync::Mutex::new(EventBroadcaster::new()));

    // Recover documents from storage.
    match backend.list_documents().await {
        Ok(docs) => {
            if !docs.is_empty() {
                info!(count = docs.len(), "recovering documents from storage");
                for doc_name in &docs {
                    // Touch each doc to trigger recovery.
                    if let Err(e) = doc_store.state_vector(doc_name).await {
                        warn!(doc = %doc_name, error = %e, "recovery failed");
                    }
                }
                info!(count = docs.len(), "recovery complete");
            }
        }
        Err(e) => warn!(error = %e, "failed to list documents for recovery"),
    }

    // Create auth provider.
    let auth_mode = config.auth.mode.clone();
    let use_psk = auth_mode == "psk";
    let psk_key: Option<String> = if use_psk {
        let key = auth::load_psk(
            config.auth.psk_command.as_deref(),
            config.auth.psk.as_deref(),
        )
        .await;
        if key.is_none() {
            error!("auth.mode = 'psk' but no PSK could be loaded");
            std::process::exit(1);
        }
        key
    } else {
        None
    };
    info!(auth = %auth_mode, "authentication configured");

    // Bind TCP.
    let tcp_listener = match TcpListener::bind(&config.bind).await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("error: address {} is already in use", config.bind);
            if cfg!(target_os = "macos") {
                eprintln!("hint: check with `lsof -i :{}`", config.bind.port());
            } else {
                eprintln!("hint: check with `ss -tlnp | grep {}`", config.bind.port());
            }
            eprintln!("hint: use --bind to specify a different address");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("error: failed to bind {}: {}", config.bind, e);
            std::process::exit(1);
        }
    };

    let server_start_time = std::time::Instant::now();

    info!(
        bind = %config.bind,
        data_dir = %data_dir.display(),
        compact_threshold = config.storage.compact_threshold,
        "mae-state-server started"
    );

    // Optional Unix socket.
    let unix_listener = if let Some(ref unix_path) = config.unix_socket {
        let _ = std::fs::remove_file(unix_path);
        match tokio::net::UnixListener::bind(unix_path) {
            Ok(l) => {
                info!(path = %unix_path.display(), "Unix socket listening");
                Some(l)
            }
            Err(e) => {
                warn!(error = %e, path = %unix_path.display(), "failed to bind Unix socket");
                None
            }
        }
    } else {
        None
    };

    // Spawn Unix accept loop if configured.
    if let Some(unix_listener) = unix_listener {
        let store = Arc::clone(&doc_store);
        let bc = Arc::clone(&broadcaster);
        tokio::spawn(async move {
            loop {
                match unix_listener.accept().await {
                    Ok((stream, _)) => {
                        info!("Unix client connected");
                        let (reader, writer) = stream.into_split();
                        let reader = BufReader::new(reader);
                        let store = Arc::clone(&store);
                        let bc = Arc::clone(&bc);
                        tokio::spawn(async move {
                            handler::handle_client(reader, writer, store, bc, server_start_time)
                                .await;
                        });
                    }
                    Err(e) => error!(error = %e, "Unix accept error"),
                }
            }
        });
    }

    // Spawn background compaction + eviction task.
    {
        let compact_interval = config.sync.compaction_interval_secs;
        let eviction_secs = config.sync.idle_eviction_secs;
        let store = Arc::clone(&doc_store);
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(compact_interval.max(10)));
            interval.tick().await; // skip first immediate tick
            loop {
                interval.tick().await;

                // Compact all in-memory documents.
                let names = store.document_names().await;
                for name in &names {
                    if let Err(e) = store.compact_doc(name).await {
                        warn!(doc = %name, error = %e, "background compaction failed");
                    }
                }
                if !names.is_empty() {
                    debug!(count = names.len(), "background compaction complete");
                }

                // Evict idle documents.
                if eviction_secs > 0 {
                    let evicted = store.evict_idle(eviction_secs).await;
                    if !evicted.is_empty() {
                        debug!(count = evicted.len(), "idle eviction complete");
                    }
                }
            }
        });
    }

    // Main event loop: TCP accept + shutdown signal.
    loop {
        tokio::select! {
            biased;

            _ = tokio::signal::ctrl_c() => {
                info!("shutting down...");
                info!("compacting all documents...");
                if let Err(e) = doc_store.compact_all().await {
                    warn!(error = %e, "compaction error during shutdown");
                }
                info!("shutdown complete");
                break;
            }

            result = tcp_listener.accept() => {
                match result {
                    Ok((stream, addr)) => {
                        info!(addr = %addr, "TCP client connected");
                        let (reader, writer) = stream.into_split();
                        let reader = BufReader::new(reader);
                        let store = Arc::clone(&doc_store);
                        let bc = Arc::clone(&broadcaster);
                        let psk_clone = psk_key.clone();
                        tokio::spawn(async move {
                            if let Some(ref key) = psk_clone {
                                let psk_auth = auth::PskAuth::new(key);
                                handler::handle_client_with_auth(
                                    reader, writer, &psk_auth, store, bc, server_start_time,
                                ).await;
                            } else {
                                handler::handle_client(
                                    reader, writer, store, bc, server_start_time,
                                ).await;
                            }
                        });
                    }
                    Err(e) => error!(error = %e, "TCP accept error"),
                }
            }
        }
    }
}
