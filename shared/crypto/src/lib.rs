//! mae-crypto: the key and envelope crypto MAE runs on every surface.
//! @stability: experimental
//!
//! Pure functions, no I/O, no clock: everything here builds for
//! `wasm32-unknown-unknown`, so the daemon, the editor and the browser run
//! one implementation rather than three that could disagree.
//!
//! - [`member`]: the Ed25519 member key (generation, public key,
//!   fingerprint, signing), in the forms `mae_mcp::identity` writes.
//! - [`identity_challenge`]: the identity-binding challenge's wire format and
//!   the rules a key holder signs it under (ADR-098 Amendment 1, C2);
//!   `mae_mcp::identity_challenge` is a wrapper over it.
//! - [`backup`] (feature `backup`): the passphrase-sealed member-key backup,
//!   envelope version 1.
//! - [`content_crypto`]: ADR-037's content AEAD and member key wrap,
//!   re-exported by mae-sync as `mae_sync::content_crypto`.
//! - [`rng`]: the one randomness source (the host's, in the WASM module).

#[cfg(feature = "backup")]
pub mod backup;
pub mod content_crypto;
pub mod identity_challenge;
pub mod member;
pub mod rng;

/// This crate's version: the contract mae-crypto-wasm reports and the
/// `mae-crypto-vX.Y.Z` tags name (the two crates move together).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
