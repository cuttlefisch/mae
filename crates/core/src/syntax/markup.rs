//! Markup span computation: markdown, org, and shared enrichment.

use super::HighlightSpan;

/// Declarative type specifying which inline markup rules apply to a buffer.
/// Follows Emacs's data-driven `font-lock-defaults` pattern: modes declare
/// what to highlight, the engine handles how.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarkupFlavor {
    /// **bold**, `code`, *italic*, ~~strikethrough~~, ``` fences
    Markdown,
    /// *bold*, /italic/, =code=, ~verbatim~, #+begin_src fences
    Org,
    #[default]
    None,
}

/// Generation-keyed cache for markup spans. Avoids recomputing 17 regex
/// patterns every frame for org/markdown buffers.
#[derive(Debug, Clone, Default)]
pub struct MarkupCache {
    pub generation: u64,
    pub flavor: MarkupFlavor,
    /// Start line of the cached range (0 for full-buffer in small files).
    pub line_start: usize,
    /// End line of the cached range (line_count for full-buffer).
    pub line_end: usize,
    /// Byte offset of `line_start` in the rope — spans are absolute byte offsets.
    pub byte_offset: usize,
    pub spans: Vec<HighlightSpan>,
}

impl MarkupCache {
    /// Check if the cache covers the requested viewport range.
    pub fn covers(&self, gen: u64, flavor: MarkupFlavor, vp_start: usize, vp_end: usize) -> bool {
        self.generation == gen
            && self.flavor == flavor
            && self.line_start <= vp_start
            && self.line_end >= vp_end
    }
}

/// Cache for viewport-local code block detection.
#[derive(Debug, Clone, Default)]
pub struct ViewportCodeBlockCache {
    pub generation: u64,
    pub flavor: MarkupFlavor,
    pub line_start: usize,
    pub line_end: usize,
    pub lines: Vec<bool>,
}

/// Single enrichment point -- all callers go through here.
/// Filters out spans that fall inside code blocks (fenced ``` for markdown,
/// #+begin_src/#+end_src for org) so regex-based inline markup doesn't
/// override tree-sitter's injected language highlighting.
pub fn compute_markup_spans(source: &str, flavor: MarkupFlavor) -> Vec<HighlightSpan> {
    let spans = match flavor {
        MarkupFlavor::Markdown => compute_markdown_style_spans(source),
        MarkupFlavor::Org => compute_org_style_spans(source),
        MarkupFlavor::None => return Vec::new(),
    };
    let code_ranges = code_block_byte_ranges(source, flavor);
    filter_code_block_spans(spans, &code_ranges)
}

/// Detect byte ranges of code block content in markdown/org source.
/// Returns `(content_start, content_end)` pairs -- the content between fences,
/// excluding the fence lines themselves.
pub fn code_block_byte_ranges(source: &str, flavor: MarkupFlavor) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut in_block = false;
    let mut block_start = 0;

    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let line_start = offset;
        let line_end = offset + line.len();
        let trimmed = line.trim();

        match flavor {
            MarkupFlavor::Markdown => {
                if trimmed.starts_with("```") {
                    if in_block {
                        ranges.push((block_start, line_start));
                        in_block = false;
                    } else {
                        block_start = line_end;
                        in_block = true;
                    }
                }
            }
            MarkupFlavor::Org => {
                let lower = trimmed.to_ascii_lowercase();
                if lower.starts_with("#+begin_src") {
                    block_start = line_end;
                    in_block = true;
                } else if in_block && lower.starts_with("#+end_src") {
                    ranges.push((block_start, line_start));
                    in_block = false;
                }
            }
            MarkupFlavor::None => {}
        }

        offset = line_end;
    }
    ranges
}

/// Filter out spans whose byte range falls inside any code block region.
fn filter_code_block_spans(
    spans: Vec<HighlightSpan>,
    code_ranges: &[(usize, usize)],
) -> Vec<HighlightSpan> {
    if code_ranges.is_empty() {
        return spans;
    }
    spans
        .into_iter()
        .filter(|span| {
            !code_ranges
                .iter()
                .any(|(start, end)| span.byte_start >= *start && span.byte_end <= *end)
        })
        .collect()
}

