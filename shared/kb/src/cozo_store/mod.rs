//! CozoKbStore — graph-native KB persistence using CozoDB (Datalog).
//!
//! Sole KB backend since v0.12.0. Storage engine selected by feature flag:
//! - `storage-sled` (default): sled embedded storage
//! - `storage-sqlite`: CozoDB's native SQLite engine (used by mae-daemon)
//!
//! CozoDB provides:
//! - Datalog query engine with recursive queries
//! - ACID + MVCC transactions
//! - Multiple storage backends (sled, SQLite, RocksDB)
//!
//! Graph algorithms (PageRank, community detection) require the `graph-algo`
//! feature, currently disabled due to upstream `graph_builder` rayon compat
//! issue. Will be re-enabled when upstream fixes land.
//!
//! @ai-caution: [architecture-debt] Dense but organized CozoDB Datalog query
//! module, split by query domain (schema/db/graph/links/blocks/agenda/health/
//! versioning/vector/suggestions/source_files/util + the `KbStore` trait
//! impl). Tracked in `.claude/commands/mae-audit.md`'s "Known exceptions"
//! and `ROADMAP.md`'s "Architecture Debt" section — re-measure before adding
//! more query surface here.

use crate::store::{
    AgendaFilter, Block, HealthReport, KbStore, KbStoreError, Link, MetaMember, NodeVersion,
    PendingUpdate, SearchHit, SubGraph, VectorHit,
};
use crate::{Node, NodeKind};
use cozo::{DataValue, DbInstance, NamedRows, ScriptMutability, Vector};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod agenda;
mod blocks;
mod db;
mod graph;
mod health;
mod kb_store_impl;
mod links;
mod schema;
mod source_files;
mod suggestions;
mod util;
mod vector;
mod versioning;

/// CozoDB-backed KbStore using SQLite embedded storage.
pub struct CozoKbStore {
    db: DbInstance,
    path: PathBuf,
}

impl std::fmt::Debug for CozoKbStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CozoKbStore")
            .field("path", &self.path)
            .finish()
    }
}

#[cfg(test)]
mod tests;
