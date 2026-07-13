//! Results formatting and insertion for babel execution.

use super::{find_results_block, ResultsFormat, ResultsType};

/// Format execution output according to the `:results` directive.
pub fn format_results(output: &str, results_type: &ResultsType) -> String {
    let format = match results_type {
        ResultsType::Output(f) | ResultsType::Value(f) => f,
    };

    match format {
        ResultsFormat::Scalar => format_scalar(output),
        ResultsFormat::Drawer => format_drawer(output),
        ResultsFormat::Table => format_table(output),
        ResultsFormat::List => format_list(output),
        ResultsFormat::Raw => output.to_string(),
        ResultsFormat::Html => format!("#+begin_export html\n{}\n#+end_export", output.trim_end()),
        ResultsFormat::Org => output.to_string(),
    }
}

fn format_scalar(output: &str) -> String {
    let trimmed = output.trim_end_matches('\n');
    if trimmed.contains('\n') {
        // Multi-line: use fixed-width format
        trimmed
            .lines()
            .map(|l| format!(": {}", l))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        format!(": {}", trimmed)
    }
}

fn format_drawer(output: &str) -> String {
    format!(":RESULTS:\n{}\n:END:", output.trim_end())
}

fn format_table(output: &str) -> String {
    // Try to parse as TSV/CSV and format as org table
    let lines: Vec<&str> = output.trim().lines().collect();
    if lines.is_empty() {
        return ": ".to_string();
    }

    let rows: Vec<Vec<&str>> = lines
        .iter()
        .map(|l| l.split('\t').collect::<Vec<_>>())
        .collect();

    if rows.iter().all(|r| r.len() == 1) {
        // Not tab-separated — try comma
        let rows: Vec<Vec<&str>> = lines
            .iter()
            .map(|l| l.split(',').map(|c| c.trim()).collect::<Vec<_>>())
            .collect();
        format_org_table(&rows)
    } else {
        format_org_table(&rows)
    }
}