/// Regex-based highlighter for org-mode files. This is a stop-gap until
/// a tree-sitter-org crate compatible with tree-sitter 0.25 is available.
///
/// Supported constructs (pragmatic subset):
/// - Headlines: `* Heading`, `** Subheading`, etc.
/// - TODO/DONE keywords in headlines
/// - Tags at end of headline: `:tag:foo:`
/// - Structural directives: `#+TITLE:`, `#+begin_src`, etc.
/// - Blocks: content between `#+begin_*` and `#+end_*` treated as literal
/// - Comments: lines starting with `# `
/// - Timestamps: `<2026-04-16>`, `[2026-04-16 Thu]`
/// - Links: `[[target][label]]` and `[[target]]`
/// - Emphasis: `*bold*`, `/italic/`, `=verbatim=`, `~code~`
/// - List markers: `- `, `+ `, `1. `
pub(crate) fn compute_org_spans(source: &str) -> Vec<HighlightSpan> {
    use regex::Regex;
    use std::sync::OnceLock;

    static HEADLINE: OnceLock<Regex> = OnceLock::new();
    static TODO_KW: OnceLock<Regex> = OnceLock::new();
    static TAGS: OnceLock<Regex> = OnceLock::new();
    static DIRECTIVE: OnceLock<Regex> = OnceLock::new();
    static COMMENT: OnceLock<Regex> = OnceLock::new();
    static TIMESTAMP: OnceLock<Regex> = OnceLock::new();
    static LINK: OnceLock<Regex> = OnceLock::new();
    static BOLD: OnceLock<Regex> = OnceLock::new();
    static ITALIC: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    static VERBATIM: OnceLock<Regex> = OnceLock::new();
    static LIST_MARKER: OnceLock<Regex> = OnceLock::new();

    let headline = HEADLINE.get_or_init(|| Regex::new(r"(?m)^(\*+)( .*)?$").unwrap());
    let todo_kw = TODO_KW
        .get_or_init(|| Regex::new(r"\b(TODO|DONE|NEXT|WAIT|CANCELLED|DEFERRED)\b").unwrap());
    let tags = TAGS.get_or_init(|| Regex::new(r"(?m)\s+(:[\w@:]+:)\s*$").unwrap());
    let directive = DIRECTIVE.get_or_init(|| Regex::new(r"(?m)^#\+[A-Za-z_]+:?.*$").unwrap());
    let comment = COMMENT.get_or_init(|| Regex::new(r"(?m)^#\s.*$").unwrap());
    let timestamp =
        TIMESTAMP.get_or_init(|| Regex::new(r"[<\[]\d{4}-\d{2}-\d{2}[^>\]]*[>\]]").unwrap());
    let link = LINK.get_or_init(|| Regex::new(r"\[\[([^\]]+)\](\[([^\]]+)\])?\]").unwrap());
    static STRIKETHROUGH: OnceLock<Regex> = OnceLock::new();
    static BLOCKQUOTE: OnceLock<Regex> = OnceLock::new();
    static HR: OnceLock<Regex> = OnceLock::new();
    static PRIORITY: OnceLock<Regex> = OnceLock::new();

    let bold = BOLD.get_or_init(|| Regex::new(r"(?:^|[\s(>])\*([^\s*][^*\n]*)\*").unwrap());
    let italic = ITALIC.get_or_init(|| Regex::new(r"(?:^|[\s(>])/([^\s/][^/\n]*)/").unwrap());
    let code = CODE.get_or_init(|| Regex::new(r"(?:^|[\s(>])~([^~\n]+)~").unwrap());
    let verbatim = VERBATIM.get_or_init(|| Regex::new(r"(?:^|[\s(>])=([^=\n]+)=").unwrap());
    let list_marker = LIST_MARKER.get_or_init(|| Regex::new(r"(?m)^\s*([-+]|\d+[.)])\s").unwrap());
    let strikethrough =
        STRIKETHROUGH.get_or_init(|| Regex::new(r"(?:^|[\s(>])\+([^\s+][^+\n]*)\+").unwrap());
    let blockquote = BLOCKQUOTE.get_or_init(|| Regex::new(r"(?m)^(>+)\s?(.*)$").unwrap());
    let hr = HR.get_or_init(|| Regex::new(r"(?m)^-{5,}\s*$").unwrap());
    let priority = PRIORITY.get_or_init(|| {
        Regex::new(r"(?m)(?:TODO|DONE|NEXT|WAIT|CANCELLED|DEFERRED) (\[#[A-C]\])").unwrap()
    });

    let mut spans: Vec<HighlightSpan> = Vec::new();

    // Headlines: the star prefix is punctuation, the text is heading.
    for cap in headline.captures_iter(source) {
        let stars = cap.get(1).unwrap();
        spans.push(HighlightSpan {
            byte_start: stars.start(),
            byte_end: stars.end(),
            theme_key: "punctuation",
        });
        if let Some(rest) = cap.get(2) {
            spans.push(HighlightSpan {
                byte_start: rest.start(),
                byte_end: rest.end(),
                theme_key: "markup.heading",
            });
            // TODO/DONE keyword at the start of the headline.
            if let Some(kw) = todo_kw.find(rest.as_str()) {
                let theme_key = match kw.as_str() {
                    "DONE" | "CANCELLED" | "DEFERRED" => "markup.done",
                    _ => "markup.todo", // TODO, NEXT, WAIT
                };
                spans.push(HighlightSpan {
                    byte_start: rest.start() + kw.start(),
                    byte_end: rest.start() + kw.end(),
                    theme_key,
                });
            }
            // Tags at end of headline.
            if let Some(tag) = tags.captures(rest.as_str()).and_then(|c| c.get(1)) {
                spans.push(HighlightSpan {
                    byte_start: rest.start() + tag.start(),
                    byte_end: rest.start() + tag.end(),
                    theme_key: "attribute",
                });
            }
        }
    }

    for m in directive.find_iter(source) {
        spans.push(HighlightSpan {
            byte_start: m.start(),
            byte_end: m.end(),
            theme_key: "attribute",
        });
    }

    for m in comment.find_iter(source) {
        spans.push(HighlightSpan {
            byte_start: m.start(),
            byte_end: m.end(),
            theme_key: "comment",
        });
    }

    for m in timestamp.find_iter(source) {
        spans.push(HighlightSpan {
            byte_start: m.start(),
            byte_end: m.end(),
            theme_key: "constant",
        });
    }

    for m in link.find_iter(source) {
        spans.push(HighlightSpan {
            byte_start: m.start(),
            byte_end: m.end(),
            theme_key: "markup.link",
        });
    }

    for cap in bold.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start() - 1,
                byte_end: m.start(),
                theme_key: "markup.bold.marker",
            });
            spans.push(HighlightSpan {
                byte_start: m.start(),
                byte_end: m.end(),
                theme_key: "markup.bold",
            });
            spans.push(HighlightSpan {
                byte_start: m.end(),
                byte_end: m.end() + 1,
                theme_key: "markup.bold.marker",
            });
        }
    }
    for cap in italic.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start() - 1,
                byte_end: m.start(),
                theme_key: "markup.italic.marker",
            });
            spans.push(HighlightSpan {
                byte_start: m.start(),
                byte_end: m.end(),
                theme_key: "markup.italic",
            });
            spans.push(HighlightSpan {
                byte_start: m.end(),
                byte_end: m.end() + 1,
                theme_key: "markup.italic.marker",
            });
        }
    }
    for cap in code.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start() - 1,
                byte_end: m.end() + 1,
                theme_key: "markup.literal",
            });
        }
    }
    for cap in verbatim.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start() - 1,
                byte_end: m.end() + 1,
                theme_key: "markup.literal",
            });
        }
    }

    for cap in list_marker.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start(),
                byte_end: m.end(),
                theme_key: "markup.list",
            });
        }
    }

    // Org +strikethrough+ (mirrors bold/italic pattern).
    for cap in strikethrough.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start() - 1,
                byte_end: m.start(),
                theme_key: "markup.strikethrough",
            });
            spans.push(HighlightSpan {
                byte_start: m.start(),
                byte_end: m.end(),
                theme_key: "markup.strikethrough",
            });
            spans.push(HighlightSpan {
                byte_start: m.end(),
                byte_end: m.end() + 1,
                theme_key: "markup.strikethrough",
            });
        }
    }

    // Blockquote: > prefix lines.
    for cap in blockquote.captures_iter(source) {
        if let Some(marker) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: marker.start(),
                byte_end: marker.end(),
                theme_key: "punctuation",
            });
        }
        if let Some(content) = cap.get(2) {
            if !content.as_str().is_empty() {
                spans.push(HighlightSpan {
                    byte_start: content.start(),
                    byte_end: content.end(),
                    theme_key: "markup.quote",
                });
            }
        }
    }

    // Horizontal rule: 5+ dashes on a line by themselves.
    for m in hr.find_iter(source) {
        spans.push(HighlightSpan {
            byte_start: m.start(),
            byte_end: m.end(),
            theme_key: "markup.hr",
        });
    }

    // Priority: [#A], [#B], [#C] after TODO keywords.
    for cap in priority.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            let theme_key = match m.as_str() {
                "[#A]" => "markup.priority.a",
                "[#B]" => "markup.priority.b",
                _ => "markup.priority.c",
            };
            spans.push(HighlightSpan {
                byte_start: m.start(),
                byte_end: m.end(),
                theme_key,
            });
        }
    }

    // Checkbox highlighting: - [ ] or - [x] or - [-]
    {
        static CHECKBOX: OnceLock<Regex> = OnceLock::new();
        let checkbox =
            CHECKBOX.get_or_init(|| Regex::new(r"(?m)(?:[-+*]|\d+[.)]) (\[[ xX\-]\])").unwrap());
        for cap in checkbox.captures_iter(source) {
            if let Some(m) = cap.get(1) {
                let checked = m.as_str().contains('x') || m.as_str().contains('X');
                spans.push(HighlightSpan {
                    byte_start: m.start(),
                    byte_end: m.end(),
                    theme_key: if checked {
                        "markup.checkbox.checked"
                    } else {
                        "markup.checkbox"
                    },
                });
            }
        }
    }

    // Table highlighting: | delimiters and separator lines.
    {
        static TABLE_PIPE: OnceLock<Regex> = OnceLock::new();
        static TABLE_SEP: OnceLock<Regex> = OnceLock::new();
        let table_pipe = TABLE_PIPE.get_or_init(|| Regex::new(r"(?m)^(\|.*\|)\s*$").unwrap());
        let table_sep = TABLE_SEP.get_or_init(|| Regex::new(r"(?m)^\|[-+: |]+\|\s*$").unwrap());
        for m in table_sep.find_iter(source) {
            spans.push(HighlightSpan {
                byte_start: m.start(),
                byte_end: m.end(),
                theme_key: "comment",
            });
        }
        for cap in table_pipe.captures_iter(source) {
            let full = cap.get(1).unwrap();
            let s = full.as_str();
            // Only highlight the pipe characters.
            for (i, ch) in s.char_indices() {
                if ch == '|' {
                    spans.push(HighlightSpan {
                        byte_start: full.start() + i,
                        byte_end: full.start() + i + 1,
                        theme_key: "punctuation",
                    });
                }
            }
        }
    }

    // Filter out every org-markup span computed above that falls entirely
    // inside a #+begin_src/#+end_src block's content. Regex-based inline
    // markup (bold/italic/code/verbatim/comment/timestamp/etc.) routinely
    // false-positives inside real source code — e.g. two `=` signs on one
    // line (`key=lambda x: x`, `plugins = sorted(...)`) look exactly like
    // an org =verbatim= span, and a `#`-prefixed Python comment at column
    // 0 looks exactly like an org comment line — corrupting the
    // language-specific highlighting the src-block injection below adds
    // for that same range. Mirrors `compute_markup_spans`'s identical
    // filtering (already applied to the KB/conversation-buffer code path
    // via `compute_org_style_spans`) — this closes the same gap for the
    // actual org file-buffer path, which was missing it.
    let code_ranges = code_block_byte_ranges(source, MarkupFlavor::Org);
    spans = filter_code_block_spans(spans, &code_ranges);

    // Org src block injection: highlight code inside #+begin_src <lang> ... #+end_src
    {
        static SRC_BLOCK: OnceLock<Regex> = OnceLock::new();
        let src_block = SRC_BLOCK.get_or_init(|| {
            Regex::new(r"(?mi)^[ \t]*#\+begin_src[ \t]+(\w+)[^\n]*\n([\s\S]*?)^[ \t]*#\+end_src")
                .unwrap()
        });
        for cap in src_block.captures_iter(source) {
            if let (Some(lang_m), Some(content_m)) = (cap.get(1), cap.get(2)) {
                if let Some(lang) = super::detection::language_from_id(lang_m.as_str()) {
                    if let Some(config) = super::languages::build_configuration(lang) {
                        let offset = content_m.start();
                        for mut span in
                            super::languages::highlight_with_config(&config, content_m.as_str())
                        {
                            span.byte_start += offset;
                            span.byte_end += offset;
                            spans.push(span);
                        }
                    }
                }
            }
        }
    }

    // Property drawers: :PROPERTIES:, :END:, and property key lines.
    {
        static DRAWER: OnceLock<Regex> = OnceLock::new();
        let drawer = DRAWER.get_or_init(|| Regex::new(r"(?m)^[ \t]*(:[A-Z_]+:)\s*$").unwrap());
        for cap in drawer.captures_iter(source) {
            if let Some(m) = cap.get(1) {
                spans.push(HighlightSpan {
                    byte_start: m.start(),
                    byte_end: m.end(),
                    theme_key: "markup.drawer",
                });
            }
        }

        static PROPERTY_LINE: OnceLock<Regex> = OnceLock::new();
        let property_line =
            PROPERTY_LINE.get_or_init(|| Regex::new(r"(?m)^[ \t]+(:[A-Za-z_]+:)\s+(.+)$").unwrap());
        for cap in property_line.captures_iter(source) {
            if let Some(m) = cap.get(0) {
                spans.push(HighlightSpan {
                    byte_start: m.start(),
                    byte_end: m.end(),
                    theme_key: "markup.drawer",
                });
            }
        }
    }

    // Renderer expects spans sorted by start offset.
    spans.sort_by_key(|s| s.byte_start);
    spans
}

