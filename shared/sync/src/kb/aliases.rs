//! A node's aliases, as every reader sees them — and org-roam's `ROAM_ALIASES`
//! format, which is where they live in an `org-literal-v1` KB.
//!
//! **The rule** ([`node_aliases`], the one place it is written):
//! - In a KB whose `body_format` is [`BODY_FORMAT_ORG_LITERAL_V1`], a node's
//!   aliases are *derived on read* from its `ROAM_ALIASES` property, stored
//!   verbatim as the file had it. The property is the only writable place for
//!   them; the node's `aliases` array must stay empty, and a value found there
//!   anyway is ignored and reported, never merged.
//! - In every other KB (MAE's own format), the `aliases` array is the source.
//!
//! Nothing copies the property into the array: that would make two writable
//! copies of one fact, and they would disagree the first time either moved.
//! Readers call [`node_aliases`] (or [`KbNodeDoc::aliases_in`]) instead.
//!
//! Lives in mae-sync because the rule is keyed on the KB `body_format` marker and
//! the [`KbNodeDoc`] fields, both defined here, and because every reader depends
//! on this crate: the daemon (`kb/query`, the Cozo projector) directly, and the
//! editor's KB layer through `mae-kb`'s `crdt` feature. mae-org would be the
//! other candidate, but it is rowan-only by ADR-112 D1 and nothing on the
//! daemon's read path depends on it; `ROAM_ALIASES` is a property *value*
//! format, not org syntax.
//!
//! ## `split-string-and-unquote`
//!
//! org-roam reads the property with Emacs' `split-string-and-unquote`, and
//! [`split_string_and_unquote`] is that function, checked against Emacs 30.2 /
//! Org 9.7 (fixtures in `tests/aliases_tests.rs`):
//! - Text outside double quotes splits on runs of whitespace-syntax characters,
//!   empty pieces dropped. That class is Emacs' standard syntax table's, which
//!   `org-mode` does not change: TAB, LF, FF, CR, SPACE, U+00A0, U+2000–U+200B,
//!   U+202F, U+205F and U+3000 — and *not* U+000B or U+0085.
//! - A `"` starts an Emacs Lisp string literal, read up to its closing quote.
//!   No separator is needed around it: `a"b c"d` is `a`, `b c`, `d`, and `""` is
//!   an empty alias, which Emacs keeps.
//! - Inside quotes a backslash escapes as the Lisp reader does: `\"` `\\`, the
//!   letters `a b d e f n r s t v`, `\` before a newline or space (dropped),
//!   octal `\NNN` (1–3 digits), `\xH…` (any number of hex digits), `\uHHHH`,
//!   `\UHHHHHHHH`, `\N{U+H…}`, and any other character standing for itself.
//!
//! ## Malformed input
//!
//! Where Emacs signals an error (an unterminated string, a trailing backslash,
//! `\x` with no hex digit, a short `\u`, a bad `\N{…}`), org-roam's
//! `split-string-and-unquote` call fails and the node gets no aliases from the
//! property. MAE returns the same — no aliases, never a partial guess — and
//! never panics, and never drops it silently: the error is carried as an
//! [`AliasProblem`] which `kb/query.get`/`list` report and the projector logs.
//!
//! A few escapes Emacs *accepts* produce something a Rust `String` cannot hold
//! or that only `\N{NAME}`'s Unicode name table could decode: a raw byte
//! (`\200`–`\377`, `\x80`–`\xff`, `\M-`), a lone surrogate, a code point above
//! U+10FFFF, a key modifier (`\C-`, `\^`, `\S-`, `\H-`, `\A-`) or a character
//! name. Those are refused the same way, as [`UnquoteErrorKind::Unsupported`]: a
//! recorded divergence, surfaced, rather than an approximation.

use std::collections::HashMap;
use std::fmt;

use super::{KbNodeDoc, BODY_FORMAT_ORG_LITERAL_V1};

/// The org-roam property holding a node's aliases in an `org-literal-v1` KB.
/// Matched case-insensitively, as Org matches property names.
pub const ROAM_ALIASES_PROPERTY: &str = "ROAM_ALIASES";

/// Why [`split_string_and_unquote`] could not read its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnquoteError {
    /// Byte offset in the input where the problem starts (the opening quote of an
    /// unterminated string, or the backslash of a bad escape).
    pub offset: usize,
    pub kind: UnquoteErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnquoteErrorKind {
    /// The input ends inside a quoted string. Emacs: `end-of-file`.
    Unterminated,
    /// An escape Emacs itself rejects (`\x` without a hex digit, a short `\u`,
    /// a malformed `\N{…}`). The string names the escape as written.
    InvalidEscape(String),
    /// An escape Emacs accepts but whose result MAE does not reproduce (see the
    /// module docs). The string names the escape as written.
    Unsupported(String),
}

