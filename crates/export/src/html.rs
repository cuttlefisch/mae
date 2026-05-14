//! HTML export backend for org documents.

use super::{
    convert_inline_markup_str, html_escape, ExportOptions, Exporter, InlineTarget, OrgElement,
    OrgMeta,
};

pub struct HtmlExporter;

impl Exporter for HtmlExporter {
    fn export(&self, meta: &OrgMeta, elements: &[OrgElement]) -> String {
        let mut html = String::with_capacity(4096);

        html.push_str("<!DOCTYPE html>\n<html");
        if let Some(lang) = &meta.language {
            html.push_str(&format!(" lang=\"{}\"", lang));
        }
        html.push_str(">\n<head>\n<meta charset=\"utf-8\">\n");

        if let Some(title) = &meta.title {
            html.push_str(&format!("<title>{}</title>\n", html_escape(title)));
        }

        html.push_str(CSS);
        html.push_str("</head>\n<body>\n");

        // Title block
        if let Some(title) = &meta.title {
            html.push_str(&format!(
                "<h1 class=\"title\">{}</h1>\n",
                html_escape(title)
            ));
        }
        if meta.options.author_p {
            if let Some(author) = &meta.author {
                html.push_str(&format!(
                    "<p class=\"author\">Author: {}</p>\n",
                    html_escape(author)
                ));
            }
        }
        if meta.options.date_p {
            if let Some(date) = &meta.date {
                html.push_str(&format!(
                    "<p class=\"date\">Date: {}</p>\n",
                    html_escape(date)
                ));
            }
        }

        // Table of contents
        if meta.options.toc {
            html.push_str(&generate_toc(elements, &meta.options));
        }

        // Content
        for element in elements {
            render_element(&mut html, element, &meta.options);
        }

        html.push_str("</body>\n</html>\n");
        html
    }
}

fn render_element(html: &mut String, element: &OrgElement, opts: &ExportOptions) {
    match element {
        OrgElement::Heading {
            level, title, tags, ..
        } => {
            let h_level = (*level).min(opts.headline_levels).max(1);
            let id = slugify(title);
            html.push_str(&format!(
                "<h{} id=\"{}\">{}</h{}>\n",
                h_level,
                id,
                convert_inline_markup_str(title, InlineTarget::Html),
                h_level
            ));
            if !tags.is_empty() {
                html.push_str("<span class=\"tag\">");
                for tag in tags {
                    html.push_str(&format!("<span class=\"tag-{}\">{}</span>", tag, tag));
                }
                html.push_str("</span>\n");
            }
        }
        OrgElement::Paragraph(text) => {
            html.push_str("<p>");
            html.push_str(&convert_inline_markup_str(text, InlineTarget::Html));
            html.push_str("</p>\n");
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
                    html.push_str(&format!(
                        "<pre><code class=\"language-{}\">{}</code></pre>\n",
                        language,
                        html_escape(body)
                    ));
                }
            }
        }
        OrgElement::ResultsBlock(content) => {
            html.push_str("<pre class=\"results\">");
            html.push_str(&html_escape(content));
            html.push_str("</pre>\n");
        }
        OrgElement::List { ordered, items } => {
            let tag = if *ordered { "ol" } else { "ul" };
            html.push_str(&format!("<{}>\n", tag));
            for item in items {
                html.push_str("<li>");
                html.push_str(&convert_inline_markup_str(
                    &item.content,
                    InlineTarget::Html,
                ));
                html.push_str("</li>\n");
            }
            html.push_str(&format!("</{}>\n", tag));
        }
        OrgElement::Table { rows, has_header } => {
            html.push_str("<table>\n");
            for (i, row) in rows.iter().enumerate() {
                if i == 0 && *has_header {
                    html.push_str("<thead>\n<tr>");
                    for cell in row {
                        html.push_str(&format!("<th>{}</th>", html_escape(cell)));
                    }
                    html.push_str("</tr>\n</thead>\n<tbody>\n");
                } else {
                    html.push_str("<tr>");
                    for cell in row {
                        html.push_str(&format!("<td>{}</td>", html_escape(cell)));
                    }
                    html.push_str("</tr>\n");
                }
            }
            if *has_header {
                html.push_str("</tbody>\n");
            }
            html.push_str("</table>\n");
        }
        OrgElement::Quote(text) => {
            html.push_str("<blockquote>\n<p>");
            html.push_str(&convert_inline_markup_str(text, InlineTarget::Html));
            html.push_str("</p>\n</blockquote>\n");
        }
        OrgElement::HorizontalRule => {
            html.push_str("<hr>\n");
        }
        OrgElement::Comment(_) => {}
        OrgElement::ExportBlock { format, content } => {
            if format == "html" {
                html.push_str(content);
                html.push('\n');
            }
        }
    }
}

