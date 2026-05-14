//! Org export infrastructure — parse org documents and export to various formats.
//!
//! @stability: experimental
//! @since: 0.9.0

pub mod html;
pub mod markdown;
pub mod markdown_parser;
pub mod org_writer;

/// Document-level metadata extracted from org keywords.
#[derive(Debug, Clone, Default)]
pub struct OrgMeta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub date: Option<String>,
    pub language: Option<String>,
    pub options: ExportOptions,
    pub select_tags: Vec<String>,
    pub exclude_tags: Vec<String>,
}

/// Export options from `#+OPTIONS:` line.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub toc: bool,
    pub toc_depth: u8,
    pub headline_levels: u8,
    pub num: bool,
    pub author_p: bool,
    pub date_p: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            toc: true,
            toc_depth: 3,
            headline_levels: 6,
            num: true,
            author_p: true,
            date_p: true,
        }
    }
}

/// Parsed org document elements.
#[derive(Debug, Clone)]
pub enum OrgElement {
    Heading {
        level: u8,
        title: String,
        tags: Vec<String>,
        todo: Option<String>,
        children: Vec<OrgElement>,
    },
    Paragraph(String),
    SrcBlock {
        language: String,
        body: String,
        exports: mae_babel::ExportsType,
    },
    ResultsBlock(String),
    List {
        ordered: bool,
        items: Vec<ListItem>,
    },
    Table {
        rows: Vec<Vec<String>>,
        has_header: bool,
    },
    Quote(String),
    HorizontalRule,
    Comment(String),
    ExportBlock {
        format: String,
        content: String,
    },
}

/// A single list item with optional nesting.
#[derive(Debug, Clone)]
pub struct ListItem {
    pub content: String,
    pub children: Vec<ListItem>,
}

/// Trait for export backends.
pub trait Exporter {
    fn export(&self, meta: &OrgMeta, elements: &[OrgElement]) -> String;
}

