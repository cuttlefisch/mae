//! # Module: pkg — Module system and package management
//!
//! Implements MAE's Doom-style module system: manifest parsing, dependency
//! resolution, module loading, and the three-file configuration model.
//!
//! ## Architecture Role
//! Part of the `mae` binary crate. Sits between bootstrap (config loading)
//! and the SchemeRuntime. Discovers modules, resolves dependencies via
//! topological sort, loads autoloads before user config.scm.
//!
//! ## Key Types
//! - `ModuleManifest` — parsed module.toml
//! - `ModuleState` — runtime status of a loaded module
//! - `ModuleRegistry` — tracks all discovered and loaded modules

pub mod cli;
pub mod loader;
pub mod manifest;
pub mod resolver;

pub use loader::ModuleRegistry;
pub use manifest::ModuleManifest;
pub use resolver::resolve_load_order;
