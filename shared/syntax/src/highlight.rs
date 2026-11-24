//! One text's highlighted regions.

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::HIGHLIGHT_NAMES;

/// A highlighted byte range of the text, named by its innermost capture.
/// Text outside every region has no capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
    /// One of [`HIGHLIGHT_NAMES`].
    pub name: &'static str,
}

/// The regions of `source` under `config`, in order; `None` when
/// tree-sitter-highlight refuses to start (it does not, for a configuration
/// [`crate::build_configuration`] built). Injections are not resolved: an
/// embedded language reads as its host's text.
pub fn highlight(config: &HighlightConfiguration, source: &str) -> Option<Vec<Region>> {
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config, source.as_bytes(), None, |_| None)
        .ok()?;
    let mut regions = Vec::new();
    // Stack of active highlight names — needed because regions can nest.
    let mut stack: Vec<&'static str> = Vec::new();
    for event in events {
        let Ok(event) = event else { continue };
        match event {
            HighlightEvent::HighlightStart(h) => {
                stack.push(HIGHLIGHT_NAMES.get(h.0).copied().unwrap_or("variable"));
            }
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if let Some(&name) = stack.last() {
                    regions.push(Region { start, end, name });
                }
            }
        }
    }
    Some(regions)
}