/// Compute inline org-style spans for non-tree-sitter contexts (KB buffers,
/// conversation buffers). Detects *bold*, /italic/, =code=, ~verbatim~ --
/// intentionally excludes headings to avoid triggering `line_heading_scale()`.
pub fn compute_org_style_spans(source: &str) -> Vec<HighlightSpan> {
    use regex::Regex;
    use std::sync::OnceLock;

    static BOLD: OnceLock<Regex> = OnceLock::new();
    static ITALIC: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    static VERBATIM: OnceLock<Regex> = OnceLock::new();

    let bold = BOLD.get_or_init(|| {
        Regex::new(r"(?:^|[\s(>])\*([^\s*][^*\n]*)\*(?:\s|[.,;:!?)>\]]|$)").unwrap()
    });
    let italic = ITALIC
        .get_or_init(|| Regex::new(r"(?:^|[\s(>])/([^\s/][^/\n]*)/(?:\s|[.,;:!?)>\]]|$)").unwrap());
    let code = CODE
        .get_or_init(|| Regex::new(r"(?:^|[\s(>])=([^\s=][^=\n]*)=(?:\s|[.,;:!?)>\]]|$)").unwrap());
    let verbatim = VERBATIM
        .get_or_init(|| Regex::new(r"(?:^|[\s(>])~([^\s~][^~\n]*)~(?:\s|[.,;:!?)>\]]|$)").unwrap());

    let mut spans: Vec<HighlightSpan> = Vec::new();

    for cap in bold.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start().saturating_sub(1),
                byte_end: m.end() + 1,
                theme_key: "markup.bold",
            });
        }
    }
    for cap in italic.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start().saturating_sub(1),
                byte_end: m.end() + 1,
                theme_key: "markup.italic",
            });
        }
    }
    for cap in code.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start().saturating_sub(1),
                byte_end: m.end() + 1,
                theme_key: "markup.literal",
            });
        }
    }
    for cap in verbatim.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start().saturating_sub(1),
                byte_end: m.end() + 1,
                theme_key: "markup.literal",
            });
        }
    }

    spans.sort_by_key(|s| s.byte_start);
    spans
}