/// Parse an org-mode document into metadata and a flat list of elements.
pub fn parse_org_document(source: &str) -> (OrgMeta, Vec<OrgElement>) {
    let mut meta = OrgMeta::default();
    let mut elements = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Keywords
        if let Some(rest) = strip_keyword(trimmed, "#+title:") {
            meta.title = Some(rest.to_string());
            i += 1;
            continue;
        }
        if let Some(rest) = strip_keyword(trimmed, "#+author:") {
            meta.author = Some(rest.to_string());
            i += 1;
            continue;
        }
        if let Some(rest) = strip_keyword(trimmed, "#+date:") {
            meta.date = Some(rest.to_string());
            i += 1;
            continue;
        }
        if let Some(rest) = strip_keyword(trimmed, "#+language:") {
            meta.language = Some(rest.to_string());
            i += 1;
            continue;
        }
        if let Some(rest) = strip_keyword(trimmed, "#+options:") {
            parse_options_line(rest, &mut meta.options);
            i += 1;
            continue;
        }
        if let Some(rest) = strip_keyword(trimmed, "#+export_select_tags:") {
            meta.select_tags = rest.split_whitespace().map(|s| s.to_string()).collect();
            i += 1;
            continue;
        }
        if let Some(rest) = strip_keyword(trimmed, "#+export_exclude_tags:") {
            meta.exclude_tags = rest.split_whitespace().map(|s| s.to_string()).collect();
            i += 1;
            continue;
        }

        // Skip other keywords
        if trimmed.starts_with("#+") && !trimmed.to_ascii_lowercase().starts_with("#+begin") {
            i += 1;
            continue;
        }

        // Headings
        if trimmed.starts_with("* ") || trimmed == "*" {
            let level = trimmed.chars().take_while(|&c| c == '*').count() as u8;
            let rest = trimmed[level as usize..].trim();
            let (title, tags) = parse_heading_tags(rest);
            let (todo, clean_title) = parse_heading_todo(&title);
            elements.push(OrgElement::Heading {
                level,
                title: clean_title,
                tags,
                todo,
                children: Vec::new(),
            });
            i += 1;
            continue;
        }

        // Multi-level headings
        if trimmed.starts_with("**") {
            let level = trimmed.chars().take_while(|&c| c == '*').count() as u8;
            let rest = trimmed[level as usize..].trim();
            let (title, tags) = parse_heading_tags(rest);
            let (todo, clean_title) = parse_heading_todo(&title);
            elements.push(OrgElement::Heading {
                level,
                title: clean_title,
                tags,
                todo,
                children: Vec::new(),
            });
            i += 1;
            continue;
        }

        // Source blocks
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("#+begin_src") {
            let header = &trimmed["#+begin_src".len()..].trim_start();
            let lang = header.split_whitespace().next().unwrap_or("").to_string();

            // Parse exports from header args
            let exports = if header.contains(":exports") {
                let blocks = mae_babel::parse_src_blocks(&lines[i..].join("\n"));
                blocks
                    .first()
                    .map(|b| b.header_args.exports.clone())
                    .unwrap_or(mae_babel::ExportsType::Code)
            } else {
                mae_babel::ExportsType::Code
            };

            let mut body_lines = Vec::new();
            i += 1;
            while i < lines.len() {
                if lines[i]
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with("#+end_src")
                {
                    break;
                }
                body_lines.push(lines[i]);
                i += 1;
            }
            elements.push(OrgElement::SrcBlock {
                language: lang,
                body: body_lines.join("\n"),
                exports,
            });
            i += 1;
            continue;
        }

        // Results blocks
        if lower.starts_with("#+results:") || lower.starts_with("#+results[") {
            let mut result_lines = Vec::new();
            i += 1;
            while i < lines.len() {
                let rl = lines[i].trim();
                if rl.is_empty() || rl.starts_with("* ") || rl.starts_with("#+") {
                    break;
                }
                // Strip fixed-width prefix
                if let Some(content) = rl.strip_prefix(": ") {
                    result_lines.push(content.to_string());
                } else {
                    result_lines.push(rl.to_string());
                }
                i += 1;
            }
            elements.push(OrgElement::ResultsBlock(result_lines.join("\n")));
            continue;
        }

        // Quote blocks
        if lower.starts_with("#+begin_quote") {
            let mut quote_lines = Vec::new();
            i += 1;
            while i < lines.len() {
                if lines[i]
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with("#+end_quote")
                {
                    break;
                }
                quote_lines.push(lines[i]);
                i += 1;
            }
            elements.push(OrgElement::Quote(quote_lines.join("\n")));
            i += 1;
            continue;
        }

        // Export blocks
        if lower.starts_with("#+begin_export") {
            let format = lower
                .strip_prefix("#+begin_export")
                .unwrap_or("")
                .trim()
                .to_string();
            let mut content_lines = Vec::new();
            i += 1;
            while i < lines.len() {
                if lines[i]
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with("#+end_export")
                {
                    break;
                }
                content_lines.push(lines[i]);
                i += 1;
            }
            elements.push(OrgElement::ExportBlock {
                format,
                content: content_lines.join("\n"),
            });
            i += 1;
            continue;
        }

        // Horizontal rule
        if trimmed.starts_with("-----") {
            elements.push(OrgElement::HorizontalRule);
            i += 1;
            continue;
        }

        // Comments
        if trimmed.starts_with("# ") || trimmed == "#" {
            i += 1;
            continue;
        }

        // Tables
        if trimmed.starts_with('|') {
            let mut rows = Vec::new();
            let mut has_header = false;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                let row_line = lines[i].trim();
                if row_line.starts_with("|-") {
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

        // Lists
        if trimmed.starts_with("- ") || trimmed.starts_with("+ ") {
            let mut items = Vec::new();
            while i < lines.len() {
                let ll = lines[i].trim();
                if ll.starts_with("- ") || ll.starts_with("+ ") {
                    items.push(ListItem {
                        content: ll[2..].to_string(),
                        children: Vec::new(),
                    });
                    i += 1;
                } else if ll.is_empty() {
                    break;
                } else {
                    // Continuation line
                    if let Some(last) = items.last_mut() {
                        last.content.push(' ');
                        last.content.push_str(ll);
                    }
                    i += 1;
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
                .and_then(|s| s.strip_prefix(". ").or(s.strip_prefix(") ")))
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
                        .and_then(|s| s.strip_prefix(". ").or(s.strip_prefix(") ")))
                    {
                        items.push(ListItem {
                            content: item_rest.to_string(),
                            children: Vec::new(),
                        });
                        i += 1;
                    } else if ll.is_empty() {
                        break;
                    } else {
                        if let Some(last) = items.last_mut() {
                            last.content.push(' ');
                            last.content.push_str(ll);
                        }
                        i += 1;
                    }
                }
                elements.push(OrgElement::List {
                    ordered: true,
                    items,
                });
                continue;
            }
        }

        // Blank lines
        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // Paragraph: collect consecutive non-blank, non-special lines
        let mut para_lines = vec![line.to_string()];
        i += 1;
        while i < lines.len() {
            let pl = lines[i].trim();
            if pl.is_empty()
                || pl.starts_with("* ")
                || pl.starts_with("**")
                || pl.starts_with("#+")
                || pl.starts_with("| ")
                || pl.starts_with("- ")
                || pl.starts_with("+ ")
                || pl.starts_with("-----")
            {
                break;
            }
            para_lines.push(lines[i].to_string());
            i += 1;
        }
        elements.push(OrgElement::Paragraph(para_lines.join("\n")));
    }

    (meta, elements)
}

fn strip_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let lower = line.to_ascii_lowercase();
    if lower.starts_with(keyword) {
        Some(line[keyword.len()..].trim())
    } else {
        None
    }
}

