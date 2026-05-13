//! Markdown/GFM parser — produces the same OrgElement AST used by org-mode.
//!
//! This enables bidirectional conversion: Markdown → OrgElement → Org text.

use super::{ListItem, OrgElement, OrgMeta};

/// Parse a Markdown document into metadata and a flat list of elements.
///
/// Handles: YAML frontmatter, ATX headings, fenced code blocks, blockquotes,
/// unordered/ordered lists, GFM tables, horizontal rules, and paragraphs.
pub fn parse_markdown_document(source: &str) -> (OrgMeta, Vec<OrgElement>) {
    let mut meta = OrgMeta::default();
    let mut elements = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;

    // YAML frontmatter
    if i < lines.len() && lines[i].trim() == "---" {
        i += 1;
        while i < lines.len() && lines[i].trim() != "---" {
            let line = lines[i].trim();
            if let Some((key, val)) = line.split_once(':') {
                let key = key.trim();
                let val = val.trim().trim_matches('"');
                match key {
                    "title" => meta.title = Some(val.to_string()),
                    "author" => meta.author = Some(val.to_string()),
                    "date" => meta.date = Some(val.to_string()),
                    "language" | "lang" => meta.language = Some(val.to_string()),
                    _ => {}
                }
            }
            i += 1;
        }
        if i < lines.len() {
            i += 1; // skip closing ---
        }
    }

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Blank lines
        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // ATX headings
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|&c| c == '#').count();
            if level <= 6 {
                let rest = trimmed[level..].trim();
                // Strip trailing # (closing ATX)
                let rest = rest.trim_end_matches('#').trim();
                let (title, tags) = parse_md_heading_tags(rest);
                elements.push(OrgElement::Heading {
                    level: level as u8,
                    title,
                    tags,
                    todo: None,
                    children: Vec::new(),
                });
                i += 1;
                continue;
            }
        }

        // Fenced code blocks
        if let Some(rest) = trimmed.strip_prefix("```") {
            let lang = rest.trim().to_string();
            let mut body_lines = Vec::new();
            i += 1;
            while i < lines.len() {
                if lines[i].trim().starts_with("```") {
                    break;
                }
                body_lines.push(lines[i]);
                i += 1;
            }
            elements.push(OrgElement::SrcBlock {
                language: lang,
                body: body_lines.join("\n"),
                exports: mae_babel::ExportsType::Code,
            });
            if i < lines.len() {
                i += 1; // skip closing ```
            }
            continue;
        }

        // Horizontal rules
        if is_horizontal_rule(trimmed) {
            elements.push(OrgElement::HorizontalRule);
            i += 1;
            continue;
        }

        // Blockquotes
        if trimmed.starts_with("> ") || trimmed == ">" {
            let mut quote_lines = Vec::new();
            while i < lines.len() {
                let ql = lines[i].trim();
                if let Some(content) = ql.strip_prefix("> ") {
                    quote_lines.push(content.to_string());
                } else if ql == ">" {
                    quote_lines.push(String::new());
                } else {
                    break;
                }
                i += 1;
            }
            elements.push(OrgElement::Quote(quote_lines.join("\n")));
            continue;
        }

        // GFM tables
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let mut rows = Vec::new();
            let mut has_header = false;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                let row_line = lines[i].trim();
                if row_line.contains("---") && row_line.starts_with('|') {
                    // Separator row
                    has_header = true;
                } else {
                    let cells: Vec<String> = row_line
                        .trim_matches('|')
                        .split('|')
                        .map(|c| c.trim().to_string())
                        .collect();
                    rows.push(cells);
                }
                i += 1;
            }
            elements.push(OrgElement::Table { rows, has_header });
            continue;
        }

        // Unordered lists
        if (trimmed.starts_with("- ") || trimmed.starts_with("* ")) && !is_horizontal_rule(trimmed)
        {
            let mut items = Vec::new();
            while i < lines.len() {
                let ll = lines[i].trim();
                if ll.starts_with("- ") || ll.starts_with("* ") {
                    items.push(ListItem {
                        content: ll[2..].to_string(),
                        children: Vec::new(),
                    });
                    i += 1;
                } else if ll.is_empty() {
                    break;
                } else if ll.starts_with("  ") {
                    // Continuation line
                    if let Some(last) = items.last_mut() {
                        last.content.push(' ');
                        last.content.push_str(ll.trim());
                    }
                    i += 1;
                } else {
                    break;
                }
            }
            elements.push(OrgElement::List {
                ordered: false,
                items,
            });
            continue;
        }

        // Ordered lists
        if trimmed.len() > 2 && trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            if let Some(rest) = trimmed
                .strip_prefix(|c: char| c.is_ascii_digit())
                .and_then(|s| s.strip_prefix(". "))
            {
                let mut items = vec![ListItem {
                    content: rest.to_string(),
                    children: Vec::new(),
                }];
                i += 1;
                while i < lines.len() {
                    let ll = lines[i].trim();
                    if let Some(item_rest) = ll
                        .strip_prefix(|c: char| c.is_ascii_digit())
                        .and_then(|s| s.strip_prefix(". "))
                    {
                        items.push(ListItem {
                            content: item_rest.to_string(),
                            children: Vec::new(),
                        });
                        i += 1;
                    } else {
                        break;
                    }
                }
                elements.push(OrgElement::List {
                    ordered: true,
                    items,
                });
                continue;
            }
        }

        // HTML comments (used for tags in MD→Org roundtrip)
        if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
            // Skip standalone comment lines (tag annotations are attached to headings)
            i += 1;
            continue;
        }

        // Paragraph: collect consecutive non-blank, non-special lines
        let mut para_lines = vec![line.to_string()];
        i += 1;
        while i < lines.len() {
            let pl = lines[i].trim();
            if pl.is_empty()
                || pl.starts_with('#')
                || pl.starts_with("```")
                || pl.starts_with('|')
                || pl.starts_with("> ")
                || pl.starts_with("- ")
                || pl.starts_with("* ")
                || is_horizontal_rule(pl)
            {
                break;
            }
            // Check for ordered list start
            if pl.chars().next().is_some_and(|c| c.is_ascii_digit()) && pl.contains(". ") {
                break;
            }
            para_lines.push(lines[i].to_string());
            i += 1;
        }
        elements.push(OrgElement::Paragraph(para_lines.join("\n")));
    }

    (meta, elements)
}

