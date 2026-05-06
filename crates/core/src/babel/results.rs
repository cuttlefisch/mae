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
/// Returns `(delete_start_byte, delete_end_byte, insert_text)`.
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
        let del_start = line_byte_offset_of(source, results_start);
        let del_end = if results_end + 1 < lines.len() {
            line_byte_offset_of(source, results_end + 1)
        } else {
            source.len()
        };
        (del_start, del_end, format!("{}\n", results_text))
    } else {
        // Insert after #+end_src line
        let insert_at = line_byte_offset_of(source, block_end_line + 1);
        (insert_at, insert_at, format!("\n{}\n", results_text))
    }
}

fn line_byte_offset_of(source: &str, line: usize) -> usize {
    let mut offset = 0;
    for (i, l) in source.lines().enumerate() {
        if i == line {
            return offset;
        }
        offset += l.len() + 1;
    }
    offset.min(source.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::babel::ResultsType;

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

    #[test]
    fn format_table_tsv() {
        let r = format_results("a\tb\n1\t2\n", &ResultsType::Output(ResultsFormat::Table));
        assert!(r.contains("| a"));
        assert!(r.contains("| 1"));
    }
}
