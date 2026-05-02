//! Tree-sitter fold range collection.

use tree_sitter::Node;

/// Recursively collect foldable node ranges from a tree-sitter parse tree.
pub(crate) fn collect_fold_nodes(
    node: Node,
    _source: &str,
    ranges: &mut Vec<(usize, usize)>,
    depth: usize,
) {
    const MAX_DEPTH: usize = 3;
    if depth > MAX_DEPTH {
        return;
    }

    let start_line = node.start_position().row;
    let end_line = node.end_position().row;

    // Only fold multi-line named nodes (skip anonymous tokens like punctuation).
    if node.is_named() && end_line > start_line + 1 && is_foldable_kind(node.kind()) {
        ranges.push((start_line, end_line));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_fold_nodes(child, _source, ranges, depth + 1);
    }
}

/// Check if a tree-sitter node kind represents a foldable code block.
pub(crate) fn is_foldable_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_definition"
            | "function_item"
            | "function_declaration"
            | "method_definition"
            | "method_declaration"
            | "struct_item"
            | "enum_item"
            | "impl_item"
            | "trait_item"
            | "class_definition"
            | "class_declaration"
            | "class_body"
            | "interface_declaration"
            | "if_expression"
            | "if_statement"
            | "match_expression"
            | "switch_statement"
            | "for_expression"
            | "for_statement"
            | "while_statement"
            | "loop_expression"
            | "block"
            | "mod_item"
            | "module"
            | "use_declaration"
            | "import_statement"
            | "macro_definition"
            | "const_item"
            | "static_item"
            | "type_alias"
    )
}