/// Compute inline markdown-style spans for non-tree-sitter contexts (KB buffers,
/// conversation buffers). Detects **bold**, `code`, and *italic* -- intentionally
/// excludes headings to avoid triggering `line_heading_scale()` in layout.
pub fn compute_markdown_style_spans(source: &str) -> Vec<HighlightSpan> {
    use regex::Regex;
    use std::sync::OnceLock;

    static BOLD: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    static ITALIC: OnceLock<Regex> = OnceLock::new();
    static STRIKETHROUGH: OnceLock<Regex> = OnceLock::new();

    let bold = BOLD.get_or_init(|| Regex::new(r"\*\*([^*\n]+)\*\*").unwrap());
    let code = CODE.get_or_init(|| Regex::new(r"`([^`\n]+)`").unwrap());
    // Match *italic* that is NOT part of **bold** -- use word boundary approach
    // instead of look-ahead (unsupported by the regex crate).
    let italic = ITALIC.get_or_init(|| {
        Regex::new(r"(?:^|[\s(>])\*([^\s*][^*\n]*)\*(?:\s|[.,;:!?)>\]]|$)").unwrap()
    });
    let strikethrough = STRIKETHROUGH.get_or_init(|| Regex::new(r"~~([^~\n]+)~~").unwrap());

    let mut spans: Vec<HighlightSpan> = Vec::new();

    for cap in bold.captures_iter(source) {
        let full = cap.get(0).unwrap();
        spans.push(HighlightSpan {
            byte_start: full.start(),
            byte_end: full.end(),
            theme_key: "markup.bold",
        });
    }

    for cap in code.captures_iter(source) {
        let full = cap.get(0).unwrap();
        spans.push(HighlightSpan {
            byte_start: full.start(),
            byte_end: full.end(),
            theme_key: "markup.literal",
        });
    }

    for cap in italic.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start().saturating_sub(1),
                byte_end: m.end() + 1,
                theme_key: "markup.italic",
            });
        }
    }

    for cap in strikethrough.captures_iter(source) {
        let full = cap.get(0).unwrap();
        spans.push(HighlightSpan {
            byte_start: full.start(),
            byte_end: full.end(),
            theme_key: "markup.strikethrough",
        });
    }

    // Blockquote: > prefix lines.
    {
        static BLOCKQUOTE: OnceLock<Regex> = OnceLock::new();
        let blockquote = BLOCKQUOTE.get_or_init(|| Regex::new(r"(?m)^(>+)\s?(.*)$").unwrap());
        for cap in blockquote.captures_iter(source) {
            if let Some(marker) = cap.get(1) {
                spans.push(HighlightSpan {
                    byte_start: marker.start(),
                    byte_end: marker.end(),
                    theme_key: "punctuation",
                });
            }
            if let Some(content) = cap.get(2) {
                if !content.as_str().is_empty() {
                    spans.push(HighlightSpan {
                        byte_start: content.start(),
                        byte_end: content.end(),
                        theme_key: "markup.quote",
                    });
                }
            }
        }
    }

    // Horizontal rule: ---, ***, ___ (3+ chars).
    {
        static HR: OnceLock<Regex> = OnceLock::new();
        let hr = HR.get_or_init(|| Regex::new(r"(?m)^(?:-{3,}|\*{3,}|_{3,})\s*$").unwrap());
        for m in hr.find_iter(source) {
            spans.push(HighlightSpan {
                byte_start: m.start(),
                byte_end: m.end(),
                theme_key: "markup.hr",
            });
        }
    }

    // Checkbox highlighting: - [ ] or - [x]
    {
        static CHECKBOX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let checkbox =
            CHECKBOX.get_or_init(|| Regex::new(r"(?m)(?:[-+*]|\d+[.)]) (\[[ xX\-]\])").unwrap());
        for cap in checkbox.captures_iter(source) {
            if let Some(m) = cap.get(1) {
                let checked = m.as_str().contains('x') || m.as_str().contains('X');
                spans.push(HighlightSpan {
                    byte_start: m.start(),
                    byte_end: m.end(),
                    theme_key: if checked {
                        "markup.checkbox.checked"
                    } else {
                        "markup.checkbox"
                    },
                });
            }
        }
    }

    spans.sort_by_key(|s| s.byte_start);
    spans
}