fn parse_options_line(options: &str, opts: &mut ExportOptions) {
    for part in options.split_whitespace() {
        if let Some((key, val)) = part.split_once(':') {
            match key {
                "toc" => {
                    if val == "nil" || val == "no" {
                        opts.toc = false;
                    } else if let Ok(n) = val.parse::<u8>() {
                        opts.toc = true;
                        opts.toc_depth = n;
                    }
                }
                "H" => {
                    if let Ok(n) = val.parse::<u8>() {
                        opts.headline_levels = n;
                    }
                }
                "num" => {
                    opts.num = val != "nil" && val != "no";
                }
                "author" => {
                    opts.author_p = val != "nil" && val != "no";
                }
                "date" => {
                    opts.date_p = val != "nil" && val != "no";
                }
                _ => {}
            }
        }
    }
}

fn parse_heading_tags(text: &str) -> (String, Vec<String>) {
    // Tags are at end: "Title  :tag1:tag2:"
    if let Some(tag_start) = text.rfind("  :") {
        let potential_tags = &text[tag_start + 2..];
        if potential_tags.ends_with(':') {
            let tags: Vec<String> = potential_tags
                .trim_matches(':')
                .split(':')
                .filter(|t| !t.is_empty())
                .map(|t| t.to_string())
                .collect();
            if !tags.is_empty() {
                return (text[..tag_start].trim().to_string(), tags);
            }
        }
    }
    (text.to_string(), Vec::new())
}

fn parse_heading_todo(title: &str) -> (Option<String>, String) {
    let todo_keywords = ["TODO", "DONE", "NEXT", "WAIT", "CANCELLED", "SOMEDAY"];
    for kw in &todo_keywords {
        if let Some(rest) = title.strip_prefix(kw) {
            if rest.starts_with(' ') || rest.is_empty() {
                return (Some(kw.to_string()), rest.trim().to_string());
            }
        }
    }
    (None, title.to_string())
}

#[derive(Debug, Clone, Copy)]
pub enum InlineTarget {
    Html,
    Markdown,
}

