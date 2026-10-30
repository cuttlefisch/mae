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
