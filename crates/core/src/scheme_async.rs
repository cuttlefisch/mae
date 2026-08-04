//! Result slots for Scheme-initiated LSP requests.
//!
//! ## Why this exists
//!
//! MAE's LSP requests are asynchronous: `Editor::lsp.pending_requests` is
//! drained by the outer binary each event-loop tick and forwarded to the LSP
//! task; the answer comes back later as an `LspTaskEvent` delivered to *that
//! same* event loop. A Scheme primitive runs synchronously **inside** an
//! `eval` on the editor's main thread, so it cannot block waiting for the
//! answer — the loop that must deliver it is the loop it would be blocking.
//! Blocking is not "slow" here; it is a deadlock.
//!
//! The MCP surface solves this by *deferring* the tool call
//! (`mae_ai::ExecuteResult::Deferred`): the reply channel is held open and
//! completed when the matching event arrives. Scheme has no reply channel —
//! `eval` has already returned by then. So the Scheme surface splits the
//! request from its result: a request primitive returns a **request id**
//! immediately, and `(lsp-result ID)` reads the slot the id names, answering
//! `pending` until the event arrives. Polling happens across evals (a hook, a
//! test step, the REPL), which is exactly the granularity at which the editor
//! re-enters its event loop.
//!
//! ## Why DAP does not use this
//!
//! Deliberate. DAP's own answer to "what happened after I continued?" is
//! `debug_state`, which reads `Editor::dap.state` — durable session state that
//! a Scheme primitive can read synchronously from the per-eval snapshot. The
//! MCP `dap_start`/`dap_continue`/`dap_step` tools defer only so the *agent*
//! gets a single blocking round trip; a Scheme program already has `(debug-state)`
//! for the same information and does not need a second correlation mechanism.
//!
//! @ai-caution: [dispatch] Do not add a second, parallel result
//! registry for DAP without first establishing that `(debug-state)` genuinely
//! cannot answer the question. Two correlation mechanisms for one editor is
//! the duplication principle #8 exists to prevent.
//!
//! @stability: experimental
//! @since: 0.14.89

use std::collections::VecDeque;

/// How many completed request slots to retain before evicting the oldest.
///
/// A Scheme program that fires requests and never reads the results must not
/// grow the editor's memory without bound. Completed slots are evicted
/// oldest-first; a *pending* slot is never evicted, because dropping it would
/// turn "the answer has not arrived" into "this id never existed", and those
/// two must stay distinguishable.
pub const SCHEME_ASYNC_RETAINED: usize = 64;

/// One Scheme-initiated asynchronous request and its eventual outcome.
#[derive(Debug, Clone)]
pub struct SchemeAsyncRequest {
    /// Correlation id handed back to Scheme at request time.
    pub id: u64,
    /// The MCP tool name whose implementation backs this request
    /// (`"lsp_definition"`, `"lsp_hover"`, …). Matching is by kind because
    /// `LspTaskEvent` carries no correlation id of its own.
    pub kind: String,
    /// `None` while the answer has not arrived. `Some(Ok(json))` carries the
    /// same JSON payload the equivalent MCP tool returns; `Some(Err(msg))`
    /// carries a failure the Scheme caller should see as an error.
    pub outcome: Option<Result<String, String>>,
}

/// One slot as the Scheme side sees it: `(id, kind, outcome)`.
///
/// A tuple alias rather than a struct because the consumer
/// (`mae-scheme`'s `SharedState`) stores it verbatim and matches on the
/// `Option<Result<…>>` directly — naming the three cases is what
/// `(lsp-result ID)` branches on, and a struct would only add a layer of
/// field access over the same three values.
pub type SchemeAsyncSlot = (u64, String, Option<Result<String, String>>);

/// Pending + recently-completed Scheme-initiated async requests.
#[derive(Debug, Default)]
pub struct SchemeAsyncRegistry {
    requests: VecDeque<SchemeAsyncRequest>,
}

impl SchemeAsyncRegistry {
    /// Record a request that has been dispatched and is awaiting its event.
    pub fn register(&mut self, id: u64, kind: impl Into<String>) {
        self.requests.push_back(SchemeAsyncRequest {
            id,
            kind: kind.into(),
            outcome: None,
        });
        self.trim();
    }

    /// Record a request whose outcome is already known (it failed to dispatch,
    /// or the backing implementation answered synchronously).
    pub fn register_completed(
        &mut self,
        id: u64,
        kind: impl Into<String>,
        outcome: Result<String, String>,
    ) {
        self.requests.push_back(SchemeAsyncRequest {
            id,
            kind: kind.into(),
            outcome: Some(outcome),
        });
        self.trim();
    }

    /// Complete the **oldest** still-pending request of `kind`, returning
    /// whether one was found.
    ///
    /// Oldest-first is the only ordering available: an `LspTaskEvent` carries
    /// no correlation id, so two concurrent requests of the same kind can only
    /// be matched FIFO. This is the same limitation the MCP deferred path has
    /// (it holds exactly one deferred reply per session for the same reason)
    /// — stated here rather than hidden, because it means two `(lsp-hover)`
    /// calls issued in one eval may receive each other's answers.
    pub fn complete_oldest(&mut self, kind: &str, outcome: Result<String, String>) -> bool {
        for req in self.requests.iter_mut() {
            if req.kind == kind && req.outcome.is_none() {
                req.outcome = Some(outcome);
                return true;
            }
        }
        false
    }

    /// Every still-pending request's kind, oldest first — so a caller matching
    /// an untagged event can try each candidate kind in arrival order.
    pub fn pending_kinds(&self) -> Vec<String> {
        self.requests
            .iter()
            .filter(|r| r.outcome.is_none())
            .map(|r| r.kind.clone())
            .collect()
    }

