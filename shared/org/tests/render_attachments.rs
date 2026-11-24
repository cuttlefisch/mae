//! Attachment and file links through the resolver's `attachments` map
//! (ADR-115 D6, Amendment 1 E3). Only `attachment:` and `file`-type links
//! without `::search` are looked up. Resolved: a description-less link to
//! an image extension renders `<img>`, any other a link to the URL showing
//! its description, or its NAME without one (ox-html's link to a file). Any
//! other state renders the labelled placeholder showing the same, and a
//! target not in the map renders as it always did.

use mae_org::{
    parse, render_hast, render_html, semantics, AttachmentState, ParseOptions, RenderOptions,
    ResolvedAttachment, Resolver,
};

fn html_with(doc: &str, resolver: &Resolver, opts: &RenderOptions) -> String {
    let d = parse(doc, &ParseOptions::default());
    render_html(&d, &semantics(&d), opts, resolver)
}

fn html(doc: &str, resolver: &Resolver) -> String {
    html_with(doc, resolver, &RenderOptions::default())
}

fn png(url: &str) -> AttachmentState {
    AttachmentState::Resolved(ResolvedAttachment::new(url, "image/png"))
}

fn pdf(url: &str) -> AttachmentState {
    AttachmentState::Resolved(ResolvedAttachment::new(url, "application/pdf"))
}

