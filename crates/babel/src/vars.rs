//! Variable resolution for babel source blocks.

use super::results::read_results_content;
use super::{find_results_block, SrcBlock, VarSource};

/// Resolve all `:var` bindings for a block.
/// Returns `(name, resolved_value)` pairs ready for injection.
pub fn resolve_vars(
    block: &SrcBlock,
    all_blocks: &[SrcBlock],
    buf_text: &str,
) -> Vec<(String, String)> {
    let mut resolved = Vec::new();

    for (name, source) in &block.header_args.var {
        let value = match source {
            VarSource::Literal(v) => v.clone(),
            VarSource::BlockRef(ref_name) => resolve_block_ref(ref_name, all_blocks, buf_text),
            VarSource::TableRef(ref_name) => resolve_table_ref(ref_name, buf_text),
        };
        resolved.push((name.clone(), value));
    }

    resolved
}

/// Resolve a block reference by reading its cached `#+RESULTS:` block.
fn resolve_block_ref(name: &str, all_blocks: &[SrcBlock], source: &str) -> String {
    match all_blocks.iter().find(|b| b.name.as_deref() == Some(name)) {
        Some(block) => {
            // Look for cached #+RESULTS: after the block
            match find_results_block(source, block.line_range.1 + 1) {
                Some((start, end)) => {
                    let content = read_results_content(source, start, end);
                    if content.is_empty() {
                        format!("<no-results:{}>", name)
                    } else {
                        content
                    }
                }
                None => format!("<no-results:{}>", name),
            }
        }
        None => format!("<unresolved:{}>", name),
    }
}

/// Resolve a table reference by parsing the named org table from buffer text.
fn resolve_table_ref(name: &str, buf_text: &str) -> String {
    // Find #+name: <name> followed by a table
    let lines: Vec<&str> = buf_text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(n) = trimmed
            .strip_prefix("#+name:")
            .or_else(|| trimmed.strip_prefix("#+NAME:"))
        {
            if n.trim() == name {
                // Next lines should be table rows
                let mut rows = Vec::new();
                for line in lines.iter().skip(i + 1) {
                    let tl = line.trim();
                    if tl.starts_with('|') {
                        if !tl.starts_with("|-") {
                            let cells: Vec<&str> =
                                tl.trim_matches('|').split('|').map(|c| c.trim()).collect();
                            rows.push(cells.join("\t"));
                        }
                    } else {
                        break;
                    }
                }
                return rows.join("\n");
            }
        }
    }
    format!("<table-not-found:{}>", name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeaderArgs;

    #[test]
    fn resolve_literal_vars() {
        let mut args = HeaderArgs::default();
        args.var
            .push(("x".to_string(), VarSource::Literal("42".to_string())));
        args.var
            .push(("name".to_string(), VarSource::Literal("test".to_string())));

        let block = SrcBlock {
            name: None,
            language: "python".to_string(),
            header_args: args,
            body: String::new(),
            line_range: (0, 0),
            body_byte_range: (0, 0),
        };

        let resolved = resolve_vars(&block, &[], "");
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0], ("x".to_string(), "42".to_string()));
        assert_eq!(resolved[1], ("name".to_string(), "test".to_string()));
    }

    #[test]
    fn resolve_table_ref_from_buffer() {
        let buf = "#+name: data\n| a | b |\n|---+---|\n| 1 | 2 |\n| 3 | 4 |\n";
        let mut args = HeaderArgs::default();
        args.var
            .push(("tbl".to_string(), VarSource::TableRef("data".to_string())));

        let block = SrcBlock {
            name: None,
            language: "python".to_string(),
            header_args: args,
            body: String::new(),
            line_range: (0, 0),
            body_byte_range: (0, 0),
        };

        let resolved = resolve_vars(&block, &[], buf);
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].1.contains("a\tb"));
        assert!(resolved[0].1.contains("1\t2"));
    }

    #[test]
    fn resolve_missing_table() {
        let result = resolve_table_ref("nonexistent", "no tables here");
        assert!(result.contains("table-not-found"));
    }

    #[test]
    fn resolve_block_ref_with_cached_results() {
        let src = "#+name: compute\n#+begin_src python\nprint(42)\n#+end_src\n\n#+RESULTS: compute\n: 42\n";
        let blocks = crate::parse_src_blocks(src);
        let result = resolve_block_ref("compute", &blocks, src);
        assert_eq!(result, "42");
    }

    #[test]
    fn resolve_block_ref_drawer_results() {
        let src = "#+name: data\n#+begin_src python\nprint('hello')\n#+end_src\n\n#+RESULTS: data\n:RESULTS:\nhello\n:END:\n";
        let blocks = crate::parse_src_blocks(src);
        let result = resolve_block_ref("data", &blocks, src);
        assert_eq!(result, "hello");
    }

    #[test]
    fn resolve_block_ref_no_results() {
        let src = "#+name: norun\n#+begin_src python\nprint(1)\n#+end_src\n";
        let blocks = crate::parse_src_blocks(src);
        let result = resolve_block_ref("norun", &blocks, src);
        assert!(result.contains("no-results"));
    }

    #[test]
    fn resolve_block_ref_unresolved() {
        let result = resolve_block_ref("nonexistent", &[], "");
        assert!(result.contains("unresolved"));
    }
}
