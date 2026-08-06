//! Re-export of [`mae_effect_sandbox`] — the test-effect sandbox.
//!
//! The implementation is a dependency-free leaf crate because the same guard is
//! needed in `mae-mcp`, which is `mae-core`'s sibling rather than its
//! dependency. Re-exported here so `mae-core` and everything above it keep the
//! natural spelling (`mae_core::effect_sandbox::with_external_effects`) while
//! there is exactly one implementation of the test-binary detection.
pub use mae_effect_sandbox::*;
