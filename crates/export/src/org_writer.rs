//! Org-mode writer — renders OrgElement AST back to org-mode text.
//!
//! Used for Markdown → Org conversion (bidirectional with markdown.rs exporter).

use super::{Exporter, OrgElement, OrgMeta};

pub struct OrgWriter;

impl Exporter for OrgWriter {
    fn export(&self, meta: &OrgMeta, elements: &[OrgElement]) -> String {
        let mut out = String::with_capacity(4096);

        // Org keywords
        if let Some(ref title) = meta.title {
            out.push_str(&format!("#+title: {}\n", title));
        }
        if let Some(ref author) = meta.author {
            out.push_str(&format!("#+author: {}\n", author));
        }
        if let Some(ref date) = meta.date {
            out.push_str(&format!("#+date: {}\n", date));
        }
        if meta.title.is_some() || meta.author.is_some() || meta.date.is_some() {
            out.push('\n');
        }

        for element in elements {
            render_element(&mut out, element);
        }

        out
    }
}

fn render_element(out: &mut String, element: &OrgElement) {
    match element {
        OrgElement::Heading {
            level,
            title,
            tags,
            todo,
            ..
        } => {
            for _ in 0..*level {
                out.push('*');
            }
            out.push(' ');
            if let Some(kw) = todo {
                out.push_str(kw);
                out.push(' ');
            }
            out.push_str(&convert_md_inline_to_org(title));
            if !tags.is_empty() {
                out.push_str("  :");
                out.push_str(&tags.join(":"));
                out.push(':');
            }
            out.push('\n');
        }
        OrgElement::Paragraph(text) => {
            out.push_str(&convert_md_inline_to_org(text));
            out.push_str("\n\n");
        }
        OrgElement::SrcBlock { language, body, .. } => {
            out.push_str(&format!("#+begin_src {}\n", language));
            out.push_str(body);
            out.push_str("\n#+end_src\n\n");
        }
        OrgElement::ResultsBlock(content) => {
            out.push_str("#+RESULTS:\n");
            for line in content.lines() {
                out.push_str(&format!(": {}\n", line));
            }
            out.push('\n');
        }
        OrgElement::List { ordered, items } => {
            for (i, item) in items.iter().enumerate() {
                if *ordered {
                    out.push_str(&format!(
                        "{}. {}\n",
                        i + 1,
                        convert_md_inline_to_org(&item.content)
                    ));
                } else {
                    out.push_str(&format!("- {}\n", convert_md_inline_to_org(&item.content)));
                }
            }
            out.push('\n');
        }
        OrgElement::Table { rows, has_header } => {
            if rows.is_empty() {
                return;
            }
            let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
            for (i, row) in rows.iter().enumerate() {
                out.push('|');
                for j in 0..cols {
                    let cell = row.get(j).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&format!(" {} |", cell));
                }
                out.push('\n');
                if i == 0 && *has_header {
                    out.push('|');
                    for _ in 0..cols {
                        out.push_str("---+");
                    }
                    // Fix last separator
                    let len = out.len();
                    if out.ends_with('+') {
                        out.truncate(len - 1);
                        out.push('|');
                    }
                    out.push('\n');
                }
            }
            out.push('\n');
        }
        OrgElement::Quote(text) => {
            out.push_str("#+begin_quote\n");
            out.push_str(text);
            out.push_str("\n#+end_quote\n\n");
        }
        OrgElement::HorizontalRule => {
            out.push_str("-----\n\n");
        }
        OrgElement::Comment(text) => {
            out.push_str(&format!("# {}\n", text));
        }
        OrgElement::ExportBlock { format, content } => {
            out.push_str(&format!("#+begin_export {}\n", format));
            out.push_str(content);
            out.push_str("\n#+end_export\n\n");
        }
    }
}

/// Convert Markdown inline markup to Org inline markup.
///
/// Handles: **bold** → *bold*, *italic* → /italic/, `code` → ~code~,
/// ~~strike~~ → +strike+, [label](url) → [[url][label]]
pub fn convert_md_inline_to_org(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // **bold**
        if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            if let Some(end) = find_double_marker(bytes, i + 2, b'*') {
                result.push('*');
                result.push_str(&convert_md_inline_to_org(&text[i + 2..end]));
                result.push('*');
                i = end + 2;
                continue;
            }
        }

        // *italic* (but not **)
        if bytes[i] == b'*' && (i + 1 >= bytes.len() || bytes[i + 1] != b'*') {
            if let Some(end) = find_single_marker(bytes, i + 1, b'*') {
                result.push('/');
                result.push_str(&convert_md_inline_to_org(&text[i + 1..end]));
                result.push('/');
                i = end + 1;
                continue;
            }
        }

        // ~~strikethrough~~
        if i + 1 < bytes.len() && bytes[i] == b'~' && bytes[i + 1] == b'~' {
            if let Some(end) = find_double_marker(bytes, i + 2, b'~') {
                result.push('+');
                result.push_str(&convert_md_inline_to_org(&text[i + 2..end]));
                result.push('+');
                i = end + 2;
                continue;
            }
        }

        // `code`
        if bytes[i] == b'`' && (i + 1 >= bytes.len() || bytes[i + 1] != b'`') {
            if let Some(end) = find_single_marker(bytes, i + 1, b'`') {
                result.push('~');
                result.push_str(&text[i + 1..end]);
                result.push('~');
                i = end + 1;
                continue;
            }
        }

        // [label](url)
        if bytes[i] == b'[' {
            if let Some((label_end, url, link_end)) = parse_md_link(text, i) {
                result.push_str(&format!("[[{}][{}]]", url, &text[i + 1..label_end]));
                i = link_end;
                continue;
            }
        }

        result.push(bytes[i] as char);
        i += 1;
    }

    result
}

