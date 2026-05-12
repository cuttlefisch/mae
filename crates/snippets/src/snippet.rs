//! Snippet expansion session — field tracking, navigation, mirror updates.

use std::collections::HashMap;

use crate::parser::{parse_snippet, ParseError, SnippetPart};

/// A single editable field in an expanded snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetField {
    /// Tab-stop index (0 = final cursor).
    pub index: u32,
    /// Byte offset in the expanded text.
    pub offset: usize,
    /// Length in bytes.
    pub length: usize,
    /// Whether this is a mirror (duplicate of another field with the same index).
    pub is_mirror: bool,
}

/// An active snippet expansion session.
///
/// Tracks the expanded text and allows navigating between fields.
/// When a field is updated, all mirrors (same index) are updated too.
#[derive(Debug, Clone)]
pub struct SnippetSession {
    /// The expanded text with defaults filled in.
    pub text: String,
    /// All fields, sorted by position in text.
    pub fields: Vec<SnippetField>,
    /// Index into `fields` for the currently active field.
    pub current_field: usize,
    /// Whether the session is finished (final cursor reached or no fields).
    pub finished: bool,
}

impl SnippetSession {
    /// Expand a snippet template into a session.
    ///
    /// Returns the session with the first non-zero field active, or finished
    /// if there are no fields (pure literal).
    pub fn expand(template: &str) -> Result<Self, ParseError> {
        let parts = parse_snippet(template)?;
        let mut text = String::new();
        let mut fields = Vec::new();
        let defaults = collect_defaults(&parts);

        expand_parts(&parts, &mut text, &mut fields, &defaults);

        // Mark mirrors: for each index, the first occurrence is primary, rest are mirrors
        let mut seen_primary = std::collections::HashSet::new();
        for field in &mut fields {
            if !seen_primary.insert(field.index) {
                field.is_mirror = true;
            }
        }

        // Sort fields: primary fields by index (ascending, 0 last), then mirrors
        fields.sort_by(|a, b| {
            let a_key = if a.index == 0 { u32::MAX } else { a.index };
            let b_key = if b.index == 0 { u32::MAX } else { b.index };
            a_key
                .cmp(&b_key)
                .then(a.is_mirror.cmp(&b.is_mirror))
                .then(a.offset.cmp(&b.offset))
        });

        let finished = fields.is_empty();
        let current_field = 0;

        Ok(Self {
            text,
            fields,
            current_field,
            finished,
        })
    }

    /// Get the byte range (offset, length) of the current field, or None if finished.
    pub fn current_range(&self) -> Option<(usize, usize)> {
        if self.finished || self.fields.is_empty() {
            return None;
        }
        let f = &self.fields[self.current_field];
        Some((f.offset, f.length))
    }

    /// Advance to the next field. Returns its (offset, length), or None if done.
    pub fn next_field(&mut self) -> Option<(usize, usize)> {
        if self.finished || self.fields.is_empty() {
            return None;
        }

        // Skip ahead to next primary (non-mirror) field with a different index
        let current_idx = self.fields[self.current_field].index;
        let mut next = self.current_field + 1;
        while next < self.fields.len() {
            if !self.fields[next].is_mirror && self.fields[next].index != current_idx {
                break;
            }
            next += 1;
        }

        if next >= self.fields.len() {
            // Check if there's a $0 field
            if let Some(final_pos) = self.fields.iter().position(|f| f.index == 0) {
                self.current_field = final_pos;
                self.finished = true;
                let f = &self.fields[final_pos];
                return Some((f.offset, f.length));
            }
            self.finished = true;
            return None;
        }

        self.current_field = next;
        // Landing on $0 means we're done
        if self.fields[next].index == 0 {
            self.finished = true;
        }
        let f = &self.fields[self.current_field];
        Some((f.offset, f.length))
    }

    /// Go back to the previous field. Returns its (offset, length), or None if at start.
    pub fn prev_field(&mut self) -> Option<(usize, usize)> {
        if self.fields.is_empty() {
            return None;
        }

        self.finished = false;

        // Find previous primary field
        let current_idx = self.fields[self.current_field].index;
        let mut prev = self.current_field;
        loop {
            if prev == 0 {
                // Already at first field
                let f = &self.fields[0];
                self.current_field = 0;
                return Some((f.offset, f.length));
            }
            prev -= 1;
            if !self.fields[prev].is_mirror && self.fields[prev].index != current_idx {
                break;
            }
        }

        self.current_field = prev;
        let f = &self.fields[self.current_field];
        Some((f.offset, f.length))
    }