fn generate_toc(elements: &[OrgElement], opts: &ExportOptions) -> String {
    let mut toc =
        String::from("<nav id=\"table-of-contents\">\n<h2>Table of Contents</h2>\n<ul>\n");
    for el in elements {
        if let OrgElement::Heading { level, title, .. } = el {
            if *level <= opts.toc_depth {
                let indent = "  ".repeat(*level as usize);
                let id = slugify(title);
                toc.push_str(&format!(
                    "{}<li><a href=\"#{}\">{}</a></li>\n",
                    indent,
                    id,
                    html_escape(title)
                ));
            }
        }
    }
    toc.push_str("</ul>\n</nav>\n");
    toc
}

fn slugify(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

const CSS: &str = r#"<style>
body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; max-width: 800px; margin: 0 auto; padding: 2rem; line-height: 1.6; color: #333; }
h1.title { border-bottom: 2px solid #eee; padding-bottom: 0.5rem; }
pre { background: #f6f8fa; padding: 1rem; border-radius: 4px; overflow-x: auto; }
code { font-family: "JetBrains Mono", "Fira Code", monospace; }
blockquote { border-left: 4px solid #ddd; margin-left: 0; padding-left: 1rem; color: #666; }
table { border-collapse: collapse; width: 100%; margin: 1rem 0; }
th, td { border: 1px solid #ddd; padding: 0.5rem; text-align: left; }
th { background: #f6f8fa; }
.tag { float: right; font-size: 0.8em; }
.tag span { background: #e1f5fe; padding: 0.1em 0.4em; border-radius: 3px; margin-left: 0.3em; }
nav#table-of-contents { background: #f9f9f9; padding: 1rem; border-radius: 4px; margin-bottom: 2rem; }
nav ul { list-style: none; padding-left: 1rem; }
.author, .date { color: #666; font-style: italic; }
pre.results { background: #fffde7; border-left: 3px solid #ffc107; }
</style>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_basic_document() {
        let meta = OrgMeta {
            title: Some("Test".to_string()),
            author: Some("Author".to_string()),
            ..Default::default()
        };
        let elements = vec![
            OrgElement::Heading {
                level: 1,
                title: "Section 1".to_string(),
                tags: vec![],
                todo: None,
                children: vec![],
            },
            OrgElement::Paragraph("Hello *world*".to_string()),
        ];
        let exporter = HtmlExporter;
        let html = exporter.export(&meta, &elements);
        assert!(html.contains("<title>Test</title>"));
        assert!(html.contains("<h1 class=\"title\">Test</h1>"));
        assert!(html.contains("Author: Author"));
        assert!(html.contains("<h1 id=\"section-1\">Section 1</h1>"));
        assert!(html.contains("<b>world</b>"));
    }

    #[test]
    fn export_code_block() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::SrcBlock {
            language: "python".to_string(),
            body: "print(1)".to_string(),
            exports: mae_babel::ExportsType::Code,
        }];
        let exporter = HtmlExporter;
        let html = exporter.export(&meta, &elements);
        assert!(html.contains("<pre><code class=\"language-python\">print(1)</code></pre>"));
    }

    #[test]
    fn export_table() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::Table {
            rows: vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ],
            has_header: true,
        }];
        let exporter = HtmlExporter;
        let html = exporter.export(&meta, &elements);
        assert!(html.contains("<th>Name</th>"));
        assert!(html.contains("<td>Alice</td>"));
    }

    #[test]
    fn export_no_toc() {
        let meta = OrgMeta {
            options: ExportOptions {
                toc: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let elements = vec![OrgElement::Heading {
            level: 1,
            title: "H1".to_string(),
            tags: vec![],
            todo: None,
            children: vec![],
        }];
        let exporter = HtmlExporter;
        let html = exporter.export(&meta, &elements);
        assert!(!html.contains("Table of Contents"));
    }

    #[test]
    fn export_html_escaping() {
        let meta = OrgMeta::default();
        let elements = vec![OrgElement::Paragraph(
            "<script>alert(1)</script>".to_string(),
        )];
        let exporter = HtmlExporter;
        let html = exporter.export(&meta, &elements);
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn toc_generation() {
        let elements = vec![
            OrgElement::Heading {
                level: 1,
                title: "A".to_string(),
                tags: vec![],
                todo: None,
                children: vec![],
            },
            OrgElement::Heading {
                level: 2,
                title: "B".to_string(),
                tags: vec![],
                todo: None,
                children: vec![],
            },
        ];
        let opts = ExportOptions::default();
        let toc = generate_toc(&elements, &opts);
        assert!(toc.contains("<a href=\"#a\">A</a>"));
        assert!(toc.contains("<a href=\"#b\">B</a>"));
    }
}
