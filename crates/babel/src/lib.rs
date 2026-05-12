//! Org-babel source block parser and data model.
//!
//! @stability: stable
//! @since: 0.9.0
//!
//! Parses `#+begin_src` blocks with full header argument extraction.
//! Foundation for babel execution (M2), export (M5), and tangle (M4).

pub mod backend;
pub mod execute;
pub mod noweb;
pub mod results;
pub mod safety;
pub mod session;
pub mod tangle;
pub mod vars;

use std::collections::HashMap;

/// A parsed org-mode source block.
#[derive(Debug, Clone, PartialEq)]
pub struct SrcBlock {
    /// Name from `#+name:` above block.
    pub name: Option<String>,
    /// Language identifier ("python", "rust", "scheme", etc.).
    pub language: String,
    /// All `:key value` header arguments.
    pub header_args: HeaderArgs,
    /// Code between begin/end markers.
    pub body: String,
    /// (begin_line, end_line) 0-indexed inclusive.
    pub line_range: (usize, usize),
    /// (begin_byte, end_byte) of the body content within the source text.
    pub body_byte_range: (usize, usize),
}

/// Parsed header arguments from a source block.
#[derive(Debug, Clone, PartialEq)]
pub struct HeaderArgs {
    pub results: ResultsType,
    pub exports: ExportsType,
    pub var: Vec<(String, VarSource)>,
    pub tangle: TangleTarget,
    pub noweb: NowebMode,
    pub session: Option<String>,
    pub dir: Option<String>,
    pub cache: bool,
    pub eval: EvalPolicy,
    pub file: Option<String>,
    pub mkdirp: bool,
    pub prologue: Option<String>,
    pub epilogue: Option<String>,
    pub wrap: Option<String>,
    pub post: Option<String>,
    pub cmd: Option<String>,
    /// Raw key-value pairs for extensibility.
    pub raw: HashMap<String, String>,
}

impl Default for HeaderArgs {
    fn default() -> Self {
        HeaderArgs {
            results: ResultsType::Output(ResultsFormat::Scalar),
            exports: ExportsType::Code,
            var: Vec::new(),
            tangle: TangleTarget::No,
            noweb: NowebMode::No,
            session: None,
            dir: None,
            cache: false,
            eval: EvalPolicy::Yes,
            file: None,
            mkdirp: false,
            prologue: None,
            epilogue: None,
            wrap: None,
            post: None,
            cmd: None,
            raw: HashMap::new(),
        }
    }
}

/// Evaluation policy for source blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalPolicy {
    Yes,
    Query,
    NoExport,
    Never,
}

/// How results are collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultsType {
    Output(ResultsFormat),
    Value(ResultsFormat),
}

/// How results are formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultsFormat {
    Scalar,
    Table,
    List,
    Drawer,
    Raw,
    Html,
    Org,
}

/// What to include in export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportsType {
    Code,
    Results,
    Both,
    None,
}

/// Tangle target specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TangleTarget {
    No,
    Yes,
    File(String),
}

/// Noweb reference expansion mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NowebMode {
    No,
    Yes,
    Tangle,
}

/// Source of a `:var` binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarSource {
    Literal(String),
    BlockRef(String),
    TableRef(String),
}