fn format_org_table(rows: &[Vec<&str>]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < cols {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    let mut result = String::new();
    for (ri, row) in rows.iter().enumerate() {
        result.push('|');
        for (i, width) in widths.iter().enumerate() {
            let cell = row.get(i).copied().unwrap_or("");
            result.push(' ');
            result.push_str(cell);
            for _ in cell.len()..*width {
                result.push(' ');
            }
            result.push_str(" |");
        }
        result.push('\n');
        // Add separator after first row (header)
        if ri == 0 && rows.len() > 1 {
            result.push('|');
            for w in &widths {
                result.push('-');
                for _ in 0..*w {
                    result.push('-');
                }
                result.push_str("-+");
            }
            // Fix last separator
            if result.ends_with("+\n") || result.ends_with('+') {
                let len = result.len();
                result.replace_range(len - 1..len, "|");
            }
            result.push('\n');
        }
    }

    result.trim_end().to_string()
}

fn format_list(output: &str) -> String {
    output
        .trim()
        .lines()
        .map(|l| format!("- {}", l.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the full results text to insert, including the `#+RESULTS:` header.
pub fn build_results_text(name: Option<&str>, formatted: &str) -> String {
    let header = match name {
        Some(n) => format!("#+RESULTS: {}", n),
        None => "#+RESULTS:".to_string(),
    };
    format!("{}\n{}", header, formatted)
}

/// Calculate the insertion point and deletion range for results.
/// Returns `(delete_start_char, delete_end_char, insert_text)` — CHARACTER
/// offsets, matching `Buffer::insert_text_at`/`delete_range`'s `ropey`-backed
/// char-index API (not byte offsets — see `line_char_offset_of`).
/// If no existing results block, `delete_start == delete_end` (pure insertion).
pub fn compute_results_edit(
    source: &str,
    block_end_line: usize,
    block_name: Option<&str>,
    formatted_output: &str,
) -> (usize, usize, String) {
    let results_text = build_results_text(block_name, formatted_output);

    if let Some((results_start, results_end)) = find_results_block(source, block_end_line + 1) {
        // Replace existing results block
        let lines: Vec<&str> = source.lines().collect();
        let del_start = line_char_offset_of(source, results_start);
        let del_end = if results_end + 1 < lines.len() {
            line_char_offset_of(source, results_end + 1)
        } else {
            source.chars().count()
        };
        (del_start, del_end, format!("{}\n", results_text))
    } else {
        // Insert after #+end_src line
        let insert_at = line_char_offset_of(source, block_end_line + 1);
        (insert_at, insert_at, format!("\n{}\n", results_text))
    }
}

/// Read the content of a `#+RESULTS:` block (excluding the header line).
/// Strips fixed-width prefixes (`: `) and drawer markers (`:RESULTS:` / `:END:`).
pub fn read_results_content(source: &str, results_start: usize, results_end: usize) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut content = Vec::new();
    // Skip the #+RESULTS: header line, read content lines
    for line in &lines[results_start + 1..=results_end.min(lines.len().saturating_sub(1))] {
        let trimmed = line.trim();
        if trimmed == ":RESULTS:" || trimmed == ":END:" {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(": ") {
            content.push(rest.to_string());
        } else {
            content.push(trimmed.to_string());
        }
    }
    content.join("\n")
}

/// Compute the CHARACTER offset (not byte offset) where line `line` starts
/// in `source`. `Buffer::insert_text_at`/`delete_range` index into a `ropey`
/// rope by character, so a byte offset here silently drifts the target
/// position forward by however many extra UTF-8 bytes any multi-byte
/// character earlier in the buffer contributes — landing mid-word in
/// whatever text follows, and (since the drifted position is never where
/// `find_results_block` looks next time) making every re-execution insert a
/// fresh, again-mis-positioned block instead of replacing the old one.
fn line_char_offset_of(source: &str, line: usize) -> usize {
    let mut offset = 0;
    for (i, l) in source.lines().enumerate() {
        if i == line {
            return offset;
        }
        offset += l.chars().count() + 1;
    }
    offset.min(source.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResultsType;

    #[test]
    fn format_scalar_single_line() {
        let r = format_results("42\n", &ResultsType::Output(ResultsFormat::Scalar));
        assert_eq!(r, ": 42");
    }

    #[test]
    fn format_scalar_multiline() {
        let r = format_results(
            "line1\nline2\n",
            &ResultsType::Output(ResultsFormat::Scalar),
        );
        assert_eq!(r, ": line1\n: line2");
    }

    #[test]
    fn format_drawer_output() {
        let r = format_results("hello\n", &ResultsType::Output(ResultsFormat::Drawer));
        assert_eq!(r, ":RESULTS:\nhello\n:END:");
    }

    #[test]
    fn format_list_output() {
        let r = format_results("a\nb\nc\n", &ResultsType::Output(ResultsFormat::List));
        assert_eq!(r, "- a\n- b\n- c");
    }

    #[test]
    fn format_raw_passthrough() {
        let r = format_results("raw text\n", &ResultsType::Output(ResultsFormat::Raw));
        assert_eq!(r, "raw text\n");
    }

    #[test]
    fn build_results_with_name() {
        let r = build_results_text(Some("myblock"), ": 42");
        assert_eq!(r, "#+RESULTS: myblock\n: 42");
    }

    #[test]
    fn build_results_without_name() {
        let r = build_results_text(None, ": 42");
        assert_eq!(r, "#+RESULTS:\n: 42");
    }

    #[test]
    fn compute_edit_new_results() {
        let src = "#+begin_src python\nprint(1)\n#+end_src\n";
        let (del_start, del_end, text) = compute_results_edit(src, 2, None, ": 1");
        assert_eq!(del_start, del_end); // pure insertion
        assert!(text.contains("#+RESULTS:"));
        assert!(text.contains(": 1"));
    }

    #[test]
    fn compute_edit_replace_results() {
        let src = "#+begin_src python\nprint(1)\n#+end_src\n\n#+RESULTS:\n: old\n";
        let (del_start, del_end, text) = compute_results_edit(src, 2, None, ": new");
        assert!(del_start < del_end); // deletion range
        assert!(text.contains(": new"));
    }

    /// Apply a `(del_start, del_end, insert_text)` edit to `source` using
    /// CHARACTER offsets — mirroring `Buffer::delete_range`/`insert_text_at`
    /// (ropey char-index semantics), which is what every real caller
    /// (`babel_run_block`, `babel_execute_all`) actually applies these
    /// offsets to.
    fn apply_char_edit(source: &str, del_start: usize, del_end: usize, insert: &str) -> String {
        let mut chars: Vec<char> = source.chars().collect();
        chars.splice(del_start..del_end, insert.chars());
        chars.into_iter().collect()
    }

    #[test]
    fn compute_edit_new_results_lands_right_after_end_src_with_multibyte_content_earlier() {
        // Regression guard: `line_char_offset_of` used to compute a BYTE
        // offset (`l.len()`) and hand it to a char-offset consumer. Any
        // multi-byte character earlier in the buffer (em dash, smart quotes,
        // checkmark, accented letter — all plausible in real org notes) then
        // drifted the insertion point forward by the extra byte count,
        // landing the results block mid-word somewhere past the intended
        // spot instead of directly after `#+end_src`.
        let src = "* Café — Notes\nSome unicode: \u{2192} \u{2713} \u{201c}quotes\u{201d}\n\n\
                   #+begin_src python\nprint(1)\n#+end_src\n\n** Downstream Section\n";
        let block_end_line = src.lines().position(|l| l.trim() == "#+end_src").unwrap();
        let (del_start, del_end, insert_text) =
            compute_results_edit(src, block_end_line, None, ": 1");
        assert_eq!(
            del_start, del_end,
            "pure insertion, no existing results block"
        );

        let result = apply_char_edit(src, del_start, del_end, &insert_text);
        assert!(
            result.contains("#+end_src\n\n#+RESULTS:\n: 1\n\n** Downstream Section"),
            "results must land directly after #+end_src, and the following heading \
             must survive completely intact — got:\n{result}"
        );
        assert!(
            result.contains("** Downstream Section"),
            "the heading must not be split mid-word — got:\n{result}"
        );
    }

    #[test]
    fn compute_edit_replaces_rather_than_stacks_on_second_execution() {
        // Regression guard: bug #2 (stacking output) was a *consequence* of
        // bug #1 — because the first execution landed at a drifted (wrong)
        // position, `find_results_block` never found it on the next run, so
        // every re-execution fell into the "insert new" branch instead of
        // "replace existing", and results piled up indefinitely. Exercises
        // the full apply -> reparse -> apply-again cycle a real user's
        // repeated block execution goes through.
        let src = "* Café notes\n\n#+begin_src python\nprint(1)\n#+end_src\n";
        let block_end_line = src.lines().position(|l| l.trim() == "#+end_src").unwrap();

        let (s1, e1, t1) = compute_results_edit(src, block_end_line, None, ": 1");
        let after_first = apply_char_edit(src, s1, e1, &t1);
        assert_eq!(
            after_first.matches("#+RESULTS:").count(),
            1,
            "exactly one results block after the first execution"
        );

        // Block itself hasn't moved (only content after it did), so
        // block_end_line is still valid against the mutated source.
        let (s2, e2, t2) = compute_results_edit(&after_first, block_end_line, None, ": 2");
        assert!(
            s2 < e2,
            "second execution must find and replace the existing results block, \
             not append a new one"
        );
        let after_second = apply_char_edit(&after_first, s2, e2, &t2);
        assert_eq!(
            after_second.matches("#+RESULTS:").count(),
            1,
            "still exactly one results block after re-executing — got:\n{after_second}"
        );
        assert!(after_second.contains(": 2"));
        assert!(!after_second.contains(": 1"));
    }

    #[test]
    fn format_table_tsv() {
        let r = format_results("a\tb\n1\t2\n", &ResultsType::Output(ResultsFormat::Table));
        assert!(r.contains("| a"));
        assert!(r.contains("| 1"));
    }

    #[test]
    fn read_results_content_fixed_width() {
        let src = "#+RESULTS:\n: 42\n: hello\n";
        let content = read_results_content(src, 0, 2);
        assert_eq!(content, "42\nhello");
    }

    #[test]
    fn read_results_content_drawer() {
        let src = "#+RESULTS:\n:RESULTS:\nsome output\n:END:\n";
        let content = read_results_content(src, 0, 3);
        assert_eq!(content, "some output");
    }

    #[test]
    fn read_results_content_single_line() {
        let src = "#+RESULTS:\n: 1\n";
        let content = read_results_content(src, 0, 1);
        assert_eq!(content, "1");
    }
}
