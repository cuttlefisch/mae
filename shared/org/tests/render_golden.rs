//! Snapshot goldens for the renderers (ADR-112 D3): every fixture in
//! `tests/render/*.org` rendered with the resolver below, checked in as
//! `tests/render/golden/NAME.html` (the body, exact bytes),
//! `NAME.json` ([`Rendered::to_json`] in hast form, pretty-printed for
//! review) and `NAME.txt` ([`text`]).
//!
//! Regenerate after an intended change, then review the diff like code:
//!   MAE_ORG_REGEN_RENDER=1 cargo test -p mae-org --test render_golden

mod support;

use mae_org::{
    parse, render, semantics, text, HeadlineIds, ParseOptions, RenderOptions, Rendered, Resolved,
    Resolver, TreeFormat,
};
use std::path::{Path, PathBuf};

/// The resolver every golden uses: two pages, one with a title, a relative
/// href, and one target whose href is refused (`javascript:`).
pub fn golden_resolver() -> Resolver {
    let mut r = Resolver::default();
    let mut add = |id: &str, href: &str, title: Option<&str>| {
        let mut resolved = Resolved::new(href);
        resolved.title = title.map(str::to_string);
        r.targets.insert(id.to_string(), resolved);
    };
    add("known", "/notes/known.html", None);
    add("titled", "/notes/titled.html", Some("A <Titled> & Node"));
    add("known2", "notes/known2", None);
    add("evil", "javascript:alert(1)", Some("Evil"));
    r
}

/// The options a fixture is rendered with.
fn options_for(name: &str) -> RenderOptions {
    match name {
        "links" => RenderOptions::default().with_images(true),
        "nodes" => RenderOptions::default().with_property_drawers(true),
        "inline" => RenderOptions::default().with_headline_ids(HeadlineIds::Slug),
        _ => RenderOptions::default(),
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/render")
}

fn render_fixture(path: &Path) -> (Rendered, String) {
    let text_in = std::fs::read_to_string(path).expect("fixture");
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let doc = parse(&text_in, &ParseOptions::default());
    let sem = semantics(&doc);
    let out = render(&doc, &sem, &options_for(name), &golden_resolver());
    (out, text(&doc, &sem))
}

fn check(golden: &Path, got: &str, regen: bool, failures: &mut Vec<String>) {
    if regen {
        std::fs::write(golden, got).expect("write golden");
        return;
    }
    match std::fs::read_to_string(golden) {
        Ok(want) if want == got => {}
        Ok(_) => failures.push(format!("{} differs", support::rel(golden))),
        Err(_) => failures.push(format!("{} is missing", support::rel(golden))),
    }
}

#[test]
fn every_fixture_matches_its_goldens() {
    let regen = std::env::var_os("MAE_ORG_REGEN_RENDER").is_some();
    let fixtures = support::org_files(&dir());
    assert!(fixtures.len() >= 8, "fixtures went missing: {fixtures:?}");
    let mut failures = Vec::new();
    for f in &fixtures {
        let name = f.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let (out, txt) = render_fixture(f);
        let gold = dir().join("golden");
        let json: serde_json::Value =
            serde_json::from_str(&out.to_json(TreeFormat::Hast)).expect("valid JSON");
        let pretty = serde_json::to_string_pretty(&json).expect("pretty") + "\n";
        let html = mae_org::to_html(&out.body) + "\n";
        check(
            &gold.join(format!("{name}.html")),
            &html,
            regen,
            &mut failures,
        );
        check(
            &gold.join(format!("{name}.json")),
            &pretty,
            regen,
            &mut failures,
        );
        check(
            &gold.join(format!("{name}.txt")),
            &(txt + "\n"),
            regen,
            &mut failures,
        );
    }
    assert!(
        failures.is_empty(),
        "{failures:#?}\nregenerate with MAE_ORG_REGEN_RENDER=1 and review the diff"
    );
}

/// The same input, options and resolver give the same bytes: twice in one
/// process (a `HashMap` iterated into the output would differ between
/// runs with different seeds, and between two maps in one run).
#[test]
fn rendering_is_deterministic() {
    for f in support::org_files(&dir()) {
        let (a, ta) = render_fixture(&f);
        let (b, tb) = render_fixture(&f);
        assert_eq!(a.to_json(TreeFormat::Hast), b.to_json(TreeFormat::Hast));
        assert_eq!(a.to_json(TreeFormat::Html), b.to_json(TreeFormat::Html));
        assert_eq!(ta, tb);
    }
}
