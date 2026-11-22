//! Test-only seams inside store writes, so a test can force a precise failure
//! instead of waiting for a probabilistic one. Compiled only for tests.

use std::cell::Cell;

thread_local! {
    static FAIL_AFTER_NODE_ROW: Cell<bool> = const { Cell::new(false) };
}

/// Make THIS thread's next `insert_node` calls fail after the node-row step,
/// before the node's derived rows (links, meta members) are written.
pub(crate) fn set_fail_after_node_row(fail: bool) {
    FAIL_AFTER_NODE_ROW.with(|f| f.set(fail));
}

pub(crate) fn fail_after_node_row() -> bool {
    FAIL_AFTER_NODE_ROW.with(|f| f.get())
}

thread_local! {
    static FAIL_REDERIVE: Cell<bool> = const { Cell::new(false) };
}

/// Make THIS thread's next re-derivation of stored edges (on open, when the
/// derivation version is stale) fail before it writes anything.
pub(crate) fn set_fail_rederive(fail: bool) {
    FAIL_REDERIVE.with(|f| f.set(fail));
}

pub(crate) fn fail_rederive() -> bool {
    FAIL_REDERIVE.with(|f| f.get())
}

thread_local! {
    static FAIL_TYPED_REPAIR_AT: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Make THIS thread's typed-link repairs stop with an error when they reach
/// batch `n` (0-based), after every earlier batch was written: an
/// interruption between batches.
pub(crate) fn set_fail_typed_repair_at(n: Option<usize>) {
    FAIL_TYPED_REPAIR_AT.with(|f| f.set(n));
}

pub(crate) fn fail_typed_repair_at_batch(n: usize) -> bool {
    FAIL_TYPED_REPAIR_AT.with(|f| f.get()) == Some(n)
}

thread_local! {
    static AFTER_TYPED_REPAIR_PLAN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` once in THIS thread's next typed-link repair, after its plan (and
/// backup) and before its first node write: where another process acting on
/// the same store would land.
pub(crate) fn set_after_typed_repair_plan(f: impl FnOnce() + 'static) {
    AFTER_TYPED_REPAIR_PLAN.with(|h| *h.borrow_mut() = Some(Box::new(f)));
}

pub(crate) fn run_after_typed_repair_plan() {
    if let Some(f) = AFTER_TYPED_REPAIR_PLAN.with(|h| h.borrow_mut().take()) {
        f();
    }
}

static PASSES_RUN: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Record that a derivation pass ran on the store at `path` (any thread).
pub(crate) fn record_pass(path: &std::path::Path) {
    PASSES_RUN.lock().unwrap().push(path.to_path_buf());
}

/// How many derivation passes have run on the store at `path`.
pub(crate) fn passes_run(path: &std::path::Path) -> usize {
    PASSES_RUN
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.as_path() == path)
        .count()
}
