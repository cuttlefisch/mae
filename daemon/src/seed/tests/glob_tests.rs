//! `--include` semantics: `**` crosses directories, `*` and `?` do not.

use crate::seed::Glob;

fn check(pattern: &str, yes: &[&str], no: &[&str]) {
    let g = Glob::new(pattern).unwrap();
    for p in yes {
        assert!(g.matches(p), "{pattern:?} must match {p:?}");
    }
    for p in no {
        assert!(!g.matches(p), "{pattern:?} must NOT match {p:?}");
    }
}

/// The operator's pattern for the publication KB. Kills: a `*` that crosses
/// `/`, a `**` that needs at least one directory, an unanchored match.
#[test]
fn notes_double_star_selects_every_depth_under_notes_only() {
    check(
        "notes/**/*.org",
        &[
            "notes/a.org",
            "notes/x/a.org",
            "notes/x/y/z/a.org",
            "notes/x/.a.org",
        ],
        &[
            "README.org",
            "canary/leak.org",
            "notes.org",
            "notesx/a.org",
            "x/notes/a.org",
            "notes/a.org.bak",
            "notes/a.orgx",
            "Notes/a.org",
            "notes/x/a.txt",
        ],
    );
}

#[test]
fn a_single_star_stays_inside_one_segment() {
    check(
        "*.org",
        &["a.org", ".a.org", ".org"],
        &["x/a.org", "a.org/b"],
    );
    check(
        "notes/*/*.org",
        &["notes/x/a.org"],
        &["notes/a.org", "notes/x/y/a.org"],
    );
    check("notes/*.org", &["notes/a.org"], &["notes/x/a.org"]);
}

#[test]
fn double_star_matches_zero_or_more_segments() {
    check(
        "**/*.org",
        &["a.org", "x/a.org", "x/y/a.org"],
        &["a.txt", "x/a.txt"],
    );
    check("**", &["a", "x/y/z"], &[]);
    check("a/**", &["a", "a/x", "a/x/y"], &["b/x", "ab/x"]);
    check(
        "a/**/b/**/c.org",
        &["a/b/c.org", "a/x/b/y/z/c.org"],
        &["a/c.org", "a/b/x.org"],
    );
}

#[test]
fn question_mark_is_one_character_but_never_a_slash() {
    check(
        "n?/a.org",
        &["nx/a.org", "nñ/a.org"],
        &["n/a.org", "nxy/a.org"],
    );
    check("a?b", &["axb"], &["a/b", "ab"]);
}

/// Anything the semantics do not define is refused, not guessed.
#[test]
fn undefined_syntax_is_refused() {
    for bad in [
        "",
        "notes/**.org",
        "**x/a",
        "[ab].org",
        "a\\*.org",
        "{a,b}.org",
        "/abs.org",
        "a//b",
        "a/",
        "./a",
        "a/../b",
        "a/./b",
    ] {
        assert!(Glob::new(bad).is_err(), "{bad:?} must be refused");
    }
}

/// Worst-case input stays fast (no exponential backtracking).
#[test]
fn many_stars_do_not_backtrack_exponentially() {
    let g = Glob::new("**/**/**/**/**/*a*a*a*a*a*a*a*b").unwrap();
    let path = format!("{}/{}", vec!["x"; 30].join("/"), "a".repeat(200));
    let start = std::time::Instant::now();
    assert!(!g.matches(&path));
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}