impl fmt::Display for UnquoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            UnquoteErrorKind::Unterminated => {
                write!(
                    f,
                    "unterminated quoted string starting at byte {}",
                    self.offset
                )
            }
            UnquoteErrorKind::InvalidEscape(e) => {
                write!(f, "invalid escape `{e}` at byte {}", self.offset)
            }
            UnquoteErrorKind::Unsupported(e) => write!(
                f,
                "escape `{e}` at byte {} is not supported (Emacs reads it as a raw byte, \
                 a key modifier, a character name or a non-Unicode character)",
                self.offset
            ),
        }
    }
}

impl std::error::Error for UnquoteError {}

/// Emacs' `\s-` class in the standard syntax table (and `org-mode`'s), measured
/// with `char-syntax` over every character on Emacs 30.2.
pub fn is_emacs_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{2000}'
            ..='\u{200B}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

/// Emacs' `(split-string-and-unquote s)`: see the module docs.
pub fn split_string_and_unquote(s: &str) -> Result<Vec<String>, UnquoteError> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '"' {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            out.push(read_lisp_string(s, i, &mut chars)?);
        } else if is_emacs_whitespace(c) {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    Ok(out)
}

type Chars<'a> = std::iter::Peekable<std::str::CharIndices<'a>>;

/// Read the body of a Lisp string whose opening quote is at `open`, consuming
/// through the closing quote.
fn read_lisp_string(s: &str, open: usize, chars: &mut Chars<'_>) -> Result<String, UnquoteError> {
    let unterminated = || UnquoteError {
        offset: open,
        kind: UnquoteErrorKind::Unterminated,
    };
    let mut out = String::new();
    loop {
        let (i, c) = chars.next().ok_or_else(unterminated)?;
        match c {
            '"' => return Ok(out),
            '\\' => {
                let (_, e) = chars.next().ok_or_else(unterminated)?;
                if let Some(ch) = read_escape(s, i, e, chars)? {
                    out.push(ch);
                }
            }
            _ => out.push(c),
        }
    }
}

/// One escape: the backslash at `at`, `e` the character after it. `None` for an
/// escape that reads as nothing (`\` before a newline or a space).
fn read_escape(
    s: &str,
    at: usize,
    e: char,
    chars: &mut Chars<'_>,
) -> Result<Option<char>, UnquoteError> {
    let err = |kind: fn(String) -> UnquoteErrorKind, end: usize| UnquoteError {
        offset: at,
        kind: kind(s[at..end.min(s.len())].to_string()),
    };
    let here = |chars: &mut Chars<'_>| chars.peek().map_or(s.len(), |(i, _)| *i);
    let simple = match e {
        'a' => Some('\u{07}'),
        'b' => Some('\u{08}'),
        'd' => Some('\u{7F}'),
        'e' => Some('\u{1B}'),
        'f' => Some('\u{0C}'),
        'n' => Some('\n'),
        'r' => Some('\r'),
        's' => Some(' '),
        't' => Some('\t'),
        'v' => Some('\u{0B}'),
        '\n' | ' ' => return Ok(None),
        _ => None,
    };
    if simple.is_some() {
        return Ok(simple);
    }
    match e {
        '0'..='7' => {
            let mut v = e.to_digit(8).unwrap_or(0);
            for _ in 0..2 {
                match chars.peek().and_then(|(_, c)| c.to_digit(8)) {
                    Some(d) => {
                        v = v * 8 + d;
                        chars.next();
                    }
                    None => break,
                }
            }
            code_point(v).ok_or_else(|| err(UnquoteErrorKind::Unsupported, here(chars)))
        }
        'x' => {
            let digits = take_hex(chars, usize::MAX);
            if digits.is_empty() {
                return Err(err(UnquoteErrorKind::InvalidEscape, here(chars)));
            }
            let end = here(chars);
            u32::from_str_radix(&digits, 16)
                .ok()
                .and_then(code_point)
                .ok_or_else(|| err(UnquoteErrorKind::Unsupported, end))
        }
        'u' | 'U' => {
            let want = if e == 'u' { 4 } else { 8 };
            let digits = take_hex(chars, want);
            let end = here(chars);
            if digits.len() < want {
                return Err(err(UnquoteErrorKind::InvalidEscape, end));
            }
            u32::from_str_radix(&digits, 16)
                .ok()
                .and_then(char::from_u32)
                .map(Some)
                .ok_or_else(|| err(UnquoteErrorKind::Unsupported, end))
        }
        'N' => read_named(chars).map(Some).map_err(|kind| UnquoteError {
            offset: at,
            kind: kind(s[at..here(chars)].to_string()),
        }),
        '^' => Err(err(UnquoteErrorKind::Unsupported, here(chars))),
        // Key-modifier prefixes. Emacs requires the `-`, and accepts only
        // control, meta and shift inside a string.
        'C' | 'M' | 'S' | 'H' | 'A' => {
            let dash = chars.next_if(|(_, c)| *c == '-').is_some();
            let kind = if dash && matches!(e, 'C' | 'M' | 'S') {
                UnquoteErrorKind::Unsupported
            } else {
                UnquoteErrorKind::InvalidEscape
            };
            Err(err(kind, here(chars)))
        }
        other => Ok(Some(other)),
    }
}

