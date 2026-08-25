//! Tests for the `cozo_store` submodules, grouped by the same query-domain
//! split as the source: one test file per domain module.

use super::*;

fn make_store() -> (tempfile::TempDir, CozoKbStore) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test_cozo");
    let store = CozoKbStore::open(&path).unwrap();
    (tmp, store)
}

mod agenda_tests;
mod blocks_tests;
mod d4_bench;
mod db_tests;
mod fts_migration_tests;
mod fts_query_tests;
mod fts_search_tests;
mod graph_tests;
mod health_tests;
mod kb_store_impl_tests;
mod links_tests;
mod query_plan_guard_tests;
mod query_plan_tests;
mod schema_tests;
mod source_files_tests;
mod vector_tests;
mod versioning_tests;
#[cfg(feature = "storage-sqlite")]
mod wal_tests;
