//! D6's proof: random edit sequences leave a warm `Deriver`'s output equal to
//! a fresh one's after every step, and an unchanged KB re-renders nothing.

use super::*;
use crate::derive::derive_with;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A page: its ID, title, tags, links (ids, with an optional `::#C`), a
/// heading with its own ID and a CUSTOM_ID, and a source block.
#[derive(Clone)]
struct Page {
    id: String,
    title: String,
    tags: Vec<String>,
    links: Vec<(String, bool)>,
    heading: String,
    heading_id: String,
    custom_id: String,
    body: String,
    /// `Some(stored)`: the page links `[[attachment:pic.png]]`, and the host
    /// holds the blob (`true`) or not.
    attachment: Option<bool>,
    /// A `#+TRANSCLUDE:` of this member (MAE's form), if any.
    transclude: Option<String>,
}

impl Page {
    fn text(&self) -> String {
        let mut t = format!(
            ":PROPERTIES:\n:ID: {}\n:END:\n#+title: {}\n",
            self.id, self.title
        );
        if !self.tags.is_empty() {
            t.push_str(&format!("#+filetags: :{}:\n", self.tags.join(":")));
        }
        t.push_str(&format!("\n{}\n\n", self.body));
        if self.attachment.is_some() {
            t.push_str("[[attachment:pic.png]]\n\n");
        }
        if let Some(m) = &self.transclude {
            t.push_str(&format!("#+TRANSCLUDE: {m}\n\n"));
        }
        for (to, custom) in &self.links {
            if *custom {
                t.push_str(&format!("See [[id:{to}::#c-{to}]] and "));
            } else {
                t.push_str(&format!("see [[id:{to}]]. "));
            }
        }
        t.push_str(&format!(
            "\n\n* {}\n:PROPERTIES:\n:ID: {}\n:CUSTOM_ID: {}\n:END:\nText.\n\n#+begin_src rust\nfn main() {{}}\n#+end_src\n",
            self.heading, self.heading_id, self.custom_id
        ));
        t
    }
}

fn page(i: usize) -> Page {
    Page {
        id: format!("p{i}"),
        title: format!("Page {i}"),
        tags: vec![format!("t{}", i % 3)],
        links: vec![],
        heading: format!("Heading {i}"),
        heading_id: format!("h{i}"),
        custom_id: format!("c-p{i}"),
        body: format!("Body of page {i}."),
        attachment: None,
        transclude: None,
    }
}

fn sources(pages: &[Page]) -> Vec<SourcePage> {
    pages
        .iter()
        .map(|p| SourcePage {
            origin: p.id.clone(),
            text: p.text(),
            attachments: match p.attachment {
                Some(true) => [(
                    "pic.png".to_string(),
                    AttachmentState::Resolved(mae_org::ResolvedAttachment::new(
                        format!("/kb/kb/blobs/{}", p.id),
                        "image/png",
                    )),
                )]
                .into(),
                _ => Default::default(),
            },
        })
        .collect()
}

/// One random edit from D6's set: retitle, body, add or remove a link, add,
/// delete or move a page, change tags, retitle a heading, change a heading's
/// ID or CUSTOM_ID, and duplicate an ID (which must withhold, never pick).
fn edit(pages: &mut Vec<Page>, r: &mut Rng, next: &mut usize) {
    let n = pages.len();
    let i = r.below(n.max(1));
    match r.below(13) {
        0 => pages[i].title = format!("Retitled {}", r.next() % 1000),
        1 => pages[i].body = format!("Edited body {}.", r.next() % 1000),
        2 => {
            let to = pages[r.below(n)].id.clone();
            let custom = r.below(2) == 0;
            pages[i].links.push((to, custom));
        }
        3 => {
            if !pages[i].links.is_empty() {
                let k = r.below(pages[i].links.len());
                pages[i].links.remove(k);
            }
        }
        4 => {
            pages.push(page(*next));
            *next += 1;
        }
        5 => {
            if n > 2 {
                pages.remove(i);
            }
        }
        6 => {
            let j = r.below(n);
            pages.swap(i, j);
        }
        7 => pages[i].tags = vec![format!("t{}", r.next() % 5)],
        8 => pages[i].heading = format!("Heading {}", r.next() % 1000),
        9 => {
            let other = pages[r.below(n)].heading_id.clone();
            pages[i].heading_id = if r.below(4) == 0 {
                other
            } else {
                format!("h{}", r.next() % 1000)
            };
        }
        10 => pages[i].custom_id = format!("c-{}", r.next() % 1000),
        11 => {
            pages[i].transclude = match pages[i].transclude {
                None => Some(pages[r.below(n)].id.clone()),
                Some(_) => None,
            }
        }
        // The attachment appears, is stored, goes missing.
        _ => {
            pages[i].attachment = match pages[i].attachment {
                None => Some(false),
                Some(stored) => Some(!stored),
            }
        }
    }
}

