//! HTML page export: `mae-org`'s rendered tree ([`crate::document`]) inside
//! MAE's page shell (title, author, date, table of contents, stylesheet).
//!
//! The body is [`mae_org::to_html`] of the rendered tree, so every tag and
//! attribute in it is one `mae-org` allows ([`mae_org::TAGS`],
//! [`mae_org::ATTRIBUTES`]) and no raw HTML from the document passes. The
//! shell's own values (title, author, date, language) are escaped here.

use mae_org::{text_of, to_html, Attr, Element, Node, Tag};

use crate::document::{has_class, is_headline_section, section_heading, section_level, Exported};
use crate::html_escape;

/// The whole HTML page for `doc`.
pub fn to_html_page(doc: &Exported) -> String {
    let mut html = String::with_capacity(4096);

    html.push_str("<!DOCTYPE html>\n<html");
    if let Some(lang) = &doc.language {
        // `#+LANGUAGE:` is document content, not a trusted constant — a
        // `"` closes the attribute (audit #596.3).
        html.push_str(&format!(" lang=\"{}\"", html_escape(lang)));
    }
    html.push_str(">\n<head>\n<meta charset=\"utf-8\">\n");

    let title_text = text_of(&doc.title);
    if !title_text.is_empty() {
        html.push_str(&format!("<title>{}</title>\n", html_escape(&title_text)));
    }
    html.push_str(CSS);
    html.push_str("</head>\n<body>\n");

    if !doc.title.is_empty() {
        html.push_str(&format!(
            "<h1 class=\"title\">{}</h1>\n",
            to_html(&doc.title)
        ));
    }
    if doc.settings.with_author && !doc.author.is_empty() {
        html.push_str(&format!(
            "<p class=\"author\">Author: {}</p>\n",
            to_html(&doc.author)
        ));
    }
    if doc.settings.with_date && !doc.date.is_empty() {
        html.push_str(&format!(
            "<p class=\"date\">Date: {}</p>\n",
            to_html(&doc.date)
        ));
    }
    if doc.settings.toc {
        html.push_str(&table_of_contents(&doc.body, doc.settings.toc_depth));
    }

    html.push_str(&to_html(&doc.body));
    html.push_str("\n</body>\n</html>\n");
    html
}

/// A `<nav>` listing every rendered headline down to `depth` stars, nested
/// as the sections nest, each linking to its heading's anchor. Built from the
/// rendered tree, so a headline the export hides is never listed.
fn table_of_contents(body: &[Node], depth: usize) -> String {
    let mut items = String::new();
    toc_items(body, depth, &mut items);
    if items.is_empty() {
        return String::new();
    }
    format!(
        "<nav id=\"table-of-contents\">\n<h2>Table of Contents</h2>\n<ul>\n{items}</ul>\n</nav>\n"
    )
}

fn toc_items(nodes: &[Node], depth: usize, out: &mut String) {
    for n in nodes {
        let Node::Element(e) = n else { continue };
        if !is_headline_section(e) {
            toc_items(&e.children, depth, out);
            continue;
        }
        if section_level(e).is_some_and(|l| l > depth) {
            continue;
        }
        let Some(heading) = section_heading(e) else {
            continue;
        };
        let mut children = String::new();
        toc_items(&e.children, depth, &mut children);
        out.push_str("<li>");
        match heading.get(Attr::Id) {
            Some(id) => out.push_str(&format!(
                "<a href=\"#{}\">{}</a>",
                html_escape(id),
                html_escape(&heading_text(heading))
            )),
            None => out.push_str(&html_escape(&heading_text(heading))),
        }
        if !children.is_empty() {
            out.push_str(&format!("\n<ul>\n{children}</ul>\n"));
        }
        out.push_str("</li>\n");
    }
}

/// A heading's text without its tags.
fn heading_text(heading: &Element) -> String {
    let shown: Vec<Node> = heading
        .children
        .iter()
        .filter(
            |c| !matches!(c, Node::Element(s) if s.tag == Tag::Span && has_class(s, "org-tags")),
        )
        .cloned()
        .collect();
    text_of(&shown).trim().to_string()
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
.org-tags { float: right; font-size: 0.8em; }
.org-tag { background: #e1f5fe; padding: 0.1em 0.4em; border-radius: 3px; margin-left: 0.3em; }
.org-todo { color: #c62828; font-weight: bold; }
.org-todo.org-done { color: #2e7d32; }
.org-verse { white-space: normal; font-style: italic; }
.org-center { text-align: center; }
.org-underline { text-decoration: underline; }
nav#table-of-contents { background: #f9f9f9; padding: 1rem; border-radius: 4px; margin-bottom: 2rem; }
nav ul { list-style: none; padding-left: 1rem; }
.author, .date { color: #666; font-style: italic; }
pre.org-fixed-width { background: #fffde7; border-left: 3px solid #ffc107; }
.org-footnotes { border-top: 1px solid #eee; margin-top: 2rem; font-size: 0.9em; }
</style>
"#;
