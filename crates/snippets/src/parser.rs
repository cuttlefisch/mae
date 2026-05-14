//! Snippet syntax parser — VSCode/LSP-compatible.
//!
//! Syntax reference: <https://code.visualstudio.com/docs/editor/userdefinedsnippets#_snippet-syntax>
//!
//! Supported:
//! - Tab stops: `$1`, `$0` (final cursor)
//! - Placeholders: `${1:default text}`
//! - Choices: `${1|one,two,three|}`
//! - Escaping: `\$`, `\}`, `\\`
//!
//! Deferred: regex transforms `${1/pattern/replacement/flags}`

use std::fmt;

/// A parsed piece of a snippet template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnippetPart {
    /// Literal text (no special meaning).
    Literal(String),
    /// A tab stop: `$1`, `$2`, etc. `$0` is the final cursor position.
    TabStop { index: u32 },
    /// A placeholder with default text: `${1:default}`.
    /// The default can itself contain nested parts (literals, tab stops).
    Placeholder {
        index: u32,
        default: Vec<SnippetPart>,
    },
    /// A choice menu: `${1|one,two,three|}`. First choice is the default.
    Choice { index: u32, choices: Vec<String> },
}

/// Parse error with position information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub position: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "snippet parse error at {}: {}",
            self.position, self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// Parse a snippet template string into a list of parts.
pub fn parse_snippet(template: &str) -> Result<Vec<SnippetPart>, ParseError> {
    let chars: Vec<char> = template.chars().collect();
    let (parts, pos) = parse_parts(&chars, 0, false)?;
    if pos != chars.len() {
        return Err(ParseError {
            message: "unexpected character".into(),
            position: pos,
        });
    }
    Ok(coalesce_literals(parts))
}

fn parse_parts(
    chars: &[char],
    mut pos: usize,
    in_placeholder: bool,
) -> Result<(Vec<SnippetPart>, usize), ParseError> {
    let mut parts = Vec::new();
    let mut literal = String::new();

    while pos < chars.len() {
        let ch = chars[pos];

        // Inside a placeholder, `}` ends it
        if in_placeholder && ch == '}' {
            if !literal.is_empty() {
                parts.push(SnippetPart::Literal(literal));
            }
            return Ok((parts, pos));
        }

        match ch {
            '\\' if pos + 1 < chars.len() => {
                let next = chars[pos + 1];
                match next {
                    '$' | '}' | '\\' => {
                        literal.push(next);
                        pos += 2;
                    }
                    _ => {
                        literal.push('\\');
                        pos += 1;
                    }
                }
            }
            '$' => {
                if !literal.is_empty() {
                    parts.push(SnippetPart::Literal(literal.clone()));
                    literal.clear();
                }
                pos += 1;
                if pos >= chars.len() {
                    literal.push('$');
                    continue;
                }
                match chars[pos] {
                    '{' => {
                        pos += 1;
                        let (part, new_pos) = parse_braced(chars, pos)?;
                        parts.push(part);
                        pos = new_pos;
                    }
                    c if c.is_ascii_digit() => {
                        let (index, new_pos) = parse_number(chars, pos);
                        parts.push(SnippetPart::TabStop { index });
                        pos = new_pos;
                    }
                    _ => {
                        // Bare `$` followed by non-digit — treat as literal
                        literal.push('$');
                    }
                }
            }
            '\n' => {
                literal.push('\n');
                pos += 1;
            }
            '\t' => {
                literal.push('\t');
                pos += 1;
            }
            _ => {
                literal.push(ch);
                pos += 1;
            }
        }
    }

    if !literal.is_empty() {
        parts.push(SnippetPart::Literal(literal));
    }
    Ok((parts, pos))
}

