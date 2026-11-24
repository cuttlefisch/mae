//! Images through the resolver's `images` map (ADR-115 D6, Amendment 1
//! E3): a resolved image renders `<img>`, any other state the labelled
//! placeholder, and a target not in the map renders as it always did. Only
//! description-less `attachment:` and `file`-type links whose name ends in
//! an image extension are looked up.

use mae_org::{
    parse, render_hast, render_html, semantics, ImageState, ParseOptions, RenderOptions,
    ResolvedImage, Resolver,
};

fn html_with(doc: &str, resolver: &Resolver, opts: &RenderOptions) -> String {
    let d = parse(doc, &ParseOptions::default());
    render_html(&d, &semantics(&d), opts, resolver)
}

fn html(doc: &str, resolver: &Resolver) -> String {
    html_with(doc, resolver, &RenderOptions::default())
}

fn png(url: &str) -> ImageState {
    ImageState::Resolved(ResolvedImage::new(url, "image/png"))
}

#[test]
fn a_resolved_image_is_an_img_with_its_size_when_known() {
    let r = Resolver::default()
        .with_image("attachment:plain.png", png("/kb/k/blobs/aa"))
        .with_image(
            "attachment:sized.png",
            ImageState::Resolved(
                ResolvedImage::new("/kb/k/blobs/bb", "image/png")
                    .with_width(640)
                    .with_height(480),
            ),
        )
        .with_image(
            "attachment:wide.png",
            ImageState::Resolved(ResolvedImage::new("/b/cc", "image/png").with_width(12)),
        );
    let out = html(
        "[[attachment:plain.png]]\n\n[[attachment:sized.png]]\n\n[[attachment:wide.png]]\n",
        &r,
    );
    assert!(
        out.contains("<p><img src=\"/kb/k/blobs/aa\" alt=\"plain.png\"></p>"),
        "{out}"
    );
    assert!(
        out.contains(
            "<p><img src=\"/kb/k/blobs/bb\" alt=\"sized.png\" width=\"640\" height=\"480\"></p>"
        ),
        "{out}"
    );
    assert!(
        out.contains("<img src=\"/b/cc\" alt=\"wide.png\" width=\"12\">"),
        "{out}"
    );
    assert!(!out.contains("org-unresolved") && !out.contains("org-attachment"));
}

#[test]
fn every_other_state_is_a_labelled_placeholder_never_an_img() {
    let states = [
        (ImageState::Missing, "missing"),
        (ImageState::Forbidden, "forbidden"),
        (ImageState::TooLarge, "too_large"),
        (ImageState::Unsupported, "unsupported"),
    ];
    for (state, name) in states {
        let r = Resolver::default().with_image("attachment:dir/x.png", state);
        let out = html("[[attachment:dir/x.png]]\n", &r);
        assert_eq!(
            out,
            format!("<p><span class=\"org-attachment\" data-state=\"{name}\">dir/x.png</span></p>"),
            "{name}"
        );
        assert!(!out.contains("<img"), "{out}");
    }
}

#[test]
fn a_target_not_in_the_map_renders_as_before() {
    let doc = "[[attachment:x.png]] [[file:y.png]]\n";
    let before = html(doc, &Resolver::default());
    let other = Resolver::default().with_image("attachment:other.png", png("/o"));
    assert_eq!(html(doc, &other), before);
    assert!(before.contains("org-unresolved") && !before.contains("<img"));
}

#[test]
fn a_described_link_is_never_an_image() {
    // Org's rule: a link with a description is a link to the file.
    let r = Resolver::default()
        .with_image("attachment:x.png", png("/x"))
        .with_image("file:y.png", ImageState::Missing);
    let out = html("[[attachment:x.png][see x]] [[file:y.png][see y]]\n", &r);
    assert!(
        !out.contains("<img") && !out.contains("org-attachment"),
        "{out}"
    );
    assert!(out.contains("see x") && out.contains("see y"), "{out}");
}

#[test]
fn file_type_links_are_looked_up_too() {
    // Org's `file' type: `file:`, `file+APP:` in any case, a file name.
    let r = Resolver::default()
        .with_image("file:a.png", png("/a"))
        .with_image("./b.JPG", png("/b"))
        .with_image("FILE+sys:c.webp", png("/c"))
        .with_image("file:d.svg", ImageState::Forbidden)
        .with_image("file:e.png", png("/e"));
    let out = html(
        "[[file:a.png]] [[./b.JPG]] [[FILE+sys:c.webp]] [[file:d.svg]] [[file:e.png::3]]\n",
        &r,
    );
    assert!(out.contains("<img src=\"/a\" alt=\"a.png\">"), "{out}");
    assert!(out.contains("<img src=\"/b\" alt=\"b.JPG\">"), "{out}");
    assert!(out.contains("<img src=\"/c\" alt=\"c.webp\">"), "{out}");
    // A search option makes it no image (Org's export fails on it).
    assert!(!out.contains("src=\"/e\""), "{out}");
    assert!(
        out.contains("<span class=\"org-attachment\" data-state=\"forbidden\">d.svg</span>"),
        "{out}"
    );
}

