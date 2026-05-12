//! mae-spell: Spell checking via aspell/hunspell subprocess pipe.
//!
//! @stability: experimental
//! @since: 0.9.0
//!
//! Communicates with aspell or hunspell in pipe mode to check text and
//! get correction suggestions. Results are cached per-buffer for rendering
//! inline squiggly markers.

pub mod checker;

pub use checker::{check_available, check_text, Misspelling, SpellBackend};
