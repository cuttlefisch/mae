//! A query layer served from its own plain OS thread.
//!
//! @ai-caution: [runtime] Wrap every layer that does BLOCKING I/O — today
//! `RemoteHubQueryLayer`, whose `reqwest::blocking::Client` owns a private tokio
//! runtime — before handing it to the editor. The editor's event loop runs inside
//! `Runtime::block_on`, and dropping a blocking client there panics ("Cannot drop
//! a runtime in a context where blocking is not allowed"). `rebuild_query_layer`
//! replaces the query layer on every register/reimport, so an unwrapped hub layer
//! crashes the editor the first time it is rebuilt. Reproduced standalone before
//! this module existed; `a_bare_blocking_layer_panics_when_dropped_in_a_runtime`
//! keeps that control honest.
//!
//! The inner layer is constructed, queried and dropped on the worker thread and
//! nowhere else. Callers block on a reply channel, which is the same synchronous
//! contract every other `KbQueryLayer` has.

use crate::query::KbQueryLayer;
use crate::store::{HealthReport, KbStoreError, Link, SearchHit, SubGraph};
use crate::Node;
use std::collections::HashMap;
use std::sync::{mpsc, Mutex};

type Job = Box<dyn FnOnce(&dyn KbQueryLayer) + Send>;

/// Forwards every [`KbQueryLayer`] call to a layer that lives on its own thread.
///
/// A worker that has died (the inner layer panicked) turns every later call into
/// an error, `None` or `false`, and [`KbQueryLayer::degraded`] into `true` — it
/// never hangs the caller.
pub struct OffThreadQueryLayer {
    jobs: Mutex<mpsc::Sender<Job>>,
}

impl OffThreadQueryLayer {
    /// Start a worker thread named after `name` and build the inner layer there.
    pub fn spawn<L, F>(name: &str, make: F) -> std::io::Result<Self>
    where
        L: KbQueryLayer + 'static,
        F: FnOnce() -> L + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name(format!("kb-query-{name}"))
            .spawn(move || {
                let layer = make();
                for job in rx {
                    job(&layer);
                }
                // `layer` drops here: on this thread, outside any runtime.
            })?;
        Ok(Self {
            jobs: Mutex::new(tx),
        })
    }

    /// Run `f` against the inner layer and wait for its result. `None` means the
    /// worker is gone.
    fn run<R, F>(&self, f: F) -> Option<R>
    where
        R: Send + 'static,
        F: FnOnce(&dyn KbQueryLayer) -> R + Send + 'static,
    {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |layer| {
            let _ = reply_tx.send(f(layer));
        });
        self.jobs.lock().ok()?.send(job).ok()?;
        reply_rx.recv().ok()
    }

    fn run_result<T, F>(&self, f: F) -> Result<T, KbStoreError>
    where
        T: Send + 'static,
        F: FnOnce(&dyn KbQueryLayer) -> Result<T, KbStoreError> + Send + 'static,
    {
        self.run(f).unwrap_or_else(|| {
            Err(KbStoreError::Storage(
                "query worker thread is gone".to_string(),
            ))
        })
    }
}

impl KbQueryLayer for OffThreadQueryLayer {
    fn get(&self, id: &str) -> Option<Node> {
        let id = id.to_string();
        self.run(move |l| l.get(&id)).flatten()
    }

    fn contains(&self, id: &str) -> bool {
        let id = id.to_string();
        self.run(move |l| l.contains(&id)).unwrap_or(false)
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError> {
        let query = query.to_string();
        self.run_result(move |l| l.search(&query, limit))
    }

    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let id = id.to_string();
        self.run_result(move |l| l.links_from(&id))
    }

    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let id = id.to_string();
        self.run_result(move |l| l.links_to(&id))
    }

    fn list_ids(&self, prefix: Option<&str>) -> Result<Vec<String>, KbStoreError> {
        let prefix = prefix.map(str::to_string);
        self.run_result(move |l| l.list_ids(prefix.as_deref()))
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError> {
        let prefix = prefix.map(str::to_string);
        self.run_result(move |l| l.id_title_pairs(prefix.as_deref()))
    }

    fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Result<Vec<(String, String, String)>, KbStoreError> {
        let prefix = prefix.map(str::to_string);
        self.run_result(move |l| l.id_title_body_triples(prefix.as_deref(), body_limit))
    }

    fn health_report(&self) -> Result<Option<HealthReport>, KbStoreError> {
        self.run_result(|l| l.health_report())
    }

    fn neighborhood(&self, id: &str, depth: u32) -> Result<Option<SubGraph>, KbStoreError> {
        let id = id.to_string();
        self.run_result(move |l| l.neighborhood(&id, depth))
    }

    fn related(&self, id: &str, limit: usize) -> Result<Vec<(String, f64)>, KbStoreError> {
        let id = id.to_string();
        self.run_result(move |l| l.related(&id, limit))
    }

    fn linked_in_degree(&self) -> Result<HashMap<String, usize>, KbStoreError> {
        self.run_result(|l| l.linked_in_degree())
    }

    fn invalidate(&self, id: &str) {
        let id = id.to_string();
        let _ = self.run(move |l| l.invalidate(&id));
    }

    fn node_crdt_state(&self, kb_id: &str, id: &str) -> Option<Vec<u8>> {
        let (kb_id, id) = (kb_id.to_string(), id.to_string());
        self.run(move |l| l.node_crdt_state(&kb_id, &id)).flatten()
    }

    fn todo_nodes(&self) -> Result<Vec<Node>, KbStoreError> {
        self.run_result(|l| l.todo_nodes())
    }

    fn agenda(&self, filter: &crate::AgendaFilter) -> Result<Vec<Node>, KbStoreError> {
        let filter = filter.clone();
        self.run_result(move |l| l.agenda(&filter))
    }

    fn history(&self, id: &str, limit: usize) -> Result<Vec<crate::NodeVersion>, KbStoreError> {
        let id = id.to_string();
        self.run_result(move |l| l.history(&id, limit))
    }

    fn capabilities(&self) -> crate::capabilities::QueryCapabilities {
        // A dead worker can answer nothing, so it claims nothing.
        self.run(|l| l.capabilities()).unwrap_or_else(|| {
            crate::capabilities::QueryCapabilities::all_except(
                crate::capabilities::QueryMethod::ALL,
            )
        })
    }

    fn degraded(&self) -> bool {
        self.run(|l| l.degraded()).unwrap_or(true)
    }

    fn namespace_prefixes(&self) -> Result<Vec<String>, KbStoreError> {
        self.run_result(|l| l.namespace_prefixes())
    }
}

#[cfg(test)]
#[path = "query_off_thread_tests.rs"]
mod tests;
