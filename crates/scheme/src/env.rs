//! mae-scheme lexical environments.
//!
//! Environments map variable names to values. They form a chain of
//! scopes (lexical scoping). The VM uses environments for global
//! bindings; closures capture environments for upvalues.
//!
//! @stability: unstable (Phase 13)
//! @since: 0.12.0

use std::collections::HashMap;

use crate::value::Value;

/// A lexical environment: a chain of scopes mapping names to values.
#[derive(Clone, Debug)]
pub struct Env {
    /// Current scope bindings.
    bindings: HashMap<String, Value>,
}

impl Env {
    pub fn new() -> Self {
        Env {
            bindings: HashMap::new(),
        }
    }

    /// Define a new binding in the current scope.
    pub fn define(&mut self, name: String, value: Value) {
        self.bindings.insert(name, value);
    }

    /// Look up a variable in this environment.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.bindings.get(name)
    }

    /// Update an existing binding. Returns false if not found.
    pub fn set(&mut self, name: &str, value: Value) -> bool {
        if let Some(slot) = self.bindings.get_mut(name) {
            *slot = value;
            true
        } else {
            false
        }
    }

    /// Check if a binding exists.
    pub fn contains(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }

    /// Iterate over all bindings.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.bindings.iter()
    }

    /// Number of bindings.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_define_and_get() {
        let mut env = Env::new();
        env.define("x".into(), Value::Int(42));
        assert_eq!(env.get("x"), Some(&Value::Int(42)));
        assert_eq!(env.get("y"), None);
    }

    #[test]
    fn test_set() {
        let mut env = Env::new();
        env.define("x".into(), Value::Int(1));
        assert!(env.set("x", Value::Int(2)));
        assert_eq!(env.get("x"), Some(&Value::Int(2)));
        assert!(!env.set("y", Value::Int(3)));
    }
}
