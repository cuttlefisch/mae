//! ThemeStyle → Skia Paint/Font conversion.
//!
//! Maps mae_core theme styles to Skia rendering primitives. This is the
//! GUI equivalent of renderer/src/theme_convert.rs (which maps to ratatui).
//!
//! Phase 8 M1: basic fg/bg color mapping. Bold/italic/underline will be
//! added in M2 when variable-height text layout lands.
