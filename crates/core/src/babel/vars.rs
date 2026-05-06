//! Variable resolution for babel source blocks.

use super::{SrcBlock, VarSource};

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
            VarSource::BlockRef(ref_name) => resolve_block_ref(ref_name, all_blocks),
            VarSource::TableRef(ref_name) => resolve_table_ref(ref_name, buf_text),
        };
        resolved.push((name.clone(), value));
    }

    resolved
}

/// Resolve a block reference by finding the named block's last results.
fn resolve_block_ref(name: &str, all_blocks: &[SrcBlock]) -> String {
    // Look for the named block — in a real implementation we'd execute it
    // or read its cached #+RESULTS:. For now, return a placeholder.
    if let Some(_block) = all_blocks.iter().find(|b| b.name.as_deref() == Some(name)) {
        format!("<block-ref:{}>", name)
    } else {
        format!("<unresolved:{}>", name)
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
    use crate::babel::HeaderArgs;

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
}
