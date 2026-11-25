//! The SVG sanitiser against hostile shapes: CSS escapes that hide a fetch,
//! URL-taking CSS functions, `<use>` chains (deep, wide, cyclic) and nesting
//! deep enough to exhaust a worker's stack.

use super::svg::{sanitize, MAX_DEPTH, MAX_EXPANDED, MAX_USE_DEPTH};

fn svg(body: &str) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{body}</svg>"#)
}

fn kept(body: &str) -> String {
    String::from_utf8(sanitize(svg(body).as_bytes()).expect("accepted")).unwrap()
}

fn refused(body: &str) -> String {
    sanitize(svg(body).as_bytes()).expect_err("refused")
}

/// `\75rl(` is CSS for `url(`: a backslash in a kept attribute drops it,
/// whichever attribute and however the escape is spelled.
#[test]
fn a_css_escape_cannot_hide_a_remote_url() {
    let hostile = [
        r#"fill="\75rl(http://evil.example/a)""#,
        r#"stroke="u\72l(http://evil.example/b)""#,
        r#"mask="\000075rl(http://evil.example/c)""#,
        r#"clip-path="\75 rl(http://evil.example/d)""#,
        r#"marker-start="\55RL(http://evil.example/e)""#,
        r#"marker-end="ur\6c(http://evil.example/f)""#,
        r#"fill="url(#g) \75rl(http://evil.example/g)""#,
    ];
    for attr in hostile {
        let out = kept(&format!(r#"<rect width="1" height="1" {attr}/>"#));
        assert!(!out.contains("evil"), "{attr} survived: {out}");
        assert!(!out.contains('\\'), "{attr}: {out}");
    }
    // A local reference is still kept.
    let ok = kept(r#"<rect width="1" height="1" fill="url(#g)" mask="url( '#m' )"/>"#);
    assert!(ok.contains(r#"fill="url(#g)""#), "{ok}");
    assert!(ok.contains(r#"mask="url( '#m' )""#), "{ok}");
}

/// CSS functions that fetch a URL written as a plain string.
#[test]
fn url_taking_css_functions_are_dropped() {
    for attr in [
        r#"mask="image-set('http://evil.example/a' 1x)""#,
        r#"mask="-webkit-image-set('http://evil.example/b' 1x)""#,
        r#"fill="IMAGE('http://evil.example/c')""#,
        r#"mask="src('http://evil.example/d')""#,
    ] {
        let out = kept(&format!(r#"<rect width="1" height="1" {attr}/>"#));
        assert!(!out.contains("evil"), "{attr} survived: {out}");
    }
}

/// A chain of `n` `<use>`s, each referencing the one before, ending at a
/// rectangle.
fn chain(n: u32) -> String {
    let mut s = String::from(r#"<defs><rect id="u0" width="1" height="1"/>"#);
    for i in 1..=n {
        s += &format!(r##"<use id="u{i}" href="#u{}"/>"##, i - 1);
    }
    s + "</defs>"
}

#[test]
fn a_use_chain_deeper_than_the_limit_is_refused_and_one_at_it_is_kept() {
    kept(&chain(MAX_USE_DEPTH));
    let why = refused(&chain(MAX_USE_DEPTH + 1));
    assert!(why.contains("deeper"), "{why}");
}

/// Few `<use>` elements, each level instantiating the one below ten times: a
/// count of `<use>` elements alone would never see this.
#[test]
fn a_wide_use_fan_out_is_refused_by_what_it_expands_to() {
    let level = |i: u32| {
        let uses = format!(r##"<use href="#l{}"/>"##, i - 1).repeat(10);
        format!(r#"<g id="l{i}">{uses}</g>"#)
    };
    let fan = |levels: u32| {
        let mut s = String::from(r#"<defs><path id="l0" d="M0 0"/>"#);
        for i in 1..=levels {
            s += &level(i);
        }
        s + "</defs>"
    };
    // 10^6 instances from 60 <use> elements, at a depth the chain cap allows.
    let six = fan(6);
    assert_eq!(six.matches("<use").count(), 60);
    const { assert!(6 <= MAX_USE_DEPTH) };
    let why = refused(&six);
    assert!(why.contains("expand"), "{why}");
    // 10^4 is a real diagram's scale, and kept.
    const { assert!(10_000 * 3 < MAX_EXPANDED) };
    kept(&fan(4));
    // A plot with one <use> per marker, ten thousand of them, is kept.
    let markers = r##"<use href="#l0"/>"##.repeat(10_000);
    kept(&format!(
        r#"<defs><path id="l0" d="M0 0"/></defs>{markers}"#
    ));
}

#[test]
fn a_use_that_instantiates_its_own_ancestor_is_refused() {
    refused(r##"<g id="a"><use href="#a"/></g>"##);
    refused(r##"<g id="a"><use href="#b"/></g><g id="b"><use href="#a"/></g>"##);
    refused(r##"<use id="s" href="#s"/>"##);
}

/// A reference to something the sanitiser drops expands to nothing, so it is
/// judged on the tree that is written, and sanitising twice is the same.
#[test]
fn the_use_check_is_on_the_written_tree_and_idempotent() {
    let body = format!(
        r##"<script id="x">alert(1)</script>{}<use href="#x"/>"##,
        chain(MAX_USE_DEPTH)
    );
    let once = sanitize(svg(&body).as_bytes()).unwrap();
    assert_eq!(sanitize(&once).unwrap(), once);
}

fn nest(depth: usize) -> String {
    // The root is level one.
    "<g>".repeat(depth - 1) + "</g>".repeat(depth - 1).as_str()
}

#[test]
fn nesting_deeper_than_the_limit_is_refused() {
    kept(&nest(MAX_DEPTH));
    let why = refused(&nest(MAX_DEPTH + 1));
    assert!(why.contains("nest"), "{why}");
}

#[test]
fn a_hundred_thousand_level_nest_is_refused_not_a_stack_overflow() {
    // Deep enough to overflow a recursive walk on a 2 MiB stack: refused,
    // not a crash. Run on a thread with exactly that stack, as a worker has.
    let deep = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| sanitize(svg(&nest(100_000)).as_bytes()).is_err())
        .unwrap()
        .join()
        .unwrap();
    assert!(deep);
}

/// The deepest recursion the limits allow: a full-depth chain whose every
/// target sits at the bottom of a nest just under the depth limit. Kept, on a
/// worker-sized stack.
#[test]
fn the_deepest_allowed_document_fits_a_workers_stack() {
    let mut body = String::new();
    let open = "<g>".repeat(MAX_DEPTH - 3);
    let close = "</g>".repeat(MAX_DEPTH - 3);
    body += &format!(r#"{open}<rect id="u0" width="1" height="1"/>{close}"#);
    for i in 1..=MAX_USE_DEPTH {
        body += &format!(r##"{open}<use id="u{i}" href="#u{}"/>{close}"##, i - 1);
    }
    let doc = svg(&body);
    let ok = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || sanitize(doc.as_bytes()).is_ok())
        .unwrap()
        .join()
        .unwrap();
    assert!(ok);
}