/// Parse all source blocks from an org-mode document.
pub fn parse_src_blocks(source: &str) -> Vec<SrcBlock> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut pending_name: Option<String> = None;
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        // Track #+name: directives
        if let Some(name) = trimmed
            .strip_prefix("#+name:")
            .or_else(|| trimmed.strip_prefix("#+NAME:"))
        {
            pending_name = Some(name.trim().to_string());
            i += 1;
            continue;
        }

        // Detect #+begin_src (case-insensitive)
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("#+begin_src") {
            let begin_line = i;
            let header_part = &trimmed["#+begin_src".len()..];
            let (language, header_args) = parse_begin_line(header_part);

            // Find body start byte offset
            let body_byte_start = line_byte_offset(source, i + 1);

            // Find matching #+end_src
            let mut end_line = i + 1;
            while end_line < lines.len() {
                if lines[end_line]
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with("#+end_src")
                {
                    break;
                }
                end_line += 1;
            }

            // Body is lines between begin and end
            let body_lines = if end_line > i + 1 {
                &lines[i + 1..end_line]
            } else {
                &[]
            };
            let body = body_lines.join("\n");
            let body_byte_end = if body.is_empty() {
                body_byte_start
            } else {
                body_byte_start + body.len()
            };

            let actual_end = if end_line < lines.len() {
                end_line
            } else {
                lines.len().saturating_sub(1)
            };

            blocks.push(SrcBlock {
                name: pending_name.take(),
                language,
                header_args,
                body,
                line_range: (begin_line, actual_end),
                body_byte_range: (body_byte_start, body_byte_end),
            });

            i = actual_end + 1;
            continue;
        }

        // Reset pending name if we hit a non-name, non-blank line without begin_src
        if !trimmed.is_empty() && !trimmed.starts_with("#+") && !trimmed.starts_with("#") {
            pending_name = None;
        }

        i += 1;
    }

    blocks
}

/// Parse the header line after `#+begin_src`.
fn parse_begin_line(header: &str) -> (String, HeaderArgs) {
    let header = header.trim();
    if header.is_empty() {
        return (String::new(), HeaderArgs::default());
    }

    let mut parts = header.splitn(2, |c: char| c.is_whitespace());
    let language = parts.next().unwrap_or("").to_string();
    let rest = parts.next().unwrap_or("");

    let args = parse_header_args(rest);
    (language, args)
}

/// Parse `:key value` pairs from a header argument string.
pub fn parse_header_args(header_line: &str) -> HeaderArgs {
    let mut args = HeaderArgs::default();
    let header_line = header_line.trim();
    if header_line.is_empty() {
        return args;
    }

    let pairs = extract_key_value_pairs(header_line);

    for (key, value) in &pairs {
        match key.as_str() {
            "results" => args.results = parse_results_type(value),
            "exports" => args.exports = parse_exports_type(value),
            "var" => {
                if let Some(binding) = parse_var_binding(value) {
                    args.var.push(binding);
                }
            }
            "tangle" => args.tangle = parse_tangle_target(value),
            "noweb" => args.noweb = parse_noweb_mode(value),
            "session" => {
                let v = value.trim().trim_matches('"');
                if v == "none" || v.is_empty() {
                    args.session = None;
                } else {
                    args.session = Some(v.to_string());
                }
            }
            "dir" => args.dir = Some(value.trim().trim_matches('"').to_string()),
            "cache" => args.cache = value.trim() == "yes",
            "eval" => args.eval = parse_eval_policy(value),
            "file" => args.file = Some(value.trim().trim_matches('"').to_string()),
            "mkdirp" => args.mkdirp = value.trim() == "yes",
            "prologue" => args.prologue = Some(value.trim().trim_matches('"').to_string()),
            "epilogue" => args.epilogue = Some(value.trim().trim_matches('"').to_string()),
            "wrap" => args.wrap = Some(value.trim().to_string()),
            "post" => args.post = Some(value.trim().to_string()),
            "cmd" => args.cmd = Some(value.trim().trim_matches('"').to_string()),
            _ => {
                args.raw.insert(key.clone(), value.clone());
            }
        }
    }

    args
}

/// Extract `:key value` pairs from a header argument string.
fn extract_key_value_pairs(s: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(&ch) = chars.peek() {
        if ch == ':' {
            chars.next(); // consume ':'
                          // Read key
            let mut key = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == ':' {
                    break;
                }
                key.push(c);
                chars.next();
            }
            // Skip whitespace
            while let Some(&c) = chars.peek() {
                if !c.is_whitespace() {
                    break;
                }
                chars.next();
            }
            // Read value until next `:key` or end
            let mut value = String::new();
            while let Some(&c) = chars.peek() {
                if c == ':' {
                    // Peek ahead: is this a new key (`:word`) or part of value?
                    // New key = `:` followed by alphabetic
                    let rest: String = chars.clone().skip(1).take(1).collect();
                    if rest.chars().next().is_some_and(|r| r.is_alphabetic()) {
                        break;
                    }
                }
                value.push(c);
                chars.next();
            }
            if !key.is_empty() {
                pairs.push((key, value.trim_end().to_string()));
            }
        } else {
            chars.next();
        }
    }

    pairs
}

