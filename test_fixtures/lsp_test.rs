// LSP self-test fixture — a small Rust file with known symbols.
//
// This file is used by MAE's `:self-test lsp` category. The AI agent
// opens this file to trigger LSP, then tests definition, references,
// hover, and document symbols on known positions.
//
// DO NOT MODIFY the line numbers without updating the self-test suite
// in crates/ai/src/executor/mod.rs.

/// A simple counter for LSP testing.
pub struct Counter {
    // line 12 — struct definition
    value: i32,
}

impl Counter {
    // line 17 — impl block
    pub fn new(initial: i32) -> Self {
        // line 19 — constructor
        Counter { value: initial }
    }

    pub fn increment(&mut self) {
        // line 24 — method
        self.value += 1;
    }

    pub fn get(&self) -> i32 {
        // line 29 — getter
        self.value
    }
}

/// Helper function that uses Counter.
pub fn count_to(n: i32) -> i32 {
    // line 36 — function using Counter (reference target)
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