/// Convert org inline markup using string slicing (not char-based).
pub fn convert_inline_markup_str(text: &str, target: InlineTarget) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let ch = bytes[i] as char;
        match ch {
            '*' | '/' | '~' | '=' | '+' if is_markup_start(text, i) => {
                if let Some((end, content)) = find_markup_end_str(text, i, ch) {
                    let converted = match (ch, target) {
                        ('*', InlineTarget::Html) => {
                            format!("<b>{}</b>", convert_inline_markup_str(content, target))
                        }
                        ('*', InlineTarget::Markdown) => {
                            format!("**{}**", convert_inline_markup_str(content, target))
                        }
                        ('/', InlineTarget::Html) => {
                            format!("<i>{}</i>", convert_inline_markup_str(content, target))
                        }
                        ('/', InlineTarget::Markdown) => {
                            format!("*{}*", convert_inline_markup_str(content, target))
                        }
                        ('~' | '=', InlineTarget::Html) => format!("<code>{}</code>", content),
                        ('~' | '=', InlineTarget::Markdown) => format!("`{}`", content),
                        ('+', InlineTarget::Html) => format!("<del>{}</del>", content),
                        ('+', InlineTarget::Markdown) => format!("~~{}~~", content),
                        _ => content.to_string(),
                    };
                    result.push_str(&converted);
                    i = end + 1;
                    continue;
                }
            }
            '[' if text[i..].starts_with("[[") => {
                if let Some((end, link_target, label)) = parse_org_link_str(text, i) {
                    match target {
                        InlineTarget::Html => {
                            let display = label.unwrap_or(link_target);
                            result.push_str(&format!(
                                "<a href=\"{}\">{}</a>",
                                html_escape(link_target),
                                html_escape(display)
                            ));
                        }
                        InlineTarget::Markdown => {
                            let display = label.unwrap_or(link_target);
                            result.push_str(&format!("[{}]({})", display, link_target));
                        }
                    }
                    i = end + 1;
                    continue;
                }
            }
            _ => {}
        }
        match (ch, target) {
            ('<', InlineTarget::Html) => result.push_str("&lt;"),
            ('>', InlineTarget::Html) => result.push_str("&gt;"),
            ('&', InlineTarget::Html) => result.push_str("&amp;"),
            ('"', InlineTarget::Html) => result.push_str("&quot;"),
            _ => result.push(ch),
        }
        i += 1;
    }

    result
}

fn is_markup_start(text: &str, pos: usize) -> bool {
    if pos == 0 {
        return true;
    }
    let prev = text.as_bytes()[pos - 1] as char;
    prev.is_whitespace() || matches!(prev, '(' | '{' | '"' | '\'' | '[')
}

fn find_markup_end_str(text: &str, start: usize, marker: char) -> Option<(usize, &str)> {
    let marker_byte = marker as u8;
    let bytes = text.as_bytes();
    // Search for closing marker after start+1
    for end in (start + 2)..bytes.len() {
        if bytes[end] == marker_byte {
            // Closing marker must be followed by whitespace, punctuation, or end
            let after_ok = end + 1 >= bytes.len() || {
                let next = bytes[end + 1] as char;
                next.is_whitespace()
                    || matches!(
                        next,
                        ')' | '}' | '"' | '\'' | '.' | ',' | ';' | ':' | '!' | '?' | ']'
                    )
            };
            // Content must not start/end with whitespace
            let content_ok =
                !bytes[start + 1].is_ascii_whitespace() && !bytes[end - 1].is_ascii_whitespace();
            if after_ok && content_ok {
                return Some((end, &text[start + 1..end]));
            }
        }
    }
    None
}