    /// Update the current field's text. All mirrors with the same index are updated too.
    ///
    /// Returns the new (offset, length) of the current field after the update.
    pub fn update_field(&mut self, new_text: &str) -> Option<(usize, usize)> {
        if self.finished || self.fields.is_empty() {
            return None;
        }

        let target_index = self.fields[self.current_field].index;
        let old_len = self.fields[self.current_field].length;
        let new_len = new_text.len();
        let len_diff = new_len as isize - old_len as isize;

        // Collect all field positions with this index, sorted by offset descending
        // (so we can replace from end to start without invalidating offsets)
        let mut positions: Vec<usize> = self
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.index == target_index)
            .map(|(i, _)| i)
            .collect();
        positions.sort_by(|a, b| self.fields[*b].offset.cmp(&self.fields[*a].offset));

        // Collect target field offsets (fields being replaced) for skip check
        let target_offsets: Vec<usize> = positions.iter().map(|&i| self.fields[i].offset).collect();

        for &field_idx in &positions {
            let start = self.fields[field_idx].offset;
            let end = start + self.fields[field_idx].length;
            self.text.replace_range(start..end, new_text);

            // Adjust offsets for all fields after this one (skip target fields)
            for other in &mut self.fields {
                if other.offset > start && !target_offsets.contains(&other.offset) {
                    other.offset = (other.offset as isize + len_diff) as usize;
                }
            }
            // Update this field's length
            self.fields[field_idx].length = new_len;
        }

        // Re-sort and fix current_field reference
        let target_offset = self.fields[self.current_field].offset;
        self.fields.sort_by(|a, b| {
            let a_key = if a.index == 0 { u32::MAX } else { a.index };
            let b_key = if b.index == 0 { u32::MAX } else { b.index };
            a_key
                .cmp(&b_key)
                .then(a.is_mirror.cmp(&b.is_mirror))
                .then(a.offset.cmp(&b.offset))
        });

        // Find current field again by index and primary status
        if let Some(pos) = self
            .fields
            .iter()
            .position(|f| f.index == target_index && !f.is_mirror)
        {
            self.current_field = pos;
        } else if let Some(pos) = self.fields.iter().position(|f| f.offset == target_offset) {
            self.current_field = pos;
        }

        self.current_range()
    }

    /// Check if the session is complete.
    pub fn is_complete(&self) -> bool {
        self.finished
    }
}

fn expand_parts(
    parts: &[SnippetPart],
    text: &mut String,
    fields: &mut Vec<SnippetField>,
    defaults: &HashMap<u32, String>,
) {
    for part in parts {
        match part {
            SnippetPart::Literal(s) => {
                text.push_str(s);
            }
            SnippetPart::TabStop { index } => {
                let start = text.len();
                // If we have a default from a placeholder with the same index, use it
                let default_text = defaults.get(index).map(|s| s.as_str()).unwrap_or("");
                text.push_str(default_text);
                fields.push(SnippetField {
                    index: *index,
                    offset: start,
                    length: default_text.len(),
                    is_mirror: false,
                });
            }
            SnippetPart::Placeholder { index, default } => {
                let start = text.len();
                expand_parts(default, text, fields, defaults);
                let length = text.len() - start;
                fields.push(SnippetField {
                    index: *index,
                    offset: start,
                    length,
                    is_mirror: false,
                });
            }
            SnippetPart::Choice { index, choices } => {
                let default = choices.first().map(|s| s.as_str()).unwrap_or("");
                let start = text.len();
                text.push_str(default);
                fields.push(SnippetField {
                    index: *index,
                    offset: start,
                    length: default.len(),
                    is_mirror: false,
                });
            }
        }
    }
}