/// Check if a line is a horizontal rule (---, ***, ___).
fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    let first = chars[0];
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    chars.iter().all(|&c| c == first || c == ' ')
}

/// Parse HTML comment tags from heading text: `Title  <!-- tag1, tag2 -->`
fn parse_md_heading_tags(text: &str) -> (String, Vec<String>) {
    if let Some(comment_start) = text.find("<!--") {
        if let Some(comment_end) = text[comment_start..].find("-->") {
            let tag_str = &text[comment_start + 4..comment_start + comment_end];
            let tags: Vec<String> = tag_str
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            if !tags.is_empty() {
                let title = text[..comment_start].trim().to_string();
                return (title, tags);
            }
        }
    }
    (text.to_string(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter() {
        let src =
            "---\ntitle: \"My Doc\"\nauthor: \"Test\"\ndate: \"2026-01-01\"\n---\n\nContent\n";
        let (meta, elements) = parse_markdown_document(src);
        assert_eq!(meta.title.as_deref(), Some("My Doc"));
        assert_eq!(meta.author.as_deref(), Some("Test"));
        assert_eq!(meta.date.as_deref(), Some("2026-01-01"));
        assert_eq!(elements.len(), 1);
    }

    #[test]
    fn parse_headings() {
        let src = "# Heading 1\n## Heading 2\n### Heading 3\n";
        let (_, elements) = parse_markdown_document(src);
        assert_eq!(elements.len(), 3);
        if let OrgElement::Heading { level, title, .. } = &elements[0] {
            assert_eq!(*level, 1);
            assert_eq!(title, "Heading 1");
        }
        if let OrgElement::Heading { level, .. } = &elements[2] {
            assert_eq!(*level, 3);
        }
    }

    #[test]
    fn parse_heading_with_tags() {
        let src = "# My Heading  <!-- tag1, tag2 -->\n";
        let (_, elements) = parse_markdown_document(src);
        if let OrgElement::Heading { title, tags, .. } = &elements[0] {
            assert_eq!(title, "My Heading");
            assert_eq!(tags, &["tag1".to_string(), "tag2".to_string()]);
        }
    }

    #[test]
    fn parse_fenced_code_block() {
        let src = "```python\nprint(1)\n```\n";
        let (_, elements) = parse_markdown_document(src);
        assert_eq!(elements.len(), 1);
        if let OrgElement::SrcBlock { language, body, .. } = &elements[0] {
            assert_eq!(language, "python");
            assert_eq!(body, "print(1)");
        }
    }

    #[test]
    fn parse_table() {
        let src = "| a | b |\n| --- | --- |\n| 1 | 2 |\n";
        let (_, elements) = parse_markdown_document(src);
        assert_eq!(elements.len(), 1);
        if let OrgElement::Table { rows, has_header } = &elements[0] {
            assert_eq!(rows.len(), 2);
            assert!(*has_header);
        }
    }

    #[test]
    fn parse_unordered_list() {
        let src = "- item 1\n- item 2\n- item 3\n";
        let (_, elements) = parse_markdown_document(src);
        assert_eq!(elements.len(), 1);
        if let OrgElement::List { ordered, items } = &elements[0] {
            assert!(!ordered);
            assert_eq!(items.len(), 3);
        }
    }

    #[test]
    fn parse_ordered_list() {
        let src = "1. first\n2. second\n3. third\n";
        let (_, elements) = parse_markdown_document(src);
        assert_eq!(elements.len(), 1);
        if let OrgElement::List { ordered, items } = &elements[0] {
            assert!(ordered);
            assert_eq!(items.len(), 3);
        }
    }

    #[test]
    fn parse_blockquote() {
        let src = "> line one\n> line two\n";
        let (_, elements) = parse_markdown_document(src);
        assert_eq!(elements.len(), 1);
        if let OrgElement::Quote(text) = &elements[0] {
            assert!(text.contains("line one"));
            assert!(text.contains("line two"));
        }
    }

    #[test]
    fn parse_horizontal_rule() {
        let src = "above\n\n---\n\nbelow\n";
        let (_, elements) = parse_markdown_document(src);
        assert!(elements
            .iter()
            .any(|e| matches!(e, OrgElement::HorizontalRule)));
    }

    #[test]
    fn parse_paragraph() {
        let src = "Hello world\nmore text\n";
        let (_, elements) = parse_markdown_document(src);
        assert_eq!(elements.len(), 1);
        if let OrgElement::Paragraph(text) = &elements[0] {
            assert!(text.contains("Hello world"));
            assert!(text.contains("more text"));
        }
    }
}
