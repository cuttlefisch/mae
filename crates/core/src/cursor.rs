//! Multi-cursor support.
//!
//! Follows the evil-mc model: cursors are lightweight state containers.
//! Operations replay at each cursor. Small allowlist — not all commands
//! are multi-cursor-aware.

/// Lightweight cursor state (evil-mc model).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    /// Per-cursor visual anchor (for multi-cursor visual selections).
    pub anchor: Option<(usize, usize)>,
}

impl Cursor {
    pub fn new(row: usize, col: usize) -> Self {
        Cursor {
            row,
            col,
            anchor: None,
        }
    }
}

/// Ordered set of cursors. Index 0 = primary.
#[derive(Debug, Clone)]
pub struct CursorSet {
    cursors: Vec<Cursor>,
}

impl CursorSet {
    /// Create a cursor set with a single primary cursor.
    pub fn new(row: usize, col: usize) -> Self {
        CursorSet {
            cursors: vec![Cursor::new(row, col)],
        }
    }

    /// The primary (first) cursor — always exists.
    pub fn primary(&self) -> &Cursor {
        &self.cursors[0]
    }

    /// Mutable primary cursor.
    pub fn primary_mut(&mut self) -> &mut Cursor {
        &mut self.cursors[0]
    }

    /// All secondary cursors (everything except index 0).
    pub fn secondaries(&self) -> &[Cursor] {
        if self.cursors.len() > 1 {
            &self.cursors[1..]
        } else {
            &[]
        }
    }

    /// Add a secondary cursor at (row, col).
    pub fn add(&mut self, row: usize, col: usize) {
        self.cursors.push(Cursor::new(row, col));
    }

    /// Remove cursor at index (cannot remove primary).
    pub fn remove_at(&mut self, idx: usize) {
        if idx > 0 && idx < self.cursors.len() {
            self.cursors.remove(idx);
        }
    }

    /// Remove all secondary cursors, keeping only primary.
    pub fn clear_secondaries(&mut self) {
        self.cursors.truncate(1);
    }

    /// Number of cursors (always >= 1).
    pub fn len(&self) -> usize {
        self.cursors.len()
    }

    /// Always false — a CursorSet always has at least one cursor.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// True if only the primary cursor exists.
    pub fn is_single(&self) -> bool {
        self.cursors.len() == 1
    }

    /// Iterate over all cursors.
    pub fn iter(&self) -> impl Iterator<Item = &Cursor> {
        self.cursors.iter()
    }

    /// Iterate mutably over all cursors.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Cursor> {
        self.cursors.iter_mut()
    }

    /// Sort cursors by (row, col), keeping primary at index 0 afterward.
    pub fn sort_by_position(&mut self) {
        if self.cursors.len() <= 1 {
            return;
        }
        let primary = self.cursors[0].clone();
        self.cursors[1..].sort_by(|a, b| a.row.cmp(&b.row).then(a.col.cmp(&b.col)));
        // Ensure primary stays at index 0
        if self.cursors[0] != primary {
            if let Some(pos) = self.cursors.iter().position(|c| *c == primary) {
                self.cursors.swap(0, pos);
            }
        }
    }

    /// Remove duplicate positions (keep first occurrence).
    pub fn dedup_positions(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.cursors.retain(|c| seen.insert((c.row, c.col)));
        // Never remove primary
        if self.cursors.is_empty() {
            self.cursors.push(Cursor::new(0, 0));
        }
    }
}

/// Operations that can be replayed at multiple cursors.
#[derive(Debug, Clone)]
pub enum CursorOp {
    InsertChar(char),
    InsertText(String),
    DeleteBackward,
    DeleteForward,
    DeleteWord,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_set_new_is_single() {
        let cs = CursorSet::new(0, 0);
        assert_eq!(cs.len(), 1);
        assert!(cs.is_single());
        assert!(cs.secondaries().is_empty());
    }

    #[test]
    fn cursor_set_add_remove() {
        let mut cs = CursorSet::new(0, 0);
        cs.add(1, 5);
        cs.add(2, 3);
        assert_eq!(cs.len(), 3);
        assert!(!cs.is_single());
        assert_eq!(cs.secondaries().len(), 2);

        // Cannot remove primary
        cs.remove_at(0);
        assert_eq!(cs.len(), 3);

        // Remove secondary
        cs.remove_at(1);
        assert_eq!(cs.len(), 2);
    }

    #[test]
    fn cursor_set_clear_secondaries() {
        let mut cs = CursorSet::new(0, 0);
        cs.add(1, 0);
        cs.add(2, 0);
        assert_eq!(cs.len(), 3);
        cs.clear_secondaries();
        assert_eq!(cs.len(), 1);
        assert!(cs.is_single());
    }

    #[test]
    fn cursor_set_dedup() {
        let mut cs = CursorSet::new(0, 0);
        cs.add(1, 0);
        cs.add(0, 0); // duplicate of primary
        cs.add(1, 0); // duplicate
        assert_eq!(cs.len(), 4);
        cs.dedup_positions();
        assert_eq!(cs.len(), 2);
    }

    #[test]
    fn cursor_set_sort() {
        let mut cs = CursorSet::new(5, 0);
        cs.add(1, 0);
        cs.add(3, 0);
        cs.sort_by_position();
        // Primary should still be at index 0
        assert_eq!(cs.primary().row, 5);
        // Secondaries sorted
        let secs: Vec<usize> = cs.secondaries().iter().map(|c| c.row).collect();
        assert_eq!(secs, vec![1, 3]);
    }

    #[test]
    fn single_cursor_zero_overhead() {
        let cs = CursorSet::new(0, 0);
        assert!(cs.is_single());
        // No heap allocations beyond the Vec's initial capacity
        assert_eq!(cs.iter().count(), 1);
    }
}