/// Collect default text for each placeholder index (first placeholder wins).
fn collect_defaults(parts: &[SnippetPart]) -> HashMap<u32, String> {
    let mut defaults = HashMap::new();
    for part in parts {
        if let SnippetPart::Placeholder { index, default } = part {
            if !defaults.contains_key(index) {
                let mut text = String::new();
                let empty = HashMap::new();
                let mut fields = Vec::new();
                expand_parts(default, &mut text, &mut fields, &empty);
                defaults.insert(*index, text);
            }
        }
    }
    defaults
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_simple_tabstops() {
        let session = SnippetSession::expand("hello $1 world $0").unwrap();
        assert_eq!(session.text, "hello  world ");
        assert!(!session.finished);
        assert_eq!(session.fields.len(), 2);
        // First field is $1 (index=1), second is $0 (index=0, sorted last)
        assert_eq!(session.fields[0].index, 1);
        assert_eq!(session.fields[1].index, 0);
    }

    #[test]
    fn expand_placeholders() {
        let session = SnippetSession::expand("fn ${1:name}(${2:args})").unwrap();
        assert_eq!(session.text, "fn name(args)");
        assert_eq!(session.fields.len(), 2);
        let f1 = &session.fields[0];
        assert_eq!(f1.index, 1);
        assert_eq!(f1.offset, 3);
        assert_eq!(f1.length, 4); // "name"
        let f2 = &session.fields[1];
        assert_eq!(f2.index, 2);
        assert_eq!(f2.offset, 8);
        assert_eq!(f2.length, 4); // "args"
    }

    #[test]
    fn expand_choice() {
        let session = SnippetSession::expand("${1|pub,pub(crate),fn|}").unwrap();
        assert_eq!(session.text, "pub");
        assert_eq!(session.fields[0].length, 3);
    }

    #[test]
    fn navigate_fields() {
        let mut session = SnippetSession::expand("$1 $2 $0").unwrap();
        // Start at $1
        assert_eq!(session.current_range(), Some((0, 0)));
        // Next → $2
        let r = session.next_field();
        assert_eq!(r, Some((1, 0)));
        // Next → $0 (final)
        let r = session.next_field();
        assert!(r.is_some());
        assert!(session.is_complete());
    }

    #[test]
    fn navigate_prev() {
        let mut session = SnippetSession::expand("${1:a} ${2:b} $0").unwrap();
        session.next_field(); // → $2
        let r = session.prev_field();
        assert!(r.is_some());
        assert_eq!(session.fields[session.current_field].index, 1);
    }

    #[test]
    fn update_field_basic() {
        let mut session = SnippetSession::expand("fn ${1:name}() {}").unwrap();
        assert_eq!(session.text, "fn name() {}");
        session.update_field("greet");
        assert_eq!(session.text, "fn greet() {}");
        assert_eq!(session.fields[0].length, 5);
    }

    #[test]
    fn update_mirrors() {
        let mut session = SnippetSession::expand("${1:x} + $1").unwrap();
        assert_eq!(session.text, "x + x");
        session.update_field("val");
        assert_eq!(session.text, "val + val");
    }

    #[test]
    fn literal_only_is_finished() {
        let session = SnippetSession::expand("just text").unwrap();
        assert!(session.is_complete());
        assert_eq!(session.text, "just text");
    }

    #[test]
    fn real_world_function_snippet() {
        let mut session =
            SnippetSession::expand("fn ${1:name}(${2:params}) -> ${3:()}\n{\n\t$0\n}").unwrap();
        assert_eq!(session.text, "fn name(params) -> ()\n{\n\t\n}");

        // Navigate through all fields
        assert_eq!(session.fields[session.current_field].index, 1);
        session.update_field("main");
        assert_eq!(session.text, "fn main(params) -> ()\n{\n\t\n}");

        session.next_field();
        assert_eq!(session.fields[session.current_field].index, 2);
        session.update_field("");
        assert_eq!(session.text, "fn main() -> ()\n{\n\t\n}");

        session.next_field();
        assert_eq!(session.fields[session.current_field].index, 3);
        session.update_field("i32");
        assert_eq!(session.text, "fn main() -> i32\n{\n\t\n}");

        session.next_field(); // → $0
        assert!(session.is_complete());
    }

    #[test]
    fn empty_template() {
        let session = SnippetSession::expand("").unwrap();
        assert!(session.is_complete());
        assert_eq!(session.text, "");
    }
}
