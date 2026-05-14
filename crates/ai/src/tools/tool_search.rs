//! Semantic tool search — fuzzy search over tool names and descriptions.
//!
//! With 146+ tools, exact name recall is impractical. This module provides
//! word-level fuzzy matching so the AI agent can search by intent
//! (e.g. "set breakpoint" → `dap_set_breakpoint`).

use crate::types::ToolDefinition;

/// A tool search result with relevance score.
#[derive(Debug, Clone)]
pub struct ToolSearchResult {
    pub name: String,
    pub description: String,
    pub score: i64,
}

/// Search tools by a natural-language query.
///
/// Tokenizes the query into words, scores each word against tool name
/// (higher weight) and description (lower weight). Requires ≥50% of
/// query words to match. Returns results sorted by score descending.
pub fn search_tools(tools: &[ToolDefinition], query: &str, limit: usize) -> Vec<ToolSearchResult> {
    let query_words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| w.len() >= 2)
        .collect();

    if query_words.is_empty() {
        return Vec::new();
    }

    let min_matches = query_words.len().div_ceil(2);
    let mut results: Vec<ToolSearchResult> = Vec::new();

    for tool in tools {
        let name_lower = tool.name.to_ascii_lowercase();
        // Split tool name on underscores for word-level matching
        let name_words: Vec<&str> = name_lower.split('_').collect();
        let desc_lower = tool.description.to_ascii_lowercase();

        let mut total_score: i64 = 0;
        let mut matched_words = 0usize;

        for qw in &query_words {
            let mut word_score: i64 = 0;

            // Name matching (higher weight)
            if name_lower.contains(qw.as_str()) {
                // Exact word match in name parts
                if name_words.contains(&qw.as_str()) {
                    word_score += 100;
                } else {
                    word_score += 80; // substring match in name
                }
            }

            // Description matching (lower weight)
            if desc_lower.contains(qw.as_str()) {
                if word_score > 0 {
                    word_score += 30; // bonus for matching both
                } else {
                    word_score += 50; // description-only match
                }
            }

            // Fuzzy subsequence match on name (lowest tier)
            if word_score == 0 && is_subsequence(qw, &name_lower) {
                word_score += 20;
            }

            if word_score > 0 {
                matched_words += 1;
                total_score += word_score;
            }
        }

        if matched_words < min_matches {
            continue;
        }

        // Bonus for matching ALL query words
        if matched_words == query_words.len() {
            total_score += 50;
        }

        // Small penalty for very long names (prefer specific tools)
        total_score -= (tool.name.len() as i64) / 10;

        results.push(ToolSearchResult {
            name: tool.name.clone(),
            description: tool.description.clone(),
            score: total_score,
        });
    }

    results.sort_by_key(|r| std::cmp::Reverse(r.score));
    results.truncate(limit);
    results
}

/// Check if `needle` is a subsequence of `haystack`.
fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut it = haystack.chars();
    for nc in needle.chars() {
        loop {
            match it.next() {
                Some(hc) if hc == nc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_tool(name: &str, desc: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: desc.into(),
            parameters: crate::types::ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: None,
        }
    }

    fn test_tools() -> Vec<ToolDefinition> {
        vec![
            make_tool("dap_set_breakpoint", "Set a breakpoint at a file and line"),
            make_tool("dap_start", "Start a debug session"),
            make_tool("dap_continue", "Continue execution until next breakpoint"),
            make_tool("lsp_references", "Find all references to symbol at point"),
            make_tool("lsp_definition", "Go to definition of symbol at point"),
            make_tool("buffer_read", "Read buffer contents"),
            make_tool("buffer_write", "Write text to buffer at position"),
            make_tool("project_search", "Search project files with ripgrep"),
            make_tool("open_file", "Open a file in the editor"),
            make_tool("kb_search", "Full-text search across knowledge base nodes"),
        ]
    }

    #[test]
    fn search_set_breakpoint() {
        let tools = test_tools();
        let results = search_tools(&tools, "set breakpoint", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "dap_set_breakpoint");
    }

    #[test]
    fn search_find_references() {
        let tools = test_tools();
        let results = search_tools(&tools, "find references", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "lsp_references");
    }

    #[test]
    fn search_debug() {
        let tools = test_tools();
        let results = search_tools(&tools, "debug", 10);
        assert!(!results.is_empty());
        // Should return dap tools that mention debug
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"dap_start"));
    }

    #[test]
    fn search_partial_word() {
        let tools = test_tools();
        let results = search_tools(&tools, "break", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "dap_set_breakpoint");
    }

    #[test]
    fn search_empty_query() {
        let tools = test_tools();
        let results = search_tools(&tools, "", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn search_no_match() {
        let tools = test_tools();
        let results = search_tools(&tools, "xyzzyplugh", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn search_respects_limit() {
        let tools = test_tools();
        let results = search_tools(&tools, "buffer", 1);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn subsequence_match() {
        assert!(is_subsequence("sb", "set_breakpoint"));
        assert!(is_subsequence("bp", "breakpoint"));
        assert!(!is_subsequence("zz", "breakpoint"));
    }
}