fn parse_results_type(value: &str) -> ResultsType {
    let parts: Vec<&str> = value.split_whitespace().collect();
    let mut collection = None;
    let mut format = ResultsFormat::Scalar;

    for part in &parts {
        match *part {
            "output" => collection = Some(false),
            "value" => collection = Some(true),
            "scalar" | "verbatim" => format = ResultsFormat::Scalar,
            "table" | "vector" => format = ResultsFormat::Table,
            "list" => format = ResultsFormat::List,
            "drawer" => format = ResultsFormat::Drawer,
            "raw" => format = ResultsFormat::Raw,
            "html" => format = ResultsFormat::Html,
            "org" => format = ResultsFormat::Org,
            _ => {}
        }
    }

    match collection {
        Some(true) => ResultsType::Value(format),
        _ => ResultsType::Output(format),
    }
}

fn parse_exports_type(value: &str) -> ExportsType {
    match value.trim() {
        "code" => ExportsType::Code,
        "results" => ExportsType::Results,
        "both" => ExportsType::Both,
        "none" => ExportsType::None,
        _ => ExportsType::Code,
    }
}

fn parse_tangle_target(value: &str) -> TangleTarget {
    let v = value.trim().trim_matches('"');
    match v {
        "no" | "" => TangleTarget::No,
        "yes" => TangleTarget::Yes,
        path => TangleTarget::File(path.to_string()),
    }
}

fn parse_noweb_mode(value: &str) -> NowebMode {
    match value.trim() {
        "yes" => NowebMode::Yes,
        "tangle" => NowebMode::Tangle,
        _ => NowebMode::No,
    }
}

fn parse_eval_policy(value: &str) -> EvalPolicy {
    match value.trim() {
        "yes" => EvalPolicy::Yes,
        "query" => EvalPolicy::Query,
        "no-export" | "noexport" => EvalPolicy::NoExport,
        "never" | "no" => EvalPolicy::Never,
        _ => EvalPolicy::Yes,
    }
}

fn parse_var_binding(value: &str) -> Option<(String, VarSource)> {
    let eq_pos = value.find('=')?;
    let name = value[..eq_pos].trim().to_string();
    let rhs = value[eq_pos + 1..].trim();

    if rhs.is_empty() {
        return None;
    }

    let source = if rhs.starts_with('"') && rhs.ends_with('"') {
        VarSource::Literal(rhs[1..rhs.len() - 1].to_string())
    } else if rhs.contains('[') || rhs.contains('(') {
        // Table reference: data[2,3] or (func)
        VarSource::TableRef(rhs.to_string())
    } else if rhs
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        && !rhs.is_empty()
    {
        // Could be a block ref or literal number
        if rhs
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
        {
            VarSource::Literal(rhs.to_string())
        } else {
            VarSource::BlockRef(rhs.to_string())
        }
    } else {
        VarSource::Literal(rhs.to_string())
    };

    Some((name, source))
}

