//! mae-dap: DAP client — breakpoints, call stacks, variables exposed to Scheme and AI.
//!
//! @stability: stable
//! @since: 0.4.0

pub mod client;
pub mod manager;
pub mod protocol;
pub mod transport;

pub use client::{DapClient, DapEventKind, DapServerConfig};
pub use manager::{run_dap_task, DapCommand, DapTaskEvent};
pub use protocol::*;
pub use transport::{DapTransport, TransportError};
