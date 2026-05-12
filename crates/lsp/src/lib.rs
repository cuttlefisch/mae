//! mae-lsp: LSP client — types, references, diagnostics exposed to Scheme and AI.
//!
//! @stability: stable
//! @since: 0.4.0

pub mod client;
pub mod manager;
pub mod protocol;
pub mod transport;

pub use client::{language_id_from_path, path_to_uri, LspClient, LspEvent, LspServerConfig};
pub use manager::{run_lsp_task, LspCommand, LspManager, LspTaskEvent};
pub use protocol::*;
pub use transport::{LspTransport, TransportError};