/// Find the `#+RESULTS:` block below a source block.
/// Returns `(start_line, end_line)` of the results block (0-indexed, inclusive).
pub fn find_results_block(source: &str, after_line: usize) -> Option<(usize, usize)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut i = after_line;

    // Skip blank lines
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }

    if i >= lines.len() {
        return None;
    }

    let trimmed = lines[i].trim();
    let lower = trimmed.to_ascii_lowercase();

    // Check for #+RESULTS: (with optional name)
    if !lower.starts_with("#+results:") && !lower.starts_with("#+results[") {
        return None;
    }

    let results_line = i;

    // Check what follows — could be:
    // 1. A drawer (:RESULTS: ... :END:)
    // 2. A fixed-width block (: line)
    // 3. An example block (#+begin_example ... #+end_example)
    // 4. A single line or table
    i += 1;

    if i >= lines.len() {
        return Some((results_line, results_line));
    }

    let next_trimmed = lines[i].trim();

    // Drawer
    if next_trimmed == ":RESULTS:" {
        while i < lines.len() {
            if lines[i].trim() == ":END:" {
                return Some((results_line, i));
            }
            i += 1;
        }
        return Some((results_line, lines.len().saturating_sub(1)));
    }

    // Example block
    if next_trimmed
        .to_ascii_lowercase()
        .starts_with("#+begin_example")
    {
        while i < lines.len() {
            if lines[i]
                .trim()
                .to_ascii_lowercase()
                .starts_with("#+end_example")
            {
                return Some((results_line, i));
            }
            i += 1;
        }
        return Some((results_line, lines.len().saturating_sub(1)));
    }

    // Fixed-width or table or plain lines until blank line or next element
    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() {
            return Some((results_line, i.saturating_sub(1)));
        }
        // Stop at next org element
        if line.starts_with("#+") || line.starts_with("* ") {
            return Some((results_line, i.saturating_sub(1)));
        }
        i += 1;
    }

    Some((results_line, lines.len().saturating_sub(1)))
}

/// Find a named block in a list of parsed blocks.
pub fn find_named_block<'a>(blocks: &'a [SrcBlock], name: &str) -> Option<&'a SrcBlock> {
    blocks.iter().find(|b| b.name.as_deref() == Some(name))
}

/// Parse buffer-level header args from `#+PROPERTY: header-args` lines.
pub fn parse_buffer_header_args(source: &str) -> HeaderArgs {
    let mut combined = String::new();
    for line in source.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("#+property:") {
            let rest = &trimmed["#+property:".len()..].trim_start();
            if let Some(args) = rest
                .strip_prefix("header-args")
                .or_else(|| rest.strip_prefix("HEADER-ARGS"))
            {
                // Could be `header-args:python :var x=1` or just `header-args :var x=1`
                let args = args
                    .trim_start_matches(':')
                    .trim_start_matches(|c: char| c.is_alphanumeric() || c == '-');
                combined.push(' ');
                combined.push_str(args.trim());
            }
        }
    }
    parse_header_args(&combined)
}

/// Merge buffer-level header args with block-level args (block wins).
pub fn merge_header_args(buffer_args: &HeaderArgs, block_args: &HeaderArgs) -> HeaderArgs {
    let default = HeaderArgs::default();

    HeaderArgs {
        results: if block_args.results != default.results {
            block_args.results.clone()
        } else {
            buffer_args.results.clone()
        },
        exports: if block_args.exports != default.exports {
            block_args.exports.clone()
        } else {
            buffer_args.exports.clone()
        },
        var: {
            let mut vars = buffer_args.var.clone();
            vars.extend(block_args.var.iter().cloned());
            vars
        },
        tangle: if block_args.tangle != default.tangle {
            block_args.tangle.clone()
        } else {
            buffer_args.tangle.clone()
        },
        noweb: if block_args.noweb != default.noweb {
            block_args.noweb.clone()
        } else {
            buffer_args.noweb.clone()
        },
        session: block_args
            .session
            .clone()
            .or_else(|| buffer_args.session.clone()),
        dir: block_args.dir.clone().or_else(|| buffer_args.dir.clone()),
        cache: block_args.cache || buffer_args.cache,
        eval: if block_args.eval != default.eval {
            block_args.eval.clone()
        } else {
            buffer_args.eval.clone()
        },
        file: block_args.file.clone().or_else(|| buffer_args.file.clone()),
        mkdirp: block_args.mkdirp || buffer_args.mkdirp,
        prologue: block_args
            .prologue
            .clone()
            .or_else(|| buffer_args.prologue.clone()),
        epilogue: block_args
            .epilogue
            .clone()
            .or_else(|| buffer_args.epilogue.clone()),
        wrap: block_args.wrap.clone().or_else(|| buffer_args.wrap.clone()),
        post: block_args.post.clone().or_else(|| buffer_args.post.clone()),
        cmd: block_args.cmd.clone().or_else(|| buffer_args.cmd.clone()),
        raw: {
            let mut raw = buffer_args.raw.clone();
            raw.extend(block_args.raw.iter().map(|(k, v)| (k.clone(), v.clone())));
            raw
        },
    }
}