fn parse_org_link_str(text: &str, start: usize) -> Option<(usize, &str, Option<&str>)> {
    // [[target][label]] or [[target]]
    if !text[start..].starts_with("[[") {
        return None;
    }
    let after_open = start + 2;
    // Find ][  or ]]
    let rest = &text[after_open..];
    if let Some(bracket_pos) = rest.find("][") {
        let target = &text[after_open..after_open + bracket_pos];
        let label_start = after_open + bracket_pos + 2;
        if let Some(close_pos) = text[label_start..].find("]]") {
            let label = &text[label_start..label_start + close_pos];
            return Some((label_start + close_pos + 1, target, Some(label)));
        }
    }
    if let Some(close_pos) = rest.find("]]") {
        let target = &text[after_open..after_open + close_pos];
        return Some((after_open + close_pos + 1, target, None));
    }
    None
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Filter elements by export tags.
pub fn filter_by_tags(
    elements: &[OrgElement],
    select_tags: &[String],
    exclude_tags: &[String],
) -> Vec<OrgElement> {
    elements
        .iter()
        .filter(|el| {
            if let OrgElement::Heading { tags, .. } = el {
                // If exclude tags match, skip
                if exclude_tags.iter().any(|t| tags.contains(t)) {
                    return false;
                }
                // If select tags are specified, only include matching headings
                if !select_tags.is_empty() && !select_tags.iter().any(|t| tags.contains(t)) {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

/// Extract a subtree starting at a given heading level and line index.
pub fn extract_subtree(elements: &[OrgElement], heading_idx: usize) -> Vec<OrgElement> {
    if heading_idx >= elements.len() {
        return Vec::new();
    }

    let start_level = match &elements[heading_idx] {
        OrgElement::Heading { level, .. } => *level,
        _ => return vec![elements[heading_idx].clone()],
    };

    let mut result = vec![elements[heading_idx].clone()];
    for el in &elements[heading_idx + 1..] {
        if let OrgElement::Heading { level, .. } = el {
            if *level <= start_level {
                break;
            }
        }
        result.push(el.clone());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_meta_title() {
        let src = "#+title: My Document\n#+author: Test\n#+date: 2026-01-01\n\nContent\n";
        let (meta, _) = parse_org_document(src);
        assert_eq!(meta.title.as_deref(), Some("My Document"));
        assert_eq!(meta.author.as_deref(), Some("Test"));
        assert_eq!(meta.date.as_deref(), Some("2026-01-01"));
    }

    #[test]
    fn parse_headings() {
        let src = "* Heading 1\n** Heading 2\n*** Heading 3\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 3);
        if let OrgElement::Heading { level, title, .. } = &elements[0] {
            assert_eq!(*level, 1);
            assert_eq!(title, "Heading 1");
        }
    }

    #[test]
    fn parse_heading_with_tags() {
        let src = "* My Heading  :tag1:tag2:\n";
        let (_, elements) = parse_org_document(src);
        if let OrgElement::Heading { title, tags, .. } = &elements[0] {
            assert_eq!(title, "My Heading");
            assert_eq!(tags, &["tag1".to_string(), "tag2".to_string()]);
        }
    }

    #[test]
    fn parse_heading_with_todo() {
        let src = "* TODO My Task\n";
        let (_, elements) = parse_org_document(src);
        if let OrgElement::Heading { todo, title, .. } = &elements[0] {
            assert_eq!(todo.as_deref(), Some("TODO"));
            assert_eq!(title, "My Task");
        }
    }

    #[test]
    fn parse_src_block() {
        let src = "#+begin_src python\nprint(1)\n#+end_src\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 1);
        if let OrgElement::SrcBlock { language, body, .. } = &elements[0] {
            assert_eq!(language, "python");
            assert_eq!(body, "print(1)");
        }
    }

    #[test]
    fn parse_table() {
        let src = "| a | b |\n|---+---|\n| 1 | 2 |\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 1);
        if let OrgElement::Table { rows, has_header } = &elements[0] {
            assert_eq!(rows.len(), 2);
            assert!(*has_header);
        }
    }

    #[test]
    fn parse_list() {
        let src = "- item 1\n- item 2\n- item 3\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 1);
        if let OrgElement::List { ordered, items } = &elements[0] {
            assert!(!ordered);
            assert_eq!(items.len(), 3);
        }
    }

    #[test]
    fn parse_options() {
        let src = "#+OPTIONS: toc:2 H:3 num:nil\n";
        let (meta, _) = parse_org_document(src);
        assert!(meta.options.toc);
        assert_eq!(meta.options.toc_depth, 2);
        assert_eq!(meta.options.headline_levels, 3);
        assert!(!meta.options.num);
    }

    #[test]
    fn inline_markup_bold_html() {
        let result = convert_inline_markup_str("hello *world*", InlineTarget::Html);
        assert_eq!(result, "hello <b>world</b>");
    }

    #[test]
    fn inline_markup_italic_html() {
        let result = convert_inline_markup_str("hello /world/", InlineTarget::Html);
        assert_eq!(result, "hello <i>world</i>");
    }

    #[test]
    fn inline_markup_code_html() {
        let result = convert_inline_markup_str("hello =world=", InlineTarget::Html);
        assert_eq!(result, "hello <code>world</code>");
    }

    #[test]
    fn inline_markup_bold_markdown() {
        let result = convert_inline_markup_str("hello *world*", InlineTarget::Markdown);
        assert_eq!(result, "hello **world**");
    }

    #[test]
    fn inline_link_html() {
        let result =
            convert_inline_markup_str("see [[https://mae.invalid][Example]]", InlineTarget::Html);
        assert!(result.contains("<a href=\"https://mae.invalid\">Example</a>"));
    }

    #[test]
    fn inline_link_markdown() {
        let result = convert_inline_markup_str(
            "see [[https://mae.invalid][Example]]",
            InlineTarget::Markdown,
        );
        assert!(result.contains("[Example](https://mae.invalid)"));
    }

    #[test]
    fn filter_exclude_tags() {
        let elements = vec![
            OrgElement::Heading {
                level: 1,
                title: "Keep".to_string(),
                tags: vec![],
                todo: None,
                children: vec![],
            },
            OrgElement::Heading {
                level: 1,
                title: "Remove".to_string(),
                tags: vec!["noexport".to_string()],
                todo: None,
                children: vec![],
            },
        ];
        let filtered = filter_by_tags(&elements, &[], &["noexport".to_string()]);
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn extract_subtree_works() {
        let elements = vec![
            OrgElement::Heading {
                level: 1,
                title: "H1".to_string(),
                tags: vec![],
                todo: None,
                children: vec![],
            },
            OrgElement::Paragraph("p1".to_string()),
            OrgElement::Heading {
                level: 2,
                title: "H2".to_string(),
                tags: vec![],
                todo: None,
                children: vec![],
            },
            OrgElement::Paragraph("p2".to_string()),
            OrgElement::Heading {
                level: 1,
                title: "H1b".to_string(),
                tags: vec![],
                todo: None,
                children: vec![],
            },
        ];
        let sub = extract_subtree(&elements, 0);
        assert_eq!(sub.len(), 4); // H1, p1, H2, p2
    }

    #[test]
    fn roundtrip_org_to_md_to_org() {
        let org_src =
            "#+title: Test\n\n* Heading\n\nSome paragraph text.\n\n- item one\n- item two\n";
        let (meta, elements) = parse_org_document(org_src);
        let md_exporter = markdown::MarkdownExporter;
        let md = md_exporter.export(&meta, &elements);
        let (meta2, elements2) = markdown_parser::parse_markdown_document(&md);
        assert_eq!(meta2.title.as_deref(), Some("Test"));
        // Should have heading + paragraph + list
        assert!(elements2.len() >= 3, "got {} elements", elements2.len());
    }

    #[test]
    fn roundtrip_md_to_org_to_md() {
        let md_src = "# Heading\n\nA paragraph.\n\n```python\nprint(1)\n```\n\n- one\n- two\n";
        let (meta, elements) = markdown_parser::parse_markdown_document(md_src);
        let org_writer = org_writer::OrgWriter;
        let org = org_writer.export(&meta, &elements);
        let (meta2, elements2) = parse_org_document(&org);
        // Should preserve structure
        assert!(meta2.title.is_none()); // no title in original
        assert!(elements2.len() >= 3, "got {} elements", elements2.len());
        // Check heading survived
        assert!(matches!(
            &elements2[0],
            OrgElement::Heading { level: 1, .. }
        ));
    }
}