    /// Fail every still-pending request, e.g. when the language server reports
    /// an error that cannot be attributed to one specific request.
    ///
    /// Failing all of them is the fail-closed choice: leaving them pending
    /// forever would make `(lsp-result ID)` answer `pending` for the rest of
    /// the session, which reads as "still working" when nothing is.
    pub fn fail_all_pending(&mut self, message: &str) -> usize {
        let mut n = 0;
        for req in self.requests.iter_mut() {
            if req.outcome.is_none() {
                req.outcome = Some(Err(message.to_string()));
                n += 1;
            }
        }
        n
    }

    /// Snapshot for the Scheme side: `(id, kind, outcome)` per slot.
    pub fn snapshot(&self) -> Vec<SchemeAsyncSlot> {
        self.requests
            .iter()
            .map(|r| (r.id, r.kind.clone(), r.outcome.clone()))
            .collect()
    }

    /// Number of slots currently held (pending + retained-completed).
    pub fn len(&self) -> usize {
        self.requests.len()
    }

    /// Whether no slots are held.
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    /// Evict completed slots, oldest first, until at most
    /// [`SCHEME_ASYNC_RETAINED`] remain. Pending slots are never evicted.
    fn trim(&mut self) {
        while self.requests.len() > SCHEME_ASYNC_RETAINED {
            let Some(pos) = self.requests.iter().position(|r| r.outcome.is_some()) else {
                // Everything outstanding is still pending — nothing may be
                // dropped. An unbounded backlog of genuinely-pending requests
                // is a caller bug, not something to paper over by discarding
                // ids that are still legitimately awaited.
                break;
            };
            self.requests.remove(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_and_absent_are_distinguishable() {
        let mut reg = SchemeAsyncRegistry::default();
        reg.register(1, "lsp_hover");
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, 1);
        assert!(snap[0].2.is_none(), "a registered request reads as pending");
        // An id that was never registered simply is not in the snapshot —
        // which is what lets `(lsp-result 999)` say "unknown id" instead of
        // "pending forever".
        assert!(!snap.iter().any(|(id, _, _)| *id == 999));
    }

    #[test]
    fn completion_is_fifo_within_a_kind_and_ignores_other_kinds() {
        let mut reg = SchemeAsyncRegistry::default();
        reg.register(1, "lsp_hover");
        reg.register(2, "lsp_definition");
        reg.register(3, "lsp_hover");

        assert!(reg.complete_oldest("lsp_hover", Ok("first".into())));
        let snap = reg.snapshot();
        // The OLDEST hover took it; the newer hover and the definition are untouched.
        assert_eq!(snap[0].2, Some(Ok("first".to_string())));
        assert_eq!(snap[1].2, None);
        assert_eq!(snap[2].2, None);

        assert!(reg.complete_oldest("lsp_hover", Ok("second".into())));
        assert_eq!(reg.snapshot()[2].2, Some(Ok("second".to_string())));

        // A third hover event has nothing left to complete.
        assert!(!reg.complete_oldest("lsp_hover", Ok("third".into())));
    }

    #[test]
    fn a_completed_slot_is_not_re_completed_by_a_later_event() {
        let mut reg = SchemeAsyncRegistry::default();
        reg.register_completed(1, "lsp_hover", Err("dispatch failed".into()));
        assert!(!reg.complete_oldest("lsp_hover", Ok("late answer".into())));
        assert_eq!(
            reg.snapshot()[0].2,
            Some(Err("dispatch failed".to_string())),
            "a late event must not overwrite an already-decided outcome"
        );
    }

    #[test]
    fn eviction_drops_completed_slots_and_never_pending_ones() {
        let mut reg = SchemeAsyncRegistry::default();
        // One genuinely-pending request up front, then a flood of completed ones.
        reg.register(0, "lsp_hover");
        for i in 1..(SCHEME_ASYNC_RETAINED as u64 * 3) {
            reg.register_completed(i, "lsp_definition", Ok(format!("{i}")));
        }
        assert!(reg.len() <= SCHEME_ASYNC_RETAINED);
        let snap = reg.snapshot();
        assert!(
            snap.iter()
                .any(|(id, _, outcome)| *id == 0 && outcome.is_none()),
            "the pending slot must survive eviction pressure"
        );
        // And the survivors are the most recent completed ones, not the oldest.
        let max = snap.iter().map(|(id, _, _)| *id).max().unwrap();
        assert_eq!(max, SCHEME_ASYNC_RETAINED as u64 * 3 - 1);
    }

    #[test]
    fn an_all_pending_backlog_is_not_silently_discarded() {
        let mut reg = SchemeAsyncRegistry::default();
        let n = SCHEME_ASYNC_RETAINED as u64 * 2;
        for i in 0..n {
            reg.register(i, "lsp_hover");
        }
        // Nothing is completed, so nothing may be evicted — the registry grows
        // past the cap rather than losing an id a caller is still awaiting.
        assert_eq!(reg.len(), n as usize);
        assert_eq!(reg.pending_kinds().len(), n as usize);
    }

    #[test]
    fn fail_all_pending_leaves_decided_slots_alone() {
        let mut reg = SchemeAsyncRegistry::default();
        reg.register_completed(1, "lsp_hover", Ok("done".into()));
        reg.register(2, "lsp_definition");
        reg.register(3, "lsp_references");
        assert_eq!(reg.fail_all_pending("server crashed"), 2);
        let snap = reg.snapshot();
        assert_eq!(snap[0].2, Some(Ok("done".to_string())));
        assert_eq!(snap[1].2, Some(Err("server crashed".to_string())));
        assert_eq!(snap[2].2, Some(Err("server crashed".to_string())));
        assert!(reg.pending_kinds().is_empty());
    }
}