#[test]
fn a_non_image_name_is_never_looked_up() {
    // In the map or not, a name that does not END in an image extension
    // (registered: image-extension) and any other link type stay links.
    let r = Resolver::default()
        .with_image("attachment:x.pdf", png("/x"))
        .with_image("attachment:y.png.txt", png("/y"))
        .with_image("attachment:z", png("/z"))
        .with_image("ATTACHMENT:w.png", png("/w"))
        .with_image("doi:v.png", png("/v"))
        .with_image("v.png", png("/v2"));
    let out = html(
        "[[attachment:x.pdf]] [[attachment:y.png.txt]] [[attachment:z]] [[ATTACHMENT:w.png]] [[doi:v.png]] [[v.png]]\n",
        &r,
    );
    assert!(
        !out.contains("<img") && !out.contains("org-attachment"),
        "{out}"
    );
}

#[test]
fn a_caption_puts_the_image_in_a_figure() {
    let r = Resolver::default()
        .with_image("attachment:d.png", png("/d"))
        .with_image("attachment:m.png", ImageState::Missing);
    let out = html(
        "#+CAPTION: The diagram\n[[attachment:d.png]]\n\n#+CAPTION: Gone\n[[attachment:m.png]]\n",
        &r,
    );
    assert!(
        out.contains("<figure class=\"org-figure\"><p><img src=\"/d\" alt=\"d.png\"></p><figcaption>The diagram</figcaption></figure>"),
        "{out}"
    );
    assert!(
        out.contains("<figure class=\"org-figure\"><p><span class=\"org-attachment\" data-state=\"missing\">m.png</span></p><figcaption>Gone</figcaption></figure>"),
        "{out}"
    );
}

#[test]
fn a_refused_url_leaves_the_link_unresolved() {
    for bad in [
        "javascript:alert(1)",
        "data:image/png;base64,AAAA",
        "//evil.example/x.png",
        "/a b.png",
        "/x\" onerror=\"y",
    ] {
        let r = Resolver::default().with_image("attachment:x.png", png(bad));
        let out = html("[[attachment:x.png]]\n", &r);
        assert!(!out.contains("<img"), "{bad}: {out}");
        assert!(out.contains("org-unresolved"), "{bad}: {out}");
    }
}

#[test]
fn resolver_images_do_not_depend_on_the_http_images_option() {
    let r = Resolver::default().with_image("attachment:x.png", png("/x"));
    let doc = "[[attachment:x.png]] [[https://h/i.png]]\n";
    let off = html_with(doc, &r, &RenderOptions::default());
    assert!(off.contains("<img src=\"/x\""), "{off}");
    assert!(!off.contains("https://h/i.png\" alt"), "{off}");
    let on = html_with(doc, &r, &RenderOptions::default().with_images(true));
    assert!(on.contains("<img src=\"/x\"") && on.contains("<img src=\"https://h/i.png\""));
}

#[test]
fn the_hast_carries_size_and_state() {
    let r = Resolver::default()
        .with_image(
            "attachment:a.png",
            ImageState::Resolved(
                ResolvedImage::new("/a", "image/png")
                    .with_width(3)
                    .with_height(4),
            ),
        )
        .with_image("attachment:b.png", ImageState::TooLarge);
    let d = parse(
        "[[attachment:a.png]] [[attachment:b.png]]\n",
        &ParseOptions::default(),
    );
    let hast = render_hast(&d, &semantics(&d), &RenderOptions::default(), &r);
    let v: serde_json::Value = serde_json::from_str(&hast).unwrap();
    let img = find(&v, "img").expect("an img");
    assert_eq!(img["properties"]["width"], "3", "{hast}");
    assert_eq!(img["properties"]["height"], "4", "{hast}");
    let span = find(&v, "span").expect("a span");
    assert_eq!(span["properties"]["dataState"], "too_large", "{hast}");
    assert_eq!(
        span["properties"]["className"][0], "org-attachment",
        "{hast}"
    );
    assert_eq!(span["children"][0]["value"], "b.png", "{hast}");
}

/// The first hast element named `tag`, depth first.
fn find<'a>(v: &'a serde_json::Value, tag: &str) -> Option<&'a serde_json::Value> {
    if v["tagName"] == tag {
        return Some(v);
    }
    let kids = v.as_array().or_else(|| v["children"].as_array())?;
    kids.iter().find_map(|k| find(k, tag))
}