/// Compute byte offset of line `n` in source (0-indexed).
fn line_byte_offset(source: &str, line: usize) -> usize {
    let mut offset = 0;
    for (i, l) in source.lines().enumerate() {
        if i == line {
            return offset;
        }
        offset += l.len() + 1; // +1 for newline
    }
    offset.min(source.len())
}

/// Simple tilde expansion: `~/foo` → `/home/user/foo`.
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    path.to_string()
}

/// Find the source block containing a given line (0-indexed).
pub fn find_block_at_line(blocks: &[SrcBlock], line: usize) -> Option<&SrcBlock> {
    blocks
        .iter()
        .find(|b| line >= b.line_range.0 && line <= b.line_range.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_python_block() {
        let src = r#"#+begin_src python
print("hello")
#+end_src"#;
        let blocks = parse_src_blocks(src);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].language, "python");
        assert_eq!(blocks[0].body, "print(\"hello\")");
        assert_eq!(blocks[0].line_range, (0, 2));
        assert!(blocks[0].name.is_none());
    }

    #[test]
    fn parse_named_block() {
        let src = r#"#+name: greeting
#+begin_src python
print("hello")
#+end_src"#;
        let blocks = parse_src_blocks(src);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].name.as_deref(), Some("greeting"));
    }

    #[test]
    fn parse_multiple_blocks() {
        let src = r#"* Heading

#+begin_src python
print("one")
#+end_src

Some text.

#+begin_src rust
fn main() {}
#+end_src"#;
        let blocks = parse_src_blocks(src);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].language, "python");
        assert_eq!(blocks[1].language, "rust");
    }

    #[test]
    fn parse_header_args_results() {
        let args = parse_header_args(":results output drawer");
        assert_eq!(args.results, ResultsType::Output(ResultsFormat::Drawer));
    }

    #[test]
    fn parse_header_args_value_table() {
        let args = parse_header_args(":results value table");
        assert_eq!(args.results, ResultsType::Value(ResultsFormat::Table));
    }

    #[test]
    fn parse_header_args_tangle() {
        let args = parse_header_args(":tangle \"output.py\"");
        assert_eq!(args.tangle, TangleTarget::File("output.py".to_string()));
    }

    #[test]
    fn parse_header_args_tangle_yes() {
        let args = parse_header_args(":tangle yes");
        assert_eq!(args.tangle, TangleTarget::Yes);
    }

    #[test]
    fn parse_header_args_session() {
        let args = parse_header_args(":session \"my-repl\"");
        assert_eq!(args.session, Some("my-repl".to_string()));
    }

    #[test]
    fn parse_header_args_eval_never() {
        let args = parse_header_args(":eval never");
        assert_eq!(args.eval, EvalPolicy::Never);
    }

    #[test]
    fn parse_header_args_var_literal() {
        let args = parse_header_args(":var x=42");
        assert_eq!(args.var.len(), 1);
        assert_eq!(args.var[0].0, "x");
        assert_eq!(args.var[0].1, VarSource::Literal("42".to_string()));
    }

    #[test]
    fn parse_header_args_var_block_ref() {
        let args = parse_header_args(":var data=compute-values");
        assert_eq!(args.var.len(), 1);
        assert_eq!(args.var[0].0, "data");
        assert_eq!(
            args.var[0].1,
            VarSource::BlockRef("compute-values".to_string())
        );
    }

    #[test]
    fn parse_header_args_multiple() {
        let args = parse_header_args(":results output :dir /tmp :cache yes :exports both");
        assert_eq!(args.results, ResultsType::Output(ResultsFormat::Scalar));
        assert_eq!(args.dir, Some("/tmp".to_string()));
        assert!(args.cache);
        assert_eq!(args.exports, ExportsType::Both);
    }

    #[test]
    fn parse_header_args_noweb() {
        let args = parse_header_args(":noweb yes");
        assert_eq!(args.noweb, NowebMode::Yes);
    }

    #[test]
    fn parse_header_args_empty() {
        let args = parse_header_args("");
        assert_eq!(args, HeaderArgs::default());
    }

    #[test]
    fn find_results_block_simple() {
        let src = "#+begin_src python\nprint(1)\n#+end_src\n\n#+RESULTS:\n: 1\n";
        let result = find_results_block(src, 3);
        assert!(result.is_some());
        let (start, end) = result.unwrap();
        assert_eq!(start, 4);
        assert_eq!(end, 5);
    }

    #[test]
    fn find_results_block_drawer() {
        let src = "#+end_src\n\n#+RESULTS:\n:RESULTS:\nsome output\n:END:\n";
        let result = find_results_block(src, 1);
        assert!(result.is_some());
        let (start, end) = result.unwrap();
        assert_eq!(start, 2);
        assert_eq!(end, 5);
    }

    #[test]
    fn find_results_block_missing() {
        let src = "#+end_src\n\nSome paragraph.\n";
        let result = find_results_block(src, 1);
        assert!(result.is_none());
    }

    #[test]
    fn find_named_block_works() {
        let src = "#+name: foo\n#+begin_src python\nx = 1\n#+end_src\n#+name: bar\n#+begin_src rust\nfn main() {}\n#+end_src\n";
        let blocks = parse_src_blocks(src);
        assert!(find_named_block(&blocks, "foo").is_some());
        assert!(find_named_block(&blocks, "bar").is_some());
        assert!(find_named_block(&blocks, "baz").is_none());
    }

    #[test]
    fn parse_buffer_header_args_property() {
        let src = "#+PROPERTY: header-args :dir /tmp :cache yes\n* Heading\n";
        let args = parse_buffer_header_args(src);
        assert_eq!(args.dir, Some("/tmp".to_string()));
        assert!(args.cache);
    }

    #[test]
    fn parse_case_insensitive_begin() {
        let src = "#+BEGIN_SRC python\nprint(1)\n#+END_SRC";
        let blocks = parse_src_blocks(src);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].language, "python");
    }

    #[test]
    fn empty_block() {
        let src = "#+begin_src python\n#+end_src";
        let blocks = parse_src_blocks(src);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].body, "");
    }

    #[test]
    fn block_with_header_args_on_begin_line() {
        let src = "#+begin_src python :results output :dir /tmp\nprint(1)\n#+end_src";
        let blocks = parse_src_blocks(src);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].language, "python");
        assert_eq!(
            blocks[0].header_args.results,
            ResultsType::Output(ResultsFormat::Scalar)
        );
        assert_eq!(blocks[0].header_args.dir, Some("/tmp".to_string()));
    }

    #[test]
    fn find_block_at_line_works() {
        let src = "Text\n#+begin_src python\nprint(1)\n#+end_src\nMore text\n";
        let blocks = parse_src_blocks(src);
        assert!(find_block_at_line(&blocks, 0).is_none());
        assert!(find_block_at_line(&blocks, 1).is_some());
        assert!(find_block_at_line(&blocks, 2).is_some());
        assert!(find_block_at_line(&blocks, 3).is_some());
        assert!(find_block_at_line(&blocks, 4).is_none());
    }

    #[test]
    fn merge_header_args_block_wins() {
        let buf = parse_header_args(":dir /tmp :cache yes");
        let block = parse_header_args(":dir /home");
        let merged = merge_header_args(&buf, &block);
        assert_eq!(merged.dir, Some("/home".to_string()));
        assert!(merged.cache); // inherited from buffer
    }

    #[test]
    fn parse_header_args_cmd() {
        let args = parse_header_args(":cmd /usr/bin/python3.11");
        assert_eq!(args.cmd, Some("/usr/bin/python3.11".to_string()));
    }

    #[test]
    fn parse_header_args_exports_none() {
        let args = parse_header_args(":exports none");
        assert_eq!(args.exports, ExportsType::None);
    }

    #[test]
    fn parse_header_args_eval_query() {
        let args = parse_header_args(":eval query");
        assert_eq!(args.eval, EvalPolicy::Query);
    }

    #[test]
    fn parse_header_args_mkdirp() {
        let args = parse_header_args(":mkdirp yes");
        assert!(args.mkdirp);
    }
}