/// Detect lines inside fenced code blocks in markdown (` ``` `) or org (`#+begin_src`/`#+end_src`).
/// Returns a `Vec<bool>` indexed by line number -- `true` if the line is inside a code block
/// (including the fence lines themselves).
pub fn detect_code_block_lines(buf: &crate::Buffer, flavor: MarkupFlavor) -> Vec<bool> {
    let line_count = buf.line_count();
    let mut result = vec![false; line_count];
    if flavor == MarkupFlavor::None {
        return result;
    }

    let mut inside = false;
    for (i, flag) in result.iter_mut().enumerate() {
        let line: String = buf.rope().line(i).chars().collect();
        let trimmed = line.trim();
        if flavor == MarkupFlavor::Org {
            if trimmed.eq_ignore_ascii_case("#+begin_src")
                || trimmed.to_ascii_lowercase().starts_with("#+begin_src ")
            {
                inside = true;
                *flag = true;
                continue;
            }
            if trimmed.eq_ignore_ascii_case("#+end_src") {
                *flag = true;
                inside = false;
                continue;
            }
        } else {
            // Markdown fenced code blocks
            if trimmed.starts_with("```") {
                inside = !inside;
                *flag = true;
                continue;
            }
        }
        if inside {
            *flag = true;
        }
    }
    result
}

