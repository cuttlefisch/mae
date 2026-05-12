//! Markdown/GFM export backend for org documents.

use super::{convert_inline_markup_str, Exporter, InlineTarget, OrgElement, OrgMeta};

pub struct MarkdownExporter;

impl Exporter for MarkdownExporter {
    fn export(&self, meta: &OrgMeta, elements: &[OrgElement]) -> String {
        let mut md = String::with_capacity(4096);

        // YAML frontmatter
        if meta.title.is_some() || meta.author.is_some() || meta.date.is_some() {
            md.push_str("---\n");
            if let Some(title) = &meta.title {
                md.push_str(&format!("title: \"{}\"\n", title));
            }
            if let Some(author) = &meta.author {
                md.push_str(&format!("author: \"{}\"\n", author));
            }
            if let Some(date) = &meta.date {
                md.push_str(&format!("date: \"{}\"\n", date));
            }
            md.push_str("---\n\n");
        }

        for element in elements {
            render_element(&mut md, element);
        }

        md
    }
}

fn render_element(md: &mut String, element: &OrgElement) {
    match element {
        OrgElement::Heading {
            level, title, tags, ..
        } => {
            for _ in 0..*level {
                md.push('#');
            }
            md.push(' ');
            md.push_str(&convert_inline_markup_str(title, InlineTarget::Markdown));
            if !tags.is_empty() {
                md.push_str("  <!-- ");
                md.push_str(&tags.join(", "));
                md.push_str(" -->");
            }
            md.push_str("\n\n");
        }
        OrgElement::Paragraph(text) => {
            md.push_str(&convert_inline_markup_str(text, InlineTarget::Markdown));
            md.push_str("\n\n");
        }
        OrgElement::SrcBlock {
            language,
            body,
            exports,
        } => {
            use mae_babel::ExportsType;
            match exports {
                ExportsType::None | ExportsType::Results => {}
                ExportsType::Code | ExportsType::Both => {
                    md.push_str("```");
                    md.push_str(language);
                    md.push('\n');
                    md.push_str(body);
                    md.push_str("\n```\n\n");
                }
            }
        }
        OrgElement::ResultsBlock(content) => {
            md.push_str("```\n");
            md.push_str(content);
            md.push_str("\n```\n\n");
        }
        OrgElement::List { ordered, items } => {
            for (i, item) in items.iter().enumerate() {
                if *ordered {
                    md.push_str(&format!(
                        "{}. {}\n",
                        i + 1,
                        convert_inline_markup_str(&item.content, InlineTarget::Markdown)
                    ));
                } else {
                    md.push_str(&format!(
                        "- {}\n",
                        convert_inline_markup_str(&item.content, InlineTarget::Markdown)
                    ));
                }
            }
            md.push('\n');
        }
        OrgElement::Table { rows, has_header } => {
            if rows.is_empty() {
                return;
            }
            // GFM table
            let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
            for (i, row) in rows.iter().enumerate() {
                md.push('|');
                for j in 0..cols {
                    let cell = row.get(j).map(|s| s.as_str()).unwrap_or("");
                    md.push_str(&format!(" {} |", cell));
                }
                md.push('\n');
                if i == 0 && (*has_header || rows.len() > 1) {
                    md.push('|');
                    for _ in 0..cols {
                        md.push_str(" --- |");
                    }
                    md.push('\n');
                }
            }
            md.push('\n');
        }
        OrgElement::Quote(text) => {
            for line in text.lines() {
                md.push_str("> ");
                md.push_str(&convert_inline_markup_str(line, InlineTarget::Markdown));
                md.push('\n');
            }
            md.push('\n');
        }
        OrgElement::HorizontalRule => {
            md.push_str("---\n\n");
        }
        OrgElement::Comment(_) => {}
        OrgElement::ExportBlock { format, content } => {
            if format == "markdown" || format == "md" {
                md.push_str(content);
                md.push('\n');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_basic_markdown() {
        let meta = OrgMeta {
            title: Some("Test".to_string()),
            ..Default::default()
        };
        let elements = vec![
            OrgElement::Heading {
                level: 1,
                title: "Section".to_string(),
                tags: vec![],
                todo: None,
                children: vec![],
            },
            OrgElement::Paragraph("Hello *world*".to_string()),
        ];
        let exporter = MarkdownExporter;
        let md = exporter.export(&meta, &elements);
        assert!(md.contains("title: \"Test\""));
        assert!(md.contains("# Section"));
        assert!(md.contains("**world**"));
    }

    #[test]
    fn export_code_block_markdown() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::SrcBlock {
            language: "python".to_string(),
            body: "print(1)".to_string(),
            exports: mae_babel::ExportsType::Code,
        }];
        let exporter = MarkdownExporter;
        let md = exporter.export(&meta, &elements);
        assert!(md.contains("```python\nprint(1)\n```"));
    }

    #[test]
    fn export_table_gfm() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::Table {
            rows: vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ],
            has_header: true,
        }];
        let exporter = MarkdownExporter;
        let md = exporter.export(&meta, &elements);
        assert!(md.contains("| Name | Age |"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| Alice | 30 |"));
    }

    #[test]
    fn export_list_unordered() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::List {
            ordered: false,
            items: vec![
                super::super::ListItem {
                    content: "one".to_string(),
                    children: vec![],
                },
                super::super::ListItem {
                    content: "two".to_string(),
                    children: vec![],
                },
            ],
        }];
        let exporter = MarkdownExporter;
        let md = exporter.export(&meta, &elements);
        assert!(md.contains("- one\n- two"));
    }

    #[test]
    fn export_list_ordered() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::List {
            ordered: true,
            items: vec![
                super::super::ListItem {
                    content: "first".to_string(),
                    children: vec![],
                },
                super::super::ListItem {
                    content: "second".to_string(),
                    children: vec![],
                },
            ],
        }];
        let exporter = MarkdownExporter;
        let md = exporter.export(&meta, &elements);
        assert!(md.contains("1. first\n2. second"));
    }

    #[test]
    fn export_quote() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::Quote("To be or not to be".to_string())];
        let exporter = MarkdownExporter;
        let md = exporter.export(&meta, &elements);
        assert!(md.contains("> To be or not to be"));
    }
}
