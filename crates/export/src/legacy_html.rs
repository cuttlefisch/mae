//! The HTML element renderer `html_graph` draws KB node bodies with, on this
//! crate's legacy org parser (`crate::parse_org_document`).
//!
//! @ai-caution: [architecture-debt] This and the parser it reads are the last
//! org parsing in `mae-export`, kept only for `html_graph` (the KB-subgraph
//! page). The editor's HTML/Markdown export renders through `mae-org`
//! (`crate::document`, issue #942). `html_graph` cannot follow yet: KB node
//! bodies arrive in `mae_kb`'s stored link form `[[UUID|label]]`, which is not
//! Org, so it moves with the KB ingest switch (ADR-112 O4, issue #941). Until
//! then this renderer shares the old parser's blind spots (ADR-112 Fact 3): a
//! `:noexport:` section or a verse/special block in a node body is drawn as
//! prose. Tracked in ROADMAP.md's Architecture Debt. Do not add callers.

use super::{
    convert_inline_markup_str, html_escape, parse_checkbox_marker, CheckboxState, ExportOptions,
    InlineTarget, OrgElement,
};

/// Render a single parsed org element as HTML, appending to `html`.
///
/// Used by `html_graph.rs`'s node-body-to-HTML rendering
/// (`render_node_body_html`, which imports this as `render_org_element`).
pub(crate) fn render_element(html: &mut String, element: &OrgElement, opts: &ExportOptions) {
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
                    let tag = html_escape(tag);
                    html.push_str(&format!("<span class=\"tag-{tag}\">{tag}</span>"));
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
                        // The `#+begin_src <lang>` token is arbitrary document
                        // text; unescaped it breaks out of the class attribute
                        // and injects markup (audit #596.3).
                        html_escape(language),
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
            render_list_items(html, items, *ordered);
        }
        OrgElement::Table { rows, has_header } => {
            // Cell content goes through the same inline-markup converter as
            // every other block (List, Quote, ...) below -- it previously
            // went through html_escape() alone, so =code=, *bold*, and
            // [[id:...][links]] inside a table cell rendered as literal,
            // unconverted org markup instead of real HTML. Every table in
            // the KB was affected, not just one note's.
            html.push_str("<table>\n");
            for (i, row) in rows.iter().enumerate() {
                if i == 0 && *has_header {
                    html.push_str("<thead>\n<tr>");
                    for cell in row {
                        html.push_str("<th>");
                        html.push_str(&convert_inline_markup_str(cell, InlineTarget::Html));
                        html.push_str("</th>");
                    }
                    html.push_str("</tr>\n</thead>\n<tbody>\n");
                } else {
                    html.push_str("<tr>");
                    for cell in row {
                        html.push_str("<td>");
                        html.push_str(&convert_inline_markup_str(cell, InlineTarget::Html));
                        html.push_str("</td>");
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
        OrgElement::Example(content) => {
            // Like SrcBlock: only escaped, never run through inline-markup
            // conversion -- example blocks are meant to show literal text
            // verbatim (including any `*`/`/`/`~` that would otherwise be
            // read as emphasis markers).
            html.push_str("<pre class=\"example\">");
            html.push_str(&html_escape(content));
            html.push_str("</pre>\n");
        }
        OrgElement::HorizontalRule => {
            html.push_str("<hr>\n");
        }
        OrgElement::Comment(_) => {}
        OrgElement::ExportBlock { format, content } => {
            if format == "html" {
                // Never raw: KB content is not necessarily self-authored
                // (federated/shared KBs), and this page is a shareable
                // artifact. Escaped, as it always was here.
                html.push_str(&html_escape(content));
                html.push('\n');
            }
        }
    }
}

/// Render a list item's content, first checking for a leading checkbox
/// marker (`- [ ] text` / `- [X] text` / `- [-] text`) and rendering it as
/// a disabled `<input type="checkbox">` (disabled since this is a static
/// export, not the live editor) ahead of the item's own inline-markup-
/// converted text.
/// Renders a (possibly nested, see `ListItem::children`) list as real
/// `<ul>`/`<ol>` markup -- a nested sub-list becomes a nested `<ul>`/`<ol>`
/// inside its parent's `<li>`, standard HTML list-nesting shape. Recursive
/// so any depth of real nesting renders correctly, not just one level.
fn render_list_items(html: &mut String, items: &[crate::ListItem], ordered: bool) {
    let tag = if ordered { "ol" } else { "ul" };
    html.push_str(&format!("<{}>\n", tag));
    for item in items {
        html.push_str("<li>");
        push_list_item_content(html, &item.content);
        if !item.children.is_empty() {
            render_list_items(html, &item.children, ordered);
        }
        html.push_str("</li>\n");
    }
    html.push_str(&format!("</{}>\n", tag));
}

fn push_list_item_content(html: &mut String, content: &str) {
    if let Some((state, rest)) = parse_checkbox_marker(content) {
        let (checked_attr, aria) = match state {
            CheckboxState::Checked => (" checked", "true"),
            CheckboxState::Partial => ("", "mixed"),
            CheckboxState::Unchecked => ("", "false"),
        };
        html.push_str(&format!(
            "<input type=\"checkbox\" disabled{checked_attr} aria-checked=\"{aria}\"> "
        ));
        html.push_str(&convert_inline_markup_str(rest, InlineTarget::Html));
    } else {
        html.push_str(&convert_inline_markup_str(content, InlineTarget::Html));
    }
}

fn slugify(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
