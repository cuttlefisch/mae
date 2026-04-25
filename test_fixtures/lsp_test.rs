// LSP self-test fixture — a small Rust file with known symbols.
//
// Used by MAE's `:self-test lsp` category. The AI agent opens this file,
// waits for rust-analyzer, then tests hover/definition/references on
// known positions.
//
// DO NOT MODIFY line numbers without updating crates/ai/src/executor/mod.rs.
//
// Key positions (1-indexed, for AI tool calls):
//   Line 15, Col 12: "Counter" struct name  → hover, references
//   Line 20, Col 12: "new" fn name          → definition target
//   Line 35, Col 28: "new" in Counter::new  → definition (resolves to line 20)

/// A simple counter for LSP testing.
pub struct Counter {
    value: i32,
}

impl Counter {
    pub fn new(initial: i32) -> Self {
        Counter { value: initial }
    }

    pub fn increment(&mut self) {
        self.value += 1;
    }

    pub fn get(&self) -> i32 {
        self.value
    }
}

/// Helper function that uses Counter.
pub fn count_to(n: i32) -> i32 {
    let mut c = Counter::new(0);
    for _ in 0..n {
        c.increment();
    }
    c.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_to() {
        assert_eq!(count_to(5), 5);
    }

    #[test]
    fn test_counter_new() {
        let c = Counter::new(42);
        assert_eq!(c.get(), 42);
    }
}
