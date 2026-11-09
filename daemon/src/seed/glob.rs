//! `--include` patterns: a deliberately small, exact glob over repo-relative
//! paths.
//!
//! - A pattern is matched against the WHOLE path, from the repository root.
//! - `/` separates segments. `**` must be a whole segment and matches zero or
//!   more segments, so `notes/**/*.org` matches `notes/a.org` and
//!   `notes/x/y/a.org`.
//! - `*` matches any run of characters inside ONE segment (never a `/`),
//!   including none and including a leading `.`. `?` matches one character
//!   other than `/`.
//! - Everything else is literal and case-sensitive.
//!
//! Anything this does not define (`[...]` classes, `\` escapes, `{a,b}`,
//! a leading or doubled `/`, `**` inside a segment, `.`/`..` segments) is
//! refused when the pattern is parsed, rather than matched by a guess.

/// A parsed `--include` pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    pattern: String,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// `**`: zero or more whole segments.
    AnyDepth,
    /// One segment's pattern: literal characters, `*` and `?`.
    One(Vec<Tok>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    Char(char),
    Star,
    Question,
}

/// A malformed pattern, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobError(pub String);

impl std::fmt::Display for GlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Glob {
    pub fn new(pattern: &str) -> Result<Glob, GlobError> {
        let err = |why: &str| Err(GlobError(format!("--include {pattern:?}: {why}")));
        if pattern.is_empty() {
            return err("empty pattern");
        }
        if let Some(c) = pattern
            .chars()
            .find(|c| matches!(c, '[' | ']' | '\\' | '{' | '}'))
        {
            return err(&format!("'{c}' is not supported (only *, ? and ** are)"));
        }
        let mut segments = Vec::new();
        for seg in pattern.split('/') {
            match seg {
                "" => return err("empty path segment (leading, trailing or doubled '/')"),
                "." | ".." => return err("'.' and '..' segments are not allowed"),
                "**" => segments.push(Segment::AnyDepth),
                s if s.contains("**") => return err("'**' must be a whole path segment"),
                s => segments.push(Segment::One(tokens(s))),
            }
        }
        Ok(Glob {
            pattern: pattern.to_string(),
            segments,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    /// Whether `path` (repo-relative, `/`-separated) matches the whole pattern.
    pub fn matches(&self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').collect();
        let (n, p) = (self.segments.len(), parts.len());
        // ok[i][j]: segments[i..] match parts[j..]. Filled from the end.
        let mut ok = vec![vec![false; p + 1]; n + 1];
        ok[n][p] = true;
        for i in (0..n).rev() {
            for j in (0..=p).rev() {
                ok[i][j] = match &self.segments[i] {
                    Segment::AnyDepth => ok[i + 1][j] || (j < p && ok[i][j + 1]),
                    Segment::One(toks) => {
                        j < p && segment_matches(toks, parts[j]) && ok[i + 1][j + 1]
                    }
                };
            }
        }
        ok[0][0]
    }
}

fn tokens(segment: &str) -> Vec<Tok> {
    segment
        .chars()
        .map(|c| match c {
            '*' => Tok::Star,
            '?' => Tok::Question,
            c => Tok::Char(c),
        })
        .collect()
}

/// Wildcard match of one segment (no `/` in `s`), linear with one backtrack
/// point: the classic two-pointer algorithm.
fn segment_matches(toks: &[Tok], s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let (mut t, mut c) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while c < chars.len() {
        match toks.get(t) {
            Some(Tok::Star) => {
                star = Some((t, c));
                t += 1;
            }
            Some(Tok::Question) => {
                t += 1;
                c += 1;
            }
            Some(Tok::Char(x)) if *x == chars[c] => {
                t += 1;
                c += 1;
            }
            _ => match star {
                Some((st, sc)) => {
                    t = st + 1;
                    c = sc + 1;
                    star = Some((st, sc + 1));
                }
                None => return false,
            },
        }
    }
    toks[t..].iter().all(|k| *k == Tok::Star)
}
