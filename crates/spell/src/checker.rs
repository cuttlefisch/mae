//! Spell checker subprocess communication.

use std::io::Write;
use std::process::{Command, Stdio};

/// A misspelled word with position and suggestions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Misspelling {
    /// The misspelled word.
    pub word: String,
    /// Byte offset in the checked text.
    pub offset: usize,
    /// Length in bytes.
    pub length: usize,
    /// Suggested corrections (may be empty).
    pub suggestions: Vec<String>,
}

/// Available spell-check backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpellBackend {
    Aspell,
    Hunspell,
}

impl std::fmt::Display for SpellBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Aspell => write!(f, "aspell"),
            Self::Hunspell => write!(f, "hunspell"),
        }
    }
}

/// Probe PATH for available spell checker.
pub fn check_available() -> Option<SpellBackend> {
    if command_exists("aspell") {
        Some(SpellBackend::Aspell)
    } else if command_exists("hunspell") {
        Some(SpellBackend::Hunspell)
    } else {
        None
    }
}

/// Check text for misspellings using the given backend.
///
/// Uses pipe mode (`aspell pipe` or `hunspell -a`) which expects one line
/// of text per check operation and returns results in ispell protocol format.
pub fn check_text(text: &str, backend: &SpellBackend) -> Result<Vec<Misspelling>, String> {
    let (cmd, args) = match backend {
        SpellBackend::Aspell => ("aspell", vec!["pipe"]),
        SpellBackend::Hunspell => ("hunspell", vec!["-a"]),
    };

    let mut child = Command::new(cmd)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {}", cmd, e))?;

    let stdin = child.stdin.as_mut().ok_or("no stdin")?;

    // Send text line by line (ispell protocol: one line per check)
    for line in text.lines() {
        // Prefix with ^ to enable terse mode checking (aspell/hunspell)
        writeln!(stdin, "^{}", line).map_err(|e| format!("write error: {}", e))?;
    }

    drop(child.stdin.take()); // close stdin to signal EOF

    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait error: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ispell_output(&stdout, text)
}

/// Parse ispell-protocol output into misspelling structs.
///
/// Ispell protocol lines:
/// - `*` = word is correct
/// - `&` word count offset: suggestion1, suggestion2, ...  (misspelled, has suggestions)
/// - `#` word offset  (misspelled, no suggestions)
/// - empty line = end of line results
fn parse_ispell_output(output: &str, original_text: &str) -> Result<Vec<Misspelling>, String> {
    let mut results = Vec::new();
    let mut line_offset: usize = 0;
    let mut current_line_idx: usize = 0;
    let lines: Vec<&str> = original_text.lines().collect();

    for protocol_line in output.lines() {
        if protocol_line.is_empty() {
            // End of results for current line; advance
            if current_line_idx < lines.len() {
                line_offset += lines[current_line_idx].len() + 1; // +1 for newline
                current_line_idx += 1;
            }
            continue;
        }

        if protocol_line.starts_with('@') {
            // Version banner from aspell/hunspell — skip
            continue;
        }

        if protocol_line.starts_with('*')
            || protocol_line.starts_with('+')
            || protocol_line.starts_with('-')
        {
            // Correct word or other status — skip
            continue;
        }

        if protocol_line.starts_with('&') {
            // & word count offset: suggestion1, suggestion2
            if let Some(m) = parse_ampersand_line(protocol_line, line_offset) {
                results.push(m);
            }
        } else if protocol_line.starts_with('#') {
            // # word offset (no suggestions)
            if let Some(m) = parse_hash_line(protocol_line, line_offset) {
                results.push(m);
            }
        }
    }

    Ok(results)
}

fn parse_ampersand_line(line: &str, line_offset: usize) -> Option<Misspelling> {
    // & word count offset: suggestion1, suggestion2, ...
    let rest = line.strip_prefix("& ")?;
    let mut parts = rest.splitn(4, ' ');
    let word = parts.next()?;
    let _count = parts.next()?;
    let offset_str = parts.next()?.trim_end_matches(':');
    let offset: usize = offset_str.parse().ok()?;
    let suggestions_str = parts.next().unwrap_or("");
    let suggestions: Vec<String> = if suggestions_str.is_empty() {
        Vec::new()
    } else {
        suggestions_str
            .split(", ")
            .map(|s| s.trim().to_string())
            .collect()
    };

    Some(Misspelling {
        word: word.to_string(),
        // ispell offset is 1-based within the line; adjust to 0-based byte offset
        offset: line_offset + offset.saturating_sub(1),
        length: word.len(),
        suggestions,
    })
}

fn parse_hash_line(line: &str, line_offset: usize) -> Option<Misspelling> {
    // # word offset
    let rest = line.strip_prefix("# ")?;
    let mut parts = rest.splitn(2, ' ');
    let word = parts.next()?;
    let offset: usize = parts.next()?.parse().ok()?;

    Some(Misspelling {
        word: word.to_string(),
        offset: line_offset + offset.saturating_sub(1),
        length: word.len(),
        suggestions: Vec::new(),
    })
}

fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ampersand() {
        let m = parse_ampersand_line("& teh 3 1: the, tea, ten", 0).unwrap();
        assert_eq!(m.word, "teh");
        assert_eq!(m.offset, 0);
        assert_eq!(m.length, 3);
        assert_eq!(m.suggestions, vec!["the", "tea", "ten"]);
    }

    #[test]
    fn parse_ampersand_with_offset() {
        let m = parse_ampersand_line("& wrold 2 10: world, would", 50).unwrap();
        assert_eq!(m.word, "wrold");
        assert_eq!(m.offset, 59); // 50 + 10 - 1
        assert_eq!(m.suggestions, vec!["world", "would"]);
    }

    #[test]
    fn parse_hash_no_suggestions() {
        let m = parse_hash_line("# xyzzy 5", 0).unwrap();
        assert_eq!(m.word, "xyzzy");
        assert_eq!(m.offset, 4); // 5 - 1
        assert!(m.suggestions.is_empty());
    }

    #[test]
    fn parse_full_output() {
        let output = "@(#) International Ispell Version 3.1.20\n\
                       *\n\
                       & teh 3 5: the, tea, ten\n\
                       *\n\
                       \n";
        let original = "good teh word";
        let results = parse_ispell_output(output, original).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].word, "teh");
    }

    #[test]
    fn parse_empty_output() {
        let results = parse_ispell_output("", "").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn backend_display() {
        assert_eq!(SpellBackend::Aspell.to_string(), "aspell");
        assert_eq!(SpellBackend::Hunspell.to_string(), "hunspell");
    }

    #[test]
    fn check_available_returns_something_or_none() {
        // Just verify it doesn't panic
        let _ = check_available();
    }
}