/// A `\` octal or `\x` value as a character, or `None` where Emacs reads a raw
/// byte (0x80–0xFF), a surrogate, or a code point beyond Unicode.
fn code_point(v: u32) -> Option<Option<char>> {
    if (0x80..=0xFF).contains(&v) {
        return None;
    }
    char::from_u32(v).map(Some)
}

fn take_hex(chars: &mut Chars<'_>, max: usize) -> String {
    let mut out = String::new();
    while out.len() < max {
        match chars.peek() {
            Some((_, c)) if c.is_ascii_hexdigit() => {
                out.push(*c);
                chars.next();
            }
            _ => break,
        }
    }
    out
}

/// `\N{U+HEX}` after the `N`. A character name (`\N{LATIN …}`) is unsupported;
/// everything else Emacs rejects is invalid.
fn read_named(chars: &mut Chars<'_>) -> Result<char, fn(String) -> UnquoteErrorKind> {
    if chars.next_if(|(_, c)| *c == '{').is_none() {
        return Err(UnquoteErrorKind::InvalidEscape);
    }
    let mut inner = String::new();
    loop {
        match chars.next() {
            Some((_, '}')) => break,
            // Emacs reads on past the closing quote and reports `end-of-file`;
            // either way the value has no reading.
            Some((_, '"')) | None => return Err(UnquoteErrorKind::InvalidEscape),
            Some((_, c)) => inner.push(c),
        }
    }
    match inner.strip_prefix("U+") {
        Some(hex) if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) => {
            u32::from_str_radix(hex, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or(UnquoteErrorKind::InvalidEscape)
        }
        Some(_) => Err(UnquoteErrorKind::InvalidEscape),
        None if inner
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic()) =>
        {
            Err(UnquoteErrorKind::Unsupported)
        }
        None => Err(UnquoteErrorKind::InvalidEscape),
    }
}

/// Something about a node's aliases a reader should be told about rather than
/// have silently resolved for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasProblem {
    /// The `ROAM_ALIASES` value could not be read; the node reports no aliases.
    Malformed {
        property: String,
        error: UnquoteError,
    },
    /// More than one property key matches `ROAM_ALIASES` case-insensitively
    /// (only reachable by an edit after the seed, which refuses duplicates). The
    /// first in byte order is used, so every reader picks the same one.
    AmbiguousProperty { keys: Vec<String>, used: String },
    /// An `org-literal-v1` node whose `aliases` array is set. The array is not a
    /// source in that format; these values are ignored.
    ArrayIgnored { values: Vec<String> },
}

impl fmt::Display for AliasProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AliasProblem::Malformed { property, error } => write!(
                f,
                ":{property}: cannot be read ({error}); the node has no aliases until it is fixed"
            ),
            AliasProblem::AmbiguousProperty { keys, used } => write!(
                f,
                "several properties name the aliases ({}); :{used}: is used",
                keys.join(", ")
            ),
            AliasProblem::ArrayIgnored { values } => write!(
                f,
                "the node's aliases array is set ({values:?}) but this KB is {BODY_FORMAT_ORG_LITERAL_V1}, \
                 where only :{ROAM_ALIASES_PROPERTY}: holds aliases; the array is ignored"
            ),
        }
    }
}

/// A node's aliases as a reader sees them, and anything wrong with them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeAliases {
    pub aliases: Vec<String>,
    pub problems: Vec<AliasProblem>,
}

/// THE rule (module docs): this node's aliases, given its KB's `body_format`,
/// its `aliases` array and its properties.
pub fn node_aliases(
    body_format: Option<&str>,
    array: &[String],
    properties: &HashMap<String, String>,
) -> NodeAliases {
    if body_format != Some(BODY_FORMAT_ORG_LITERAL_V1) {
        return NodeAliases {
            aliases: array.to_vec(),
            problems: Vec::new(),
        };
    }
    let mut problems = Vec::new();
    if !array.is_empty() {
        problems.push(AliasProblem::ArrayIgnored {
            values: array.to_vec(),
        });
    }
    let mut keys: Vec<&String> = properties
        .keys()
        .filter(|k| k.eq_ignore_ascii_case(ROAM_ALIASES_PROPERTY))
        .collect();
    keys.sort();
    let Some(key) = keys.first().copied() else {
        return NodeAliases {
            aliases: Vec::new(),
            problems,
        };
    };
    if keys.len() > 1 {
        problems.push(AliasProblem::AmbiguousProperty {
            keys: keys.iter().map(|k| k.to_string()).collect(),
            used: key.clone(),
        });
    }
    let aliases = match split_string_and_unquote(&properties[key]) {
        Ok(a) => a,
        Err(error) => {
            problems.push(AliasProblem::Malformed {
                property: key.clone(),
                error,
            });
            Vec::new()
        }
    };
    NodeAliases { aliases, problems }
}

impl KbNodeDoc {
    /// This node's aliases in a KB whose `body_format` is `body_format`
    /// ([`node_aliases`]). [`KbNodeDoc::aliases`] is the raw array, which is not
    /// what a reader should show in an `org-literal-v1` KB.
    pub fn aliases_in(&self, body_format: Option<&str>) -> NodeAliases {
        node_aliases(body_format, &self.aliases(), &self.properties())
    }
}
