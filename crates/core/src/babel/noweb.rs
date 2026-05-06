//! Noweb reference expansion for babel source blocks.

use std::collections::HashSet;

use super::SrcBlock;

/// Expand `<<block-name>>` references in a block body.
/// Returns the expanded body or an error on cycle detection.
pub fn expand_noweb(body: &str, all_blocks: &[SrcBlock]) -> Result<String, String> {
    let mut visited = HashSet::new();
    expand_noweb_inner(body, all_blocks, &mut visited, 0)
}

fn expand_noweb_inner(
    body: &str,
    all_blocks: &[SrcBlock],
    visited: &mut HashSet<String>,
    depth: usize,
) -> Result<String, String> {
    if depth > 100 {
        return Err("Noweb expansion depth limit exceeded (possible cycle)".to_string());
    }

    let mut result = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    let mut line_indent = String::new();
    let mut at_line_start = true;

    while let Some(ch) = chars.next() {
        if at_line_start && (ch == ' ' || ch == '\t') {
            line_indent.push(ch);
            result.push(ch);
            continue;
        }
        at_line_start = false;

        if ch == '<' && chars.peek() == Some(&'<') {
            chars.next(); // consume second '<'

            // Read reference name until ">>"
            let mut ref_name = String::new();
            let mut found_close = false;
            while let Some(c) = chars.next() {
                if c == '>' && chars.peek() == Some(&'>') {
                    chars.next(); // consume second '>'
                    found_close = true;
                    break;
                }
                ref_name.push(c);
            }

            if !found_close {
                // Not a valid noweb ref, output as-is
                result.push_str("<<");
                result.push_str(&ref_name);
                continue;
            }

            let ref_name = ref_name.trim().to_string();

            if visited.contains(&ref_name) {
                return Err(format!("Cyclic noweb reference detected: <<{}>>", ref_name));
            }

            // Find the referenced block
            let referenced = all_blocks
                .iter()
                .find(|b| b.name.as_deref() == Some(ref_name.as_str()));

            match referenced {
                Some(block) => {
                    visited.insert(ref_name.clone());
                    let expanded = expand_noweb_inner(&block.body, all_blocks, visited, depth + 1)?;
                    visited.remove(&ref_name);

                    // Apply indentation to each line of the expansion
                    let indent = &line_indent;
                    for (i, line) in expanded.lines().enumerate() {
                        if i > 0 {
                            result.push('\n');
                            result.push_str(indent);
                        }
                        result.push_str(line);
                    }
                }
                None => {
                    return Err(format!("Noweb reference not found: <<{}>>", ref_name));
                }
            }
        } else {
            result.push(ch);
            if ch == '\n' {
                at_line_start = true;
                line_indent.clear();
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::babel::HeaderArgs;

    fn make_block(name: &str, body: &str) -> SrcBlock {
        SrcBlock {
            name: Some(name.to_string()),
            language: "python".to_string(),
            header_args: HeaderArgs::default(),
            body: body.to_string(),
            line_range: (0, 2),
            body_byte_range: (0, body.len()),
        }
    }

    #[test]
    fn simple_expansion() {
        let blocks = vec![make_block("greet", "print(\"hello\")")];
        let body = "<<greet>>\nprint(\"done\")";
        let result = expand_noweb(body, &blocks).unwrap();
        assert_eq!(result, "print(\"hello\")\nprint(\"done\")");
    }

    #[test]
    fn recursive_expansion() {
        let blocks = vec![
            make_block("inner", "x = 1"),
            make_block("outer", "<<inner>>\ny = 2"),
        ];
        let body = "<<outer>>";
        let result = expand_noweb(body, &blocks).unwrap();
        assert_eq!(result, "x = 1\ny = 2");
    }

    #[test]
    fn cycle_detection() {
        let blocks = vec![make_block("a", "<<b>>"), make_block("b", "<<a>>")];
        let body = "<<a>>";
        let result = expand_noweb(body, &blocks);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cyclic"));
    }

    #[test]
    fn missing_reference() {
        let blocks = vec![];
        let body = "<<nonexistent>>";
        let result = expand_noweb(body, &blocks);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn indentation_preserved() {
        let blocks = vec![make_block("body", "line1\nline2")];
        let body = "def f():\n    <<body>>";
        let result = expand_noweb(body, &blocks).unwrap();
        assert_eq!(result, "def f():\n    line1\n    line2");
    }

    #[test]
    fn no_refs_passthrough() {
        let blocks = vec![];
        let body = "just plain text\nno refs here";
        let result = expand_noweb(body, &blocks).unwrap();
        assert_eq!(result, body);
    }
}