/// Parse a braced construct: `{1}`, `{1:default}`, `{1|a,b,c|}`.
fn parse_braced(chars: &[char], mut pos: usize) -> Result<(SnippetPart, usize), ParseError> {
    let start = pos;
    // Parse the index number
    if pos >= chars.len() || !chars[pos].is_ascii_digit() {
        return Err(ParseError {
            message: "expected digit after ${".into(),
            position: pos,
        });
    }
    let (index, new_pos) = parse_number(chars, pos);
    pos = new_pos;

    if pos >= chars.len() {
        return Err(ParseError {
            message: "unterminated ${".into(),
            position: start,
        });
    }

    match chars[pos] {
        '}' => {
            pos += 1;
            Ok((SnippetPart::TabStop { index }, pos))
        }
        ':' => {
            pos += 1; // skip ':'
            let (default_parts, new_pos) = parse_parts(chars, pos, true)?;
            pos = new_pos;
            if pos >= chars.len() || chars[pos] != '}' {
                return Err(ParseError {
                    message: "unterminated placeholder".into(),
                    position: start,
                });
            }
            pos += 1; // skip '}'
            Ok((
                SnippetPart::Placeholder {
                    index,
                    default: default_parts,
                },
                pos,
            ))
        }
        '|' => {
            pos += 1; // skip '|'
            let mut choices = Vec::new();
            let mut current = String::new();
            loop {
                if pos >= chars.len() {
                    return Err(ParseError {
                        message: "unterminated choice".into(),
                        position: start,
                    });
                }
                match chars[pos] {
                    '|' => {
                        choices.push(current);
                        pos += 1; // skip closing '|'
                        if pos >= chars.len() || chars[pos] != '}' {
                            return Err(ParseError {
                                message: "expected } after choice |".into(),
                                position: pos,
                            });
                        }
                        pos += 1; // skip '}'
                        break;
                    }
                    ',' => {
                        choices.push(current.clone());
                        current.clear();
                        pos += 1;
                    }
                    '\\' if pos + 1 < chars.len() => {
                        current.push(chars[pos + 1]);
                        pos += 2;
                    }
                    c => {
                        current.push(c);
                        pos += 1;
                    }
                }
            }
            Ok((SnippetPart::Choice { index, choices }, pos))
        }
        '/' => {
            // Regex transform — skip to closing } and treat as plain tab stop
            let mut depth = 0;
            while pos < chars.len() {
                if chars[pos] == '}' && depth == 0 {
                    pos += 1;
                    return Ok((SnippetPart::TabStop { index }, pos));
                }
                if chars[pos] == '{' {
                    depth += 1;
                }
                if chars[pos] == '}' {
                    depth -= 1;
                }
                pos += 1;
            }
            Err(ParseError {
                message: "unterminated transform".into(),
                position: start,
            })
        }
        c => Err(ParseError {
            message: format!("unexpected '{}' in ${{}}", c),
            position: pos,
        }),
    }
}

fn parse_number(chars: &[char], mut pos: usize) -> (u32, usize) {
    let mut n: u32 = 0;
    while pos < chars.len() && chars[pos].is_ascii_digit() {
        n = n
            .saturating_mul(10)
            .saturating_add(chars[pos] as u32 - '0' as u32);
        pos += 1;
    }
    (n, pos)
}