/// D6: after every step of random edit sequences, the warm deriver's output
/// equals a fresh derivation's, byte for byte. Kills any input a page's
/// output reads that its fingerprint does not cover.
#[test]
fn a_warm_deriver_matches_a_fresh_one_after_every_edit() {
    let addressing = Addressing::live("kb").unwrap();
    let opts = SiteOptions::default();
    for seed in 1..=12u64 {
        let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut pages: Vec<Page> = (0..8).map(page).collect();
        for (p, page) in pages.iter_mut().enumerate() {
            page.links.push((format!("p{}", (p + 1) % 8), p % 2 == 0));
        }
        let mut next = 8;
        let mut warm = Deriver::default();
        let mut opts = opts.clone();
        for step in 0..40 {
            edit(&mut pages, &mut r, &mut next);
            // Now and then the operator hides a namespace, or stops hiding it.
            if r.below(8) == 0 {
                opts.hidden_namespaces = match opts.hidden_namespaces.is_empty() {
                    true => vec!["p3".into()],
                    false => vec![],
                };
            }
            let src = sources(&pages);
            let got = warm.derive(&src, &addressing, &opts);
            let want = derive_with(&src, &addressing, &opts);
            assert_eq!(got, want, "seed {seed} step {step}");
        }
    }
}

/// An unchanged KB re-renders no page; one page's body edit re-renders that
/// page only; a retitle re-renders it and the pages that link to it.
#[test]
fn only_what_changed_is_rendered() {
    let addressing = Addressing::live("kb").unwrap();
    let opts = SiteOptions::default();
    let mut pages: Vec<Page> = (0..6).map(page).collect();
    pages[1].links.push(("p0".into(), false));
    pages[2].links.push(("p0".into(), false));
    let mut d = Deriver::default();
    d.derive(&sources(&pages), &addressing, &opts);
    assert_eq!(d.last_rendered(), 6);
    d.derive(&sources(&pages), &addressing, &opts);
    assert_eq!(d.last_rendered(), 0, "unchanged");
    pages[4].body = "Changed.".into();
    d.derive(&sources(&pages), &addressing, &opts);
    assert_eq!(d.last_rendered(), 1, "one body");
    pages[0].title = "Renamed".into();
    d.derive(&sources(&pages), &addressing, &opts);
    assert_eq!(d.last_rendered(), 3, "the page and its two linkers");
}

/// The page-local resolver renders every page exactly as the whole KB's
/// resolver did before D6: over the property test's own pages after many
/// edits. Kills a read set that misses a target the page renders.
#[test]
fn a_page_local_resolver_renders_as_the_kb_wide_one() {
    let addressing = Addressing::live("kb").unwrap();
    let opts = SiteOptions::default();
    let mut r = Rng(7);
    let mut pages: Vec<Page> = (0..10).map(page).collect();
    let mut next = 10;
    for _ in 0..200 {
        edit(&mut pages, &mut r, &mut next);
    }
    assert_eq!(
        local_vs_global(&sources(&pages), &addressing, &opts),
        Vec::<String>::new()
    );
}

/// The same over a real-shaped bundle of `.org` files, when one is given:
/// `MAE_SITE_BUNDLE=/path cargo test -p mae-site --release -- --ignored`.
#[test]
#[ignore]
fn a_page_local_resolver_renders_a_bundle_as_the_kb_wide_one() {
    let Ok(root) = std::env::var("MAE_SITE_BUNDLE") else {
        return;
    };
    let mut files = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "org") {
                files.push(p);
            }
        }
    }
    files.sort();
    let src: Vec<SourcePage> = files
        .iter()
        .map(|p| SourcePage {
            origin: p.display().to_string(),
            text: std::fs::read_to_string(p).unwrap(),
            attachments: Default::default(),
        })
        .collect();
    let addressing = Addressing::live("kb").unwrap();
    let differ = local_vs_global(&src, &addressing, &SiteOptions::default());
    eprintln!("pages: {}, differing: {}", src.len(), differ.len());
    assert_eq!(differ, Vec::<String>::new());
}
