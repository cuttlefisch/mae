//! The explorer (P-39 to P-45, ADR-119 Amendment 3): every published page
//! under each of its tags, untagged pages once, each list A to Z, the same
//! whatever order the sources arrive in.
//!
//! Checked over seeded random KBs rather than one hand-built one: titles
//! that differ only in case or accent, pages with no tag or several, tags
//! that sort differently by bytes and by reader's order, and withheld pages
//! (a duplicate ID) that must appear nowhere.

use std::collections::BTreeMap;

use mae_site::explorer::explorer;
use mae_site::order::cmp_text;
use mae_site::{derive, Addressing, SourcePage};

/// xorshift64*: seeded, so a failure names its seed and reproduces.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const TITLES: &[&str] = &[
    "apple",
    "Apple",
    "\u{c1}rbol",
    "Zebra",
    "\u{cd}ndice",
    "indigo",
    "E\u{301}tude",
    "\u{c9}tude",
    "\u{f1}and\u{fa}",
    "nube",
    "Nota",
    "b",
];
const TAGS: &[&str] = &[
    "zeta",
    "Alpha",
    "alpha",
    "\u{e9}t\u{e9}",
    "beta",
    "_x",
    "%y",
];

/// A random KB: `(sources, the IDs that publish with their tags)`. Every
/// fifth page reuses an earlier page's ID, so both are withheld.
fn kb(rng: &mut Rng) -> (Vec<SourcePage>, BTreeMap<String, Vec<String>>) {
    let n = 1 + rng.below(24);
    let mut sources = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for i in 0..n {
        let id = if i > 0 && rng.below(5) == 0 {
            ids[rng.below(ids.len())].clone()
        } else {
            format!("p{i}")
        };
        let title = TITLES[rng.below(TITLES.len())];
        let mut tags: Vec<&str> = TAGS.iter().copied().filter(|_| rng.below(3) == 0).collect();
        tags.dedup();
        let filetags = if tags.is_empty() {
            String::new()
        } else {
            format!("#+filetags: :{}:\n", tags.join(":"))
        };
        sources.push(SourcePage {
            attachments: Default::default(),
            origin: format!("{i}.org"),
            text: format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {title}\n{filetags}\nBody.\n"),
        });
        ids.push(id);
    }
    // What publishes: IDs held by exactly one source.
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for id in &ids {
        *count.entry(id).or_default() += 1;
    }
    let mut published = BTreeMap::new();
    for (s, id) in sources.iter().zip(&ids) {
        if count[id.as_str()] == 1 {
            let tags = s
                .text
                .lines()
                .find_map(|l| l.strip_prefix("#+filetags: "))
                .map(|t| {
                    t.split(':')
                        .filter(|x| !x.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            published.insert(id.clone(), tags);
        }
    }
    (sources, published)
}

#[test]
fn every_published_page_is_under_each_of_its_tags_once_and_untagged_ones_once() {
    let live = Addressing::live("kb-1").unwrap();
    for seed in 1..=400u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let (mut sources, published) = kb(&mut rng);
        let site = derive(&sources, &live);
        let ex = explorer(&site);

        // Where each page is listed: (group or None for untagged) per ID.
        let mut seen: BTreeMap<String, Vec<Option<String>>> = BTreeMap::new();
        for g in &ex.groups {
            for p in &g.pages {
                seen.entry(p.id.clone())
                    .or_default()
                    .push(Some(g.tag.clone()));
            }
        }
        for p in &ex.untagged {
            seen.entry(p.id.clone()).or_default().push(None);
        }
        assert_eq!(
            seen.keys().collect::<Vec<_>>(),
            published.keys().collect::<Vec<_>>(),
            "seed {seed}: exactly the published pages, withheld ones nowhere"
        );
        for (id, tags) in &published {
            let mut want: Vec<Option<String>> = tags.iter().cloned().map(Some).collect();
            if want.is_empty() {
                want.push(None);
            }
            let mut got = seen[id].clone();
            want.sort();
            got.sort();
            assert_eq!(got, want, "seed {seed}: {id}");
        }
        // Each list A to Z, the groups too; and hrefs are the page's own.
        for w in ex.groups.windows(2) {
            assert!(cmp_text(&w[0].tag, &w[1].tag).is_lt(), "seed {seed}");
        }
        for list in ex.groups.iter().map(|g| &g.pages).chain([&ex.untagged]) {
            for w in list.windows(2) {
                let o = cmp_text(&w[0].title, &w[1].title).then_with(|| w[0].id.cmp(&w[1].id));
                assert!(o.is_lt(), "seed {seed}: {:?} then {:?}", w[0], w[1]);
            }
            for p in list {
                assert_eq!(Some(p.href.clone()), live.page_href(&p.id), "seed {seed}");
            }
        }
        // The same KB in another order is the same explorer.
        sources.reverse();
        assert_eq!(explorer(&derive(&sources, &live)), ex, "seed {seed}");
    }
}

/// The tag index's order is the explorer's: both read `tags_in_order`, and
/// that is A to Z, not by bytes (`Alpha` and `alpha` before `beta`, `été`
/// before `zeta`).
#[test]
fn the_tag_order_is_a_readers_not_the_bytes() {
    let live = Addressing::live("kb-1").unwrap();
    let src = |id: &str, tags: &str| SourcePage {
        attachments: Default::default(),
        origin: format!("{id}.org"),
        text: format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n#+filetags: {tags}\n"),
    };
    let site = derive(
        &[
            src("a", ":zeta:beta:"),
            src("b", ":alpha:\u{e9}t\u{e9}:Alpha:"),
        ],
        &live,
    );
    let order: Vec<&str> = site.tags_in_order().into_iter().map(|(t, _)| t).collect();
    assert_eq!(order, ["Alpha", "alpha", "beta", "\u{e9}t\u{e9}", "zeta"]);
    let groups: Vec<String> = explorer(&site).groups.into_iter().map(|g| g.tag).collect();
    assert_eq!(groups, order);
}