/// Merge adjacent Literal parts.
fn coalesce_literals(parts: Vec<SnippetPart>) -> Vec<SnippetPart> {
    let mut result: Vec<SnippetPart> = Vec::with_capacity(parts.len());
    for part in parts {
        if let SnippetPart::Literal(ref s) = part {
            if let Some(SnippetPart::Literal(ref mut prev)) = result.last_mut() {
                prev.push_str(s);
                continue;
            }
        }
        result.push(part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_tabstop() {
        let parts = parse_snippet("hello $1 world").unwrap();
        assert_eq!(
            parts,
            vec![
                SnippetPart::Literal("hello ".into()),
                SnippetPart::TabStop { index: 1 },
                SnippetPart::Literal(" world".into()),
            ]
        );
    }

    #[test]
    fn parse_braced_tabstop() {
        let parts = parse_snippet("${1}").unwrap();
        assert_eq!(parts, vec![SnippetPart::TabStop { index: 1 }]);
    }

    #[test]
    fn parse_placeholder_with_default() {
        let parts = parse_snippet("${1:foo}").unwrap();
        assert_eq!(
            parts,
            vec![SnippetPart::Placeholder {
                index: 1,
                default: vec![SnippetPart::Literal("foo".into())],
            }]
        );
    }

    #[test]
    fn parse_nested_placeholder() {
        let parts = parse_snippet("${1:hello $2 world}").unwrap();
        assert_eq!(
            parts,
            vec![SnippetPart::Placeholder {
                index: 1,
                default: vec![
                    SnippetPart::Literal("hello ".into()),
                    SnippetPart::TabStop { index: 2 },
                    SnippetPart::Literal(" world".into()),
                ],
            }]
        );
    }

    #[test]
    fn parse_choice() {
        let parts = parse_snippet("${1|one,two,three|}").unwrap();
        assert_eq!(
            parts,
            vec![SnippetPart::Choice {
                index: 1,
                choices: vec!["one".into(), "two".into(), "three".into()],
            }]
        );
    }

    #[test]
    fn parse_escape_dollar() {
        let parts = parse_snippet(r"cost: \$100").unwrap();
        assert_eq!(parts, vec![SnippetPart::Literal("cost: $100".into())]);
    }

    #[test]
    fn parse_escape_backslash() {
        let parts = parse_snippet(r"path: \\n").unwrap();
        assert_eq!(parts, vec![SnippetPart::Literal(r"path: \n".into())]);
    }

    #[test]
    fn parse_final_cursor() {
        let parts = parse_snippet("fn $1($2) {\n\t$0\n}").unwrap();
        assert_eq!(
            parts,
            vec![
                SnippetPart::Literal("fn ".into()),
                SnippetPart::TabStop { index: 1 },
                SnippetPart::Literal("(".into()),
                SnippetPart::TabStop { index: 2 },
                SnippetPart::Literal(") {\n\t".into()),
                SnippetPart::TabStop { index: 0 },
                SnippetPart::Literal("\n}".into()),
            ]
        );
    }

    #[test]
    fn parse_multiple_same_index() {
        let parts = parse_snippet("$1 and $1").unwrap();
        assert_eq!(
            parts,
            vec![
                SnippetPart::TabStop { index: 1 },
                SnippetPart::Literal(" and ".into()),
                SnippetPart::TabStop { index: 1 },
            ]
        );
    }

    #[test]
    fn parse_regex_transform_as_tabstop() {
        // Deferred: parse but treat as plain tabstop
        let parts = parse_snippet("${1/foo/bar/g}").unwrap();
        assert_eq!(parts, vec![SnippetPart::TabStop { index: 1 }]);
    }

    #[test]
    fn parse_bare_dollar_noop() {
        let parts = parse_snippet("$ not a tabstop").unwrap();
        assert_eq!(parts, vec![SnippetPart::Literal("$ not a tabstop".into())]);
    }

    #[test]
    fn parse_empty_template() {
        let parts = parse_snippet("").unwrap();
        assert!(parts.is_empty());
    }

    #[test]
    fn parse_unterminated_brace() {
        let err = parse_snippet("${1").unwrap_err();
        assert!(err.message.contains("unterminated"));
    }

    #[test]
    fn parse_unterminated_placeholder() {
        let err = parse_snippet("${1:hello").unwrap_err();
        assert!(err.message.contains("unterminated"));
    }

    #[test]
    fn parse_real_world_function_snippet() {
        let template = "fn ${1:name}(${2:params}) -> ${3:ReturnType} {\n\t$0\n}";
        let parts = parse_snippet(template).unwrap();
        assert_eq!(parts.len(), 9);
        match &parts[1] {
            SnippetPart::Placeholder { index, default } => {
                assert_eq!(*index, 1);
                assert_eq!(default, &[SnippetPart::Literal("name".into())]);
            }
            _ => panic!("expected placeholder"),
        }
    }
}
