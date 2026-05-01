//! Incremental reparsing via `Tree::edit` integration.
//!
//! The `apply_edit` method on `SyntaxMap` and the incremental reparse path
//! in `tree_for` are kept in the main `SyntaxMap` impl (mod.rs) since they
//! operate on `SyntaxState`. This file exists as a logical grouping marker
//! and re-exports nothing -- all incremental logic lives in mod.rs.
//!
//! This module is reserved for future extraction if the incremental path
//! grows beyond what fits in the `SyntaxMap` impl block.
