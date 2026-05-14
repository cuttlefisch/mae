//! Per-language execution backends for babel source blocks.
//!
//! Each backend handles a set of languages with optimized execution strategies:
//! - `ShellBackend`: subprocess per execution (bash, zsh, fish)
//! - `ScriptBackend`: interpreted languages with session support (python, ruby, node)
//! - `CompiledBackend`: compile-cache-execute (rust, go, c)
//! - `InternalBackend`: scheme blocks routed to editor runtime

pub mod compiled;
pub mod internal;
pub mod script;
pub mod shell;

use std::path::Path;

use super::execute::ExecResult;
use super::SrcBlock;

/// Trait for language-specific execution backends.
pub trait LanguageBackend: Send {
    /// Backend name for diagnostics.
    fn name(&self) -> &str;

    /// Whether this backend can handle the given language.
    fn can_handle(&self, language: &str) -> bool;

    /// Execute a source block.
    fn execute(&mut self, block: &SrcBlock, dir: &Path, vars: &[(String, String)]) -> ExecResult;
}
