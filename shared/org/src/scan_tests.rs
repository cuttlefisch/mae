//! Each recogniser against the regexp it implements, including the near
//! misses that decide Org's structure (oracle-confirmed cases are marked).

use super::*;

#[test]
fn headline_needs_a_space_after_the_stars() {
    assert_eq!(headline_level("* a"), Some(1));
    assert_eq!(headline_level("*** "), Some(3));
    // Oracle (Org 9.7.11): `*\tB`, `*D` and a lone `*` are paragraphs.
    assert_eq!(headline_level("*\tB"), None);
    assert_eq!(headline_level("*D"), None);
    assert_eq!(headline_level("*"), None);
    assert_eq!(headline_level(" * C"), None);
}

#[test]
fn item_bullets_follow_org_item_re() {
    assert_eq!(item_bullet("- a"), Some((0, 1)));
    assert_eq!(item_bullet("  12) a"), Some((2, 5)));
    assert_eq!(item_bullet(" * C"), Some((1, 2)));
    assert_eq!(item_bullet("-"), Some((0, 1)));
    // A column-0 star is never a bullet; letters need org-list-allow-alphabetical.
    assert_eq!(item_bullet("* a"), None);
    assert_eq!(item_bullet("a. x"), None);
    assert_eq!(item_bullet("-a"), None);
    assert_eq!(item_bullet("1.5"), None);
}

#[test]
fn blocks_drawers_and_keywords() {
    let s = "  #+begin_SRC rust";
    let (a, b) = block_begin(s).unwrap();
    assert_eq!(&s[a..b], "SRC");
    assert_eq!(block_begin("#+begin_ x"), None);
    assert_eq!(block_end_name("#+END_src  "), Some("src"));
    assert_eq!(block_end_name("#+end_src x"), None);
    assert!(is_drawer_end(" :end: "));
    assert!(!is_drawer_end(":END: x"));
    let k = "#+TITLE: x";
    let (a, b) = keyword_key(k).unwrap();
    assert_eq!(&k[a..b], "TITLE");
    // Greedy key: the LAST colon of the non-space run ends the key.
    let k = "#+a:b: c";
    let (a, b) = keyword_key(k).unwrap();
    assert_eq!(&k[a..b], "a:b");
    assert_eq!(keyword_key("#+: x"), None);
    assert_eq!(keyword_key("#+é"), None);
}

#[test]
fn affiliated_keywords_and_their_prefix_end() {
    assert_eq!(affiliated("#+NAME: x"), Some(7));
    assert_eq!(affiliated("#+caption[short]: long"), Some(17));
    assert_eq!(affiliated("#+attr_html: :width 3"), Some(12));
    assert_eq!(affiliated("#+RESULT: x"), Some(9));
    assert_eq!(affiliated("#+NAMES: x"), None);
    assert_eq!(affiliated("#+TITLE: x"), None);
    assert_eq!(affiliated("#+ATTR_: x"), None);
}

#[test]
fn tables_rules_and_clock() {
    assert!(is_tableel_rule("+--+---+"));
    assert!(!is_tableel_rule("++"));
    assert!(!is_tableel_rule("+--"));
    assert!(is_horizontal_rule("  -----  "));
    assert!(!is_horizontal_rule("----"));
    assert!(is_clock(
        "CLOCK: [2024-01-02 Tue 10:00]--[2024-01-02 Tue 11:30] =>  1:30"
    ));
    assert!(is_clock("  CLOCK: [2024-01-02 Tue 10:00]"));
    assert!(!is_clock("CLOCK:[2024-01-02 Tue 10:00]"));
    assert!(!is_clock("CLOCK: soon"));
}

#[test]
fn property_lines_and_separators() {
    assert!(is_property_line(":ID: abc"));
    assert!(is_property_line("  :a+: x"));
    assert!(is_property_line(":END:"));
    assert!(!is_property_line(":a:b"));
    assert!(!is_property_line("::"));
    assert!(may_separate(""));
    assert!(may_separate("#+NAME: x"));
    assert!(may_separate("# c"));
    assert!(!may_separate("#c"));
    assert!(may_separate("[fn:1] x"));
    assert!(!may_separate("text"));
    assert!(list_like("*\tB"));
    assert!(list_like("*"));
    assert!(!list_like("*D"));
}