/// Every state but `resolved`, with its `data-state`.
fn placeholder_states() -> [(AttachmentState, &'static str); 4] {
    [
        (AttachmentState::Missing, "missing"),
        (AttachmentState::Forbidden, "forbidden"),
        (AttachmentState::TooLarge, "too_large"),
        (AttachmentState::Unsupported, "unsupported"),
    ]
}

#[test]
fn a_resolved_image_is_an_img_with_its_size_when_known() {
    let r = Resolver::default()
        .with_attachment("attachment:plain.png", png("/kb/k/blobs/aa"))
        .with_attachment(
            "attachment:sized.png",
            AttachmentState::Resolved(
                ResolvedAttachment::new("/kb/k/blobs/bb", "image/png")
                    .with_width(640)
                    .with_height(480),
            ),
        )
        .with_attachment(
            "attachment:wide.png",
            AttachmentState::Resolved(ResolvedAttachment::new("/b/cc", "image/png").with_width(12)),
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
        (AttachmentState::Missing, "missing"),
        (AttachmentState::Forbidden, "forbidden"),
        (AttachmentState::TooLarge, "too_large"),
        (AttachmentState::Unsupported, "unsupported"),
    ];
    for (state, name) in states {
        let r = Resolver::default().with_attachment("attachment:dir/x.png", state);
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
    let doc = "[[attachment:x.png]] [[file:y.png]] [[attachment:r.pdf]] [[file:s.pdf][S]] [[attachment:t.png][T]]\n";
    let before = html(doc, &Resolver::default());
    let other = Resolver::default()
        .with_attachment("attachment:other.png", png("/o"))
        .with_attachment("attachment:other.pdf", pdf("/p"));
    assert_eq!(html(doc, &other), before);
    assert!(before.contains("org-unresolved") && !before.contains("<img"));
    assert!(!before.contains("<a ") && !before.contains("org-attachment"));
}

#[test]
fn a_described_link_is_never_an_image() {
    // Org's rule: a link with a description is a link to the file, even
    // to an image, even resolved (ox-html: <a href="x.png">see x</a>).
    let r = Resolver::default()
        .with_attachment("attachment:x.png", png("/x"))
        .with_attachment("file:y.png", AttachmentState::Missing);
    let out = html("[[attachment:x.png][see x]] [[file:y.png][see y]]\n", &r);
    assert_eq!(
        out,
        "<p><a class=\"org-link\" href=\"/x\">see x</a> <span class=\"org-attachment\" data-state=\"missing\">see y</span></p>",
    );
}

#[test]
fn a_description_that_renders_empty_still_makes_it_a_link() {
    // Org reads the link's contents, not what they export to: an inline
    // src block with :exports results (its default) renders nothing here,
    // and the link shows its NAME, as a link without one.
    let r = Resolver::default().with_attachment("attachment:x.png", png("/x"));
    let out = html("[[attachment:x.png][src_sh{echo hi}]]\n", &r);
    assert_eq!(
        out, "<p><a class=\"org-link\" href=\"/x\">x.png</a></p>",
        "{out}"
    );
    // The same rule for an http(s) image.
    let on = RenderOptions::default().with_images(true);
    let out = html_with("[[https://h/i.png][src_sh{echo hi}]]\n", &r, &on);
    assert!(!out.contains("<img"), "{out}");
}

#[test]
fn a_described_resolved_link_links_its_rendered_description() {
    // ox-html: <a href="report.pdf">The <b>report</b></a>.
    let r = Resolver::default()
        .with_attachment("attachment:r.pdf", pdf("/kb/k/blobs/r"))
        .with_attachment("file:s.pdf", pdf("/kb/k/blobs/s"))
        .with_attachment("./t.pdf", pdf("/t"))
        .with_attachment("FILE+sys:u.pdf", pdf("/u?dl=1"));
    let out = html(
        "[[attachment:r.pdf][The *report* /v2/]] [[file:s.pdf][S =code=]] [[./t.pdf][T]] [[FILE+sys:u.pdf][U]]\n",
        &r,
    );
    assert_eq!(
        out,
        "<p><a class=\"org-link\" href=\"/kb/k/blobs/r\">The <strong>report</strong> <em>v2</em></a> \
         <a class=\"org-link\" href=\"/kb/k/blobs/s\">S <code class=\"org-verbatim\">code</code></a> \
         <a class=\"org-link\" href=\"/t\">T</a> \
         <a class=\"org-link\" href=\"/u?dl=1\">U</a></p>",
    );
}

#[test]
fn a_described_link_in_any_other_state_shows_its_description() {
    for (state, name) in placeholder_states() {
        for target in ["attachment:dir/r.pdf", "file:dir/r.pdf", "attachment:i.png"] {
            let r = Resolver::default().with_attachment(target, state.clone());
            let out = html(&format!("[[{target}][The *report*]]\n"), &r);
            assert_eq!(
                out,
                format!(
                    "<p><span class=\"org-attachment\" data-state=\"{name}\">The <strong>report</strong></span></p>"
                ),
                "{target} {name}"
            );
        }
    }
}

#[test]
fn a_link_without_a_description_to_a_non_image_shows_its_name() {
    // ox-html: <a href="report.pdf">report.pdf</a>, <a href="./t.pdf">./t.pdf</a>.
    // For attachment: Org shows the file's path on disk; MAE its NAME
    // (registered: file-link-text).
    let r = Resolver::default()
        .with_attachment("attachment:sub/r.pdf", pdf("/r"))
        .with_attachment("file:s.pdf", pdf("/s"))
        .with_attachment("./t.pdf", pdf("/t"))
        .with_attachment("file+sys:u.pdf", pdf("/u"));
    let out = html(
        "[[attachment:sub/r.pdf]] [[file:s.pdf]] [[./t.pdf]] [[file+sys:u.pdf]]\n",
        &r,
    );
    assert_eq!(
        out,
        "<p><a class=\"org-link\" href=\"/r\">sub/r.pdf</a> \
         <a class=\"org-link\" href=\"/s\">s.pdf</a> \
         <a class=\"org-link\" href=\"/t\">./t.pdf</a> \
         <a class=\"org-link\" href=\"/u\">u.pdf</a></p>",
    );
    for (state, name) in placeholder_states() {
        let r = Resolver::default().with_attachment("file:s.pdf", state);
        let out = html("[[file:s.pdf]]\n", &r);
        assert_eq!(
            out,
            format!("<p><span class=\"org-attachment\" data-state=\"{name}\">s.pdf</span></p>"),
        );
    }
}

#[test]
fn a_search_option_or_a_nested_link_is_never_looked_up() {
    let r = Resolver::default()
        .with_attachment("file:r.pdf", pdf("/r"))
        .with_attachment("attachment:x.pdf", pdf("/x"))
        .with_attachment("attachment:thumb.png", png("/thumb"));
    // ::search, described or not, stays unresolved, as before.
    let out = html("[[file:r.pdf::3][page 3]] [[file:r.pdf::*H]]\n", &r);
    assert!(
        !out.contains("href") && !out.contains("org-attachment"),
        "{out}"
    );
    // A plain link in another link's description is its text, unlinked:
    // never a second <a>, never an image (registered: image-in-description).
    let r = r.with_attachment("file:thumb.png", png("/thumb2"));
    let out = html(
        "[[https://e.org][see attachment:x.pdf]] [[attachment:x.pdf][attachment:thumb.png]] [[file:r.pdf][file:thumb.png]]\n",
        &r,
    );
    assert_eq!(out.matches("<a ").count(), 3, "{out}");
    assert!(
        out.contains("<a class=\"org-link\" href=\"/r\">file:thumb.png</a>"),
        "{out}"
    );
    assert!(
        !out.contains("<img") && !out.contains("org-attachment"),
        "{out}"
    );
    assert!(out.contains("<a class=\"org-link\" href=\"/x\">"), "{out}");
    // In a property value only a bracket link is a link: a plain one is
    // its text there, never looked up.
    let doc = "* T\n:PROPERTIES:\n:A: file:r.pdf [[file:r.pdf]]\n:END:\n";
    let d = parse(doc, &ParseOptions::default());
    let opts = RenderOptions::default().with_property_drawers(true);
    let out = render_html(&d, &semantics(&d), &opts, &r);
    assert_eq!(out.matches("href=\"/r\"").count(), 1, "{out}");
}

#[test]
fn file_type_links_are_looked_up_too() {
    // Org's `file' type: `file:`, `file+APP:` in any case, a file name.
    let r = Resolver::default()
        .with_attachment("file:a.png", png("/a"))
        .with_attachment("./b.JPG", png("/b"))
        .with_attachment("FILE+sys:c.webp", png("/c"))
        .with_attachment("file:d.svg", AttachmentState::Forbidden)
        .with_attachment("file:e.png", png("/e"));
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
fn a_non_image_name_is_a_link_never_an_image() {
    // A name that does not END in an image extension (registered:
    // image-extension) is a file link: resolved, a link showing its NAME.
    let r = Resolver::default()
        .with_attachment("attachment:x.pdf", png("/x"))
        .with_attachment("attachment:y.png.txt", png("/y"))
        .with_attachment("attachment:z", png("/z"));
    let out = html(
        "[[attachment:x.pdf]] [[attachment:y.png.txt]] [[attachment:z]]\n",
        &r,
    );
    assert_eq!(
        out,
        "<p><a class=\"org-link\" href=\"/x\">x.pdf</a> \
         <a class=\"org-link\" href=\"/y\">y.png.txt</a> \
         <a class=\"org-link\" href=\"/z\">z</a></p>",
    );
}

#[test]
fn any_other_link_type_is_never_looked_up() {
    // Org's attachment type is case-sensitive; doi: and a bare word are
    // other types, described or not.
    let r = Resolver::default()
        .with_attachment("ATTACHMENT:w.png", png("/w"))
        .with_attachment("doi:v.png", png("/v"))
        .with_attachment("v.png", png("/v2"))
        .with_attachment("ATTACHMENT:w.pdf", pdf("/w3"));
    let out = html(
        "[[ATTACHMENT:w.png]] [[doi:v.png]] [[v.png]] [[ATTACHMENT:w.pdf][W]] [[doi:v.png][V]]\n",
        &r,
    );
    assert!(
        !out.contains("<img") && !out.contains("org-attachment") && !out.contains("href"),
        "{out}"
    );
}

#[test]
fn a_caption_puts_the_image_in_a_figure() {
    let r = Resolver::default()
        .with_attachment("attachment:d.png", png("/d"))
        .with_attachment("attachment:m.png", AttachmentState::Missing);
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
        "#top",
        "/x&#58;y",
    ] {
        let r = Resolver::default()
            .with_attachment("attachment:x.png", png(bad))
            .with_attachment("attachment:r.pdf", pdf(bad));
        let out = html(
            "[[attachment:x.png]] [[attachment:x.png][X]] [[attachment:r.pdf]] [[attachment:r.pdf][R]]\n",
            &r,
        );
        assert!(
            !out.contains("<img") && !out.contains("href"),
            "{bad}: {out}"
        );
        assert_eq!(out.matches("org-unresolved").count(), 4, "{bad}: {out}");
    }
}

#[test]
fn resolver_attachments_do_not_depend_on_the_http_images_option() {
    let r = Resolver::default().with_attachment("attachment:x.png", png("/x"));
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
        .with_attachment(
            "attachment:a.png",
            AttachmentState::Resolved(
                ResolvedAttachment::new("/a", "image/png")
                    .with_width(3)
                    .with_height(4),
            ),
        )
        .with_attachment("attachment:b.png", AttachmentState::TooLarge);
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