/// Compute markup spans for a line range only. O(range) instead of O(buffer).
/// Spans have absolute byte offsets (adjusted by `byte_start_offset`).
pub fn compute_markup_spans_for_range(
    rope: &ropey::Rope,
    flavor: MarkupFlavor,
    line_start: usize,
    line_end: usize,
) -> (usize, Vec<HighlightSpan>) {
    if flavor == MarkupFlavor::None || line_start >= line_end {
        return (0, Vec::new());
    }
    let line_count = rope.len_lines();
    let line_end = line_end.min(line_count);
    let byte_start = rope.line_to_byte(line_start);
    let byte_end = rope.line_to_byte(line_end.min(line_count));
    let slice = rope.byte_slice(byte_start..byte_end);
    let source: String = slice.chars().collect();
    let mut spans = compute_markup_spans(&source, flavor);
    // Adjust spans to absolute byte offsets.
    for span in &mut spans {
        span.byte_start += byte_start;
        span.byte_end += byte_start;
    }
    (byte_start, spans)
}

/// Detect code block lines for a line range. O(range + backward scan) instead of O(buffer).
/// Returns `(line_start, Vec<bool>)` where Vec is indexed relative to `line_start`.
pub fn detect_code_block_lines_for_range(
    buf: &crate::Buffer,
    flavor: MarkupFlavor,
    line_start: usize,
    line_end: usize,
) -> Vec<bool> {
    let line_count = buf.line_count();
    let line_end = line_end.min(line_count);
    if flavor == MarkupFlavor::None || line_start >= line_end {
        return vec![false; line_end.saturating_sub(line_start)];
    }

    // Backward scan to determine initial `inside` state at `line_start`.
    // Capped at 500 lines to bound cost.
    let scan_start = line_start.saturating_sub(500);
    let mut inside = false;
    for i in scan_start..line_start {
        let line: String = buf.rope().line(i).chars().collect();
        let trimmed = line.trim();
        if flavor == MarkupFlavor::Org {
            if trimmed.eq_ignore_ascii_case("#+begin_src")
                || trimmed.to_ascii_lowercase().starts_with("#+begin_src ")
            {
                inside = true;
            } else if trimmed.eq_ignore_ascii_case("#+end_src") {
                inside = false;
            }
        } else {
            if trimmed.starts_with("```") {
                inside = !inside;
            }
        }
    }

    // Forward scan for the requested range.
    let range_len = line_end - line_start;
    let mut result = vec![false; range_len];
    for (rel_idx, flag) in result.iter_mut().enumerate() {
        let i = line_start + rel_idx;
        let line: String = buf.rope().line(i).chars().collect();
        let trimmed = line.trim();
        if flavor == MarkupFlavor::Org {
            if trimmed.eq_ignore_ascii_case("#+begin_src")
                || trimmed.to_ascii_lowercase().starts_with("#+begin_src ")
            {
                inside = true;
                *flag = true;
                continue;
            }
            if trimmed.eq_ignore_ascii_case("#+end_src") {
                *flag = true;
                inside = false;
                continue;
            }
        } else {
            if trimmed.starts_with("```") {
                inside = !inside;
                *flag = true;
                continue;
            }
        }
        if inside {
            *flag = true;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_span(spans: &[HighlightSpan], key: &str) -> bool {
        spans.iter().any(|s| s.theme_key == key)
    }

    fn span_text<'a>(source: &'a str, spans: &[HighlightSpan], key: &str) -> Vec<&'a str> {
        spans
            .iter()
            .filter(|s| s.theme_key == key)
            .map(|s| &source[s.byte_start..s.byte_end])
            .collect()
    }

    // --- Headlines ---

    #[test]
    fn org_spans_headline() {
        let src = "* Heading\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "punctuation"), "star prefix");
        assert!(has_span(&spans, "markup.heading"), "heading text");
    }

    #[test]
    fn org_spans_headline_levels() {
        let src = "** H2\n*** H3\n";
        let spans = compute_org_spans(src);
        let headings = span_text(src, &spans, "markup.heading");
        assert!(headings.iter().any(|t| t.contains("H2")));
        assert!(headings.iter().any(|t| t.contains("H3")));
    }

    // --- TODO/DONE keywords ---

    #[test]
    fn org_spans_todo_keyword() {
        let src = "* TODO Task\n";
        let spans = compute_org_spans(src);
        let todos = span_text(src, &spans, "markup.todo");
        assert!(todos.contains(&"TODO"), "expected TODO span");
    }

    #[test]
    fn org_spans_done_keyword() {
        let src = "* DONE Task\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.done"), "expected markup.done");
    }

    #[test]
    fn org_spans_next_wait_keywords() {
        for kw in &["NEXT", "WAIT"] {
            let src = format!("* {} Task\n", kw);
            let spans = compute_org_spans(&src);
            assert!(
                has_span(&spans, "markup.todo"),
                "{} should be markup.todo",
                kw
            );
        }
    }

    #[test]
    fn org_spans_cancelled_deferred() {
        for kw in &["CANCELLED", "DEFERRED"] {
            let src = format!("* {} Task\n", kw);
            let spans = compute_org_spans(&src);
            assert!(
                has_span(&spans, "markup.done"),
                "{} should be markup.done",
                kw
            );
        }
    }

    // --- Tags ---

    #[test]
    fn org_spans_tags() {
        let src = "* Heading :tag1:tag2:\n";
        let spans = compute_org_spans(src);
        let tags = span_text(src, &spans, "attribute");
        assert!(tags.iter().any(|t| t.contains("tag1")), "expected tag span");
    }

    // --- Directives ---

    #[test]
    fn org_spans_directive() {
        let src = "#+TITLE: My Doc\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "attribute"));
    }

    // --- Comments ---

    #[test]
    fn org_spans_comment() {
        let src = "# this is a comment\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "comment"));
    }

    // --- Timestamps ---

    #[test]
    fn org_spans_timestamp_angle() {
        let src = "Deadline: <2026-05-19>\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "constant"));
    }

    #[test]
    fn org_spans_timestamp_bracket() {
        let src = "Closed: [2026-05-19 Mon]\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "constant"));
    }

    // --- Links ---

    #[test]
    fn org_spans_link_with_label() {
        let src = "Visit [[https://example.com][Example]] here.\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.link"));
    }

    #[test]
    fn org_spans_link_bare() {
        let src = "See [[internal-node]] for details.\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.link"));
    }

    // --- Emphasis ---

    #[test]
    fn org_spans_bold() {
        let src = "This is *bold text* here.\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.bold"));
        assert!(has_span(&spans, "markup.bold.marker"));
    }

    #[test]
    fn org_spans_italic() {
        let src = "This is /italic text/ here.\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.italic"));
        assert!(has_span(&spans, "markup.italic.marker"));
    }

    #[test]
    fn org_spans_code() {
        let src = "Use ~some code~ here.\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.literal"));
    }

    #[test]
    fn org_spans_verbatim() {
        let src = "Use =verbatim text= here.\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.literal"));
    }

    #[test]
    fn org_spans_strikethrough() {
        let src = "This is +struck out+ text.\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.strikethrough"));
    }

    // --- Lists ---

    #[test]
    fn org_spans_list_marker() {
        let src = "- item one\n+ item two\n1. item three\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.list"));
    }

    // --- Checkboxes ---

    #[test]
    fn org_spans_checkbox_unchecked() {
        let src = "- [ ] item\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.checkbox"));
    }

    #[test]
    fn org_spans_checkbox_checked() {
        let src = "- [x] item\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.checkbox.checked"));
    }

    // --- Priorities ---

    #[test]
    fn org_spans_priority_a() {
        let src = "* TODO [#A] Urgent task\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.priority.a"));
    }

    #[test]
    fn org_spans_priority_b() {
        let src = "* TODO [#B] Normal task\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.priority.b"));
    }

    #[test]
    fn org_spans_priority_c() {
        let src = "* TODO [#C] Low task\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.priority.c"));
    }

    // --- Blockquotes ---

    #[test]
    fn org_spans_blockquote() {
        let src = "> quoted text\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "punctuation"), "> marker");
        assert!(has_span(&spans, "markup.quote"), "quote content");
    }

    // --- Horizontal rule ---

    #[test]
    fn org_spans_horizontal_rule() {
        let src = "-----\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.hr"));
    }

    // --- Drawers ---

    #[test]
    fn org_spans_drawer() {
        let src = ":PROPERTIES:\n:END:\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "markup.drawer"));
    }

    #[test]
    fn org_spans_property_line() {
        let src = ":PROPERTIES:\n :ID: abc-123\n:END:\n";
        let spans = compute_org_spans(src);
        let drawer_spans: Vec<_> = spans
            .iter()
            .filter(|s| s.theme_key == "markup.drawer")
            .collect();
        assert!(
            drawer_spans.len() >= 2,
            "expected drawer + property line spans, got {}",
            drawer_spans.len()
        );
    }

    // --- Tables ---

    #[test]
    fn org_spans_table_pipe() {
        let src = "| a | b |\n";
        let spans = compute_org_spans(src);
        let pipes: Vec<_> = spans
            .iter()
            .filter(|s| s.theme_key == "punctuation")
            .collect();
        assert!(
            pipes.len() >= 2,
            "expected pipe punctuation spans, got {}",
            pipes.len()
        );
    }

    #[test]
    fn org_spans_table_separator() {
        let src = "|---+---|\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "comment"), "table separator");
    }

    // --- Code block injection ---

    #[test]
    fn org_spans_src_block_injection() {
        let src = "#+begin_src rust\nfn hello() {}\n#+end_src\n";
        let spans = compute_org_spans(src);
        assert!(
            has_span(&spans, "keyword"),
            "expected injected rust keyword span"
        );
    }

    #[test]
    fn org_spans_code_block_filter() {
        // Verify src block directives still produce attribute spans.
        let src = "#+begin_src python\nprint(\"hello\")\n#+end_src\n";
        let spans = compute_org_spans(src);
        assert!(has_span(&spans, "attribute"), "directive span");
    }

    #[test]
    fn org_spans_src_block_two_equals_signs_is_not_treated_as_verbatim() {
        // Regression guard: a source line with two `=` signs (extremely
        // common in real code — keyword args, assignment, comparisons)
        // looks exactly like an org =verbatim= span if the org-level
        // regex passes aren't excluded from code-block content. Before
        // the fix, this produced a spurious `markup.literal` span running
        // from the first `=` to the second, corrupting the injected
        // Python highlighting underneath it.
        let src = "#+begin_src python\nresult = sorted(xs, key=len)\n#+end_src\n";
        let spans = compute_org_spans(src);
        let code_start = src.find("result").unwrap();
        let code_end = src.find("\n#+end_src").unwrap();
        assert!(
            !spans.iter().any(|s| s.theme_key == "markup.literal"
                && s.byte_start >= code_start
                && s.byte_end <= code_end),
            "no org =verbatim= span should be produced inside the code block: {spans:?}"
        );
        // The injected Python highlighting itself must survive the filter
        // (the `=` and `sorted(...)` are still tree-sitter-highlighted).
        assert!(
            has_span(&spans, "operator"),
            "expected injected python operator span (=) to survive filtering: {spans:?}"
        );
        assert!(
            has_span(&spans, "function"),
            "expected injected python function span (sorted) to survive filtering: {spans:?}"
        );
    }

    #[test]
    fn org_spans_src_block_comment_line_is_not_double_tagged() {
        // Regression guard for the same class of bug: a `#`-prefixed
        // comment at column 0 inside a source block looks exactly like an
        // org comment line (`^#\s.*$`) unless code-block content is
        // excluded from that regex pass too. Both the (buggy) org-regex
        // match and the (correct) injected Python tree-sitter highlight
        // use the same "comment" theme_key, so this doesn't corrupt the
        // *color* — but before the fix it produced a genuine duplicate/
        // overlapping span for the same range. Assert exactly one.
        let src = "#+begin_src python\n# a real python comment\nx = 1\n#+end_src\n";
        let spans = compute_org_spans(src);
        let comment_start = src.find("# a real").unwrap();
        let comment_end = src.find("\nx = 1").unwrap();
        let comment_spans_in_range = spans
            .iter()
            .filter(|s| {
                s.theme_key == "comment"
                    && s.byte_start >= comment_start
                    && s.byte_end <= comment_end
            })
            .count();
        assert_eq!(
            comment_spans_in_range, 1,
            "expected exactly one comment span (from Python's injected highlighting only), got: {spans:?}"
        );
    }

    // --- Sort order ---

    #[test]
    fn org_spans_sorted_by_offset() {
        let src = "* TODO [#A] Heading :tag:\n- [ ] item\n[[link]]\n#+TITLE: T\n";
        let spans = compute_org_spans(src);
        for w in spans.windows(2) {
            assert!(
                w[0].byte_start <= w[1].byte_start,
                "spans not sorted: {} > {}",
                w[0].byte_start,
                w[1].byte_start
            );
        }
    }
}