fn find_double_marker(bytes: &[u8], start: usize, marker: u8) -> Option<usize> {
    let mut j = start;
    while j + 1 < bytes.len() {
        if bytes[j] == marker && bytes[j + 1] == marker {
            return Some(j);
        }
        j += 1;
    }
    None
}

fn find_single_marker(bytes: &[u8], start: usize, marker: u8) -> Option<usize> {
    (start..bytes.len()).find(|&j| bytes[j] == marker)
}

/// Parse `[label](url)` at position `start` (the `[`).
/// Returns `(label_end, url_str, total_end)`.
fn parse_md_link(text: &str, start: usize) -> Option<(usize, &str, usize)> {
    let rest = &text[start..];
    let close_bracket = rest.find(']')?;
    let after_bracket = start + close_bracket + 1;
    if after_bracket >= text.len() || text.as_bytes()[after_bracket] != b'(' {
        return None;
    }
    let paren_start = after_bracket + 1;
    let close_paren = text[paren_start..].find(')')?;
    let url = &text[paren_start..paren_start + close_paren];
    Some((start + close_bracket, url, paren_start + close_paren + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ListItem, OrgMeta};

    #[test]
    fn write_heading() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::Heading {
            level: 2,
            title: "Test".to_string(),
            tags: vec!["tag1".to_string()],
            todo: Some("TODO".to_string()),
            children: vec![],
        }];
        let writer = OrgWriter;
        let org = writer.export(&meta, &elements);
        assert!(org.contains("** TODO Test  :tag1:"));
    }

    #[test]
    fn write_paragraph() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::Paragraph("Hello world".to_string())];
        let writer = OrgWriter;
        let org = writer.export(&meta, &elements);
        assert!(org.contains("Hello world"));
    }

    #[test]
    fn write_src_block() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::SrcBlock {
            language: "python".to_string(),
            body: "print(1)".to_string(),
            exports: mae_babel::ExportsType::Code,
        }];
        let writer = OrgWriter;
        let org = writer.export(&meta, &elements);
        assert!(org.contains("#+begin_src python\nprint(1)\n#+end_src"));
    }

    #[test]
    fn write_list() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::List {
            ordered: false,
            items: vec![
                ListItem {
                    content: "one".to_string(),
                    children: vec![],
                },
                ListItem {
                    content: "two".to_string(),
                    children: vec![],
                },
            ],
        }];
        let writer = OrgWriter;
        let org = writer.export(&meta, &elements);
        assert!(org.contains("- one\n- two"));
    }

    #[test]
    fn write_table() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::Table {
            rows: vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ],
            has_header: true,
        }];
        let writer = OrgWriter;
        let org = writer.export(&meta, &elements);
        assert!(org.contains("| Name | Age |"));
        assert!(org.contains("| Alice | 30 |"));
    }

    #[test]
    fn write_quote() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::Quote("To be or not to be".to_string())];
        let writer = OrgWriter;
        let org = writer.export(&meta, &elements);
        assert!(org.contains("#+begin_quote\nTo be or not to be\n#+end_quote"));
    }

    #[test]
    fn write_meta() {
        let meta = OrgMeta {
            title: Some("My Doc".to_string()),
            author: Some("Test".to_string()),
            ..Default::default()
        };
        let writer = OrgWriter;
        let org = writer.export(&meta, &[]);
        assert!(org.contains("#+title: My Doc"));
        assert!(org.contains("#+author: Test"));
    }

    #[test]
    fn convert_md_bold_to_org() {
        assert_eq!(convert_md_inline_to_org("hello **world**"), "hello *world*");
    }

    #[test]
    fn convert_md_italic_to_org() {
        assert_eq!(convert_md_inline_to_org("hello *world*"), "hello /world/");
    }

    #[test]
    fn convert_md_code_to_org() {
        assert_eq!(convert_md_inline_to_org("hello `code`"), "hello ~code~");
    }

    #[test]
    fn convert_md_strike_to_org() {
        assert_eq!(convert_md_inline_to_org("hello ~~gone~~"), "hello +gone+");
    }

    #[test]
    fn convert_md_link_to_org() {
        assert_eq!(
            convert_md_inline_to_org("[Example](https://example.com)"),
            "[[https://example.com][Example]]"
        );
    }
}
