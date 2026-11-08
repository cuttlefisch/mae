//! Events to a rowan green tree. Headline lines are cut here, once the
//! document's TODO keywords are known, and inline objects are parsed here.

use crate::headline;
use crate::inline::{self, radio::Radio, InlineCx};
use crate::kind::{OrgLanguage, SyntaxKind};
use crate::options::TodoKeywords;
use crate::span::ContentsTable;
use crate::tokens::Event;
use rowan::{GreenNode, GreenToken, Language, NodeOrToken};

/// The tree of `events`, and the radio targets it was parsed with (in
/// regexp order). `given`: the targets to use (a fragment's file's), or
/// `None` to find them in this text.
pub(crate) fn to_green(
    cx: &InlineCx,
    events: &[Event],
    todo: &TodoKeywords,
    contents: &mut ContentsTable,
    given: Option<&[String]>,
) -> (GreenNode, Vec<String>) {
    // Radio targets are found with no radio links active, as Org finds them
    // when the buffer is set up; then every container is parsed with them.
    let radio = match given {
        Some([]) => None,
        Some(values) => {
            let values: Vec<&str> = values.iter().map(String::as_str).collect();
            Some(Radio::build(cx, &values))
        }
        None => Radio::find(cx, |cx, table| {
            expand(cx, events, todo, table, &mut |_| {});
        }),
    };
    let targets = radio
        .as_ref()
        .map_or_else(Vec::new, |r| r.values().to_vec());
    let cx = &InlineCx {
        radio: radio.as_ref(),
        ..*cx
    };
    let mut b = Builder::default();
    let text = cx.src.text;
    expand(cx, events, todo, contents, &mut |ev| {
        simple(&mut b, text, ev)
    });
    if let Some(from) = radio.as_ref().and_then(Radio::exhausted_at) {
        cx.diagnostics.borrow_mut().push(crate::Diagnostic::new(
            crate::DiagnosticKind::RadioLinkBudget,
            from,
            text.len(),
        ));
    }
    (b.finish(), targets)
}

/// The events with every headline line cut and every object container
/// parsed, handed to `sink` in order.
fn expand(
    cx: &InlineCx,
    events: &[Event],
    todo: &TodoKeywords,
    contents: &mut ContentsTable,
    sink: &mut dyn FnMut(&Event),
) {
    let text = cx.src.text;
    let mut objects = Vec::new();
    for ev in events {
        match *ev {
            Event::HeadlineLine(s, e, budget) => {
                for (k, a, z) in headline::tokens(&text[s..e], todo) {
                    if k == SyntaxKind::TITLE {
                        objects.clear();
                        objects.push(Event::Start(SyntaxKind::HEADLINE_TITLE));
                        inline::parse(
                            cx,
                            s + a,
                            s + z,
                            SyntaxKind::HEADLINE_TITLE,
                            budget,
                            &mut objects,
                            contents,
                        );
                        objects.push(Event::Finish);
                        objects.iter().for_each(&mut *sink);
                    } else {
                        sink(&Event::Token(k, s + a, s + z));
                    }
                }
            }
            Event::Inline(s, e, container, budget) => {
                objects.clear();
                inline::parse(cx, s, e, container, budget, &mut objects, contents);
                objects.iter().for_each(&mut *sink);
            }
            ref other => sink(other),
        }
    }
}

/// The default link types, longest first (as the lexer wants them), made
/// once: [`objects`] runs for every keyword and property value.
fn default_link_types() -> &'static [String] {
    static TYPES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    TYPES.get_or_init(|| {
        let mut types: Vec<String> = crate::DEFAULT_LINK_TYPES
            .iter()
            .map(|t| t.to_string())
            .collect();
        types.sort_by_key(|t| std::cmp::Reverse(t.len()));
        types
    })
}

/// `text` parsed as the objects of a `container` (as `org-element` parses a
/// secondary string such as a keyword value), under a node of that kind.
/// Link types are the default ones: they only decide which `type:path` text
/// is a plain link, so the plain-text reading of the result is the same for
/// any set (a plain link reads as its path either way).
pub(crate) fn objects(text: &str, container: SyntaxKind) -> crate::SyntaxNode {
    let src = crate::text::Source::new(text);
    let ix = inline::InlineIndex::build(&src);
    let link_types = default_link_types();
    // A secondary string's diagnostics have no place in a document.
    let diagnostics = std::cell::RefCell::new(Vec::new());
    let cx = InlineCx {
        src: &src,
        ix: &ix,
        link_types,
        // A radio link reads as its own text, so a plain-text value is the
        // same without the document's radio targets.
        radio: None,
        radio_limit: crate::ParseOptions::ORG_TARGET_LINK_REGEXP_LIMIT,
        diagnostics: &diagnostics,
    };
    let mut events = vec![Event::Start(container)];
    let mut contents = ContentsTable::default();
    let budget = crate::DEFAULT_MAX_DEPTH;
    inline::parse(
        &cx,
        0,
        text.len(),
        container,
        budget,
        &mut events,
        &mut contents,
    );
    events.push(Event::Finish);
    let mut b = Builder::default();
    events.iter().for_each(|e| simple(&mut b, text, e));
    crate::SyntaxNode::new_root(b.finish())
}

/// Builds a green tree from start, token and finish calls, as rowan's
/// `GreenNodeBuilder` does, but shares equal short tokens through
/// [`TokenCache`] rather than rowan's node cache (a hash table over every
/// token and small node, a sizeable part of every parse).
#[derive(Default)]
struct Builder {
    /// Open nodes: their kind and where their children start in `children`.
    parents: Vec<(rowan::SyntaxKind, usize)>,
    children: Vec<NodeOrToken<GreenNode, GreenToken>>,
    tokens: TokenCache,
}

/// Recently made short tokens, one per slot: a token whose slot holds an
/// equal one shares it; otherwise it is made and takes the slot. A lookup
/// is one comparison, with no probing and no table to grow; two tokens
/// that contend for a slot are each made rather than shared. Most repeated
/// tokens (line endings, blanks, markup delimiters, drawer lines) are short.
struct TokenCache {
    slots: Vec<Option<GreenToken>>,
}

impl TokenCache {
    const SLOTS: usize = 1024;
    /// Longer tokens are rarely equal: they are made without a lookup.
    const MAX_LEN: usize = 24;

    fn get(&mut self, kind: rowan::SyntaxKind, text: &str) -> GreenToken {
        if text.len() > Self::MAX_LEN {
            return GreenToken::new(kind, text);
        }
        // FNV-1a over the kind and the text: a spread over the slots.
        let mut h: u32 = 0x811c_9dc5;
        for b in kind.0.to_le_bytes().iter().chain(text.as_bytes()) {
            h = (h ^ u32::from(*b)).wrapping_mul(0x0100_0193);
        }
        let slot = &mut self.slots[h as usize % Self::SLOTS];
        match slot {
            Some(t) if t.kind() == kind && t.text() == text => t.clone(),
            _ => slot.insert(GreenToken::new(kind, text)).clone(),
        }
    }
}

impl Default for TokenCache {
    fn default() -> TokenCache {
        TokenCache {
            slots: vec![None; Self::SLOTS],
        }
    }
}

impl Builder {
    fn start_node(&mut self, kind: rowan::SyntaxKind) {
        self.parents.push((kind, self.children.len()));
    }

    fn token(&mut self, kind: rowan::SyntaxKind, text: &str) {
        let token = self.tokens.get(kind, text);
        self.children.push(NodeOrToken::Token(token));
    }

    fn finish_node(&mut self) {
        let (kind, first) = self.parents.pop().expect("a node to finish");
        let node = GreenNode::new(kind, self.children.drain(first..));
        self.children.push(NodeOrToken::Node(node));
    }

    /// The root: the one node left once every node is finished.
    fn finish(mut self) -> GreenNode {
        assert!(self.parents.is_empty(), "unfinished nodes");
        match (self.children.pop(), self.children.is_empty()) {
            (Some(NodeOrToken::Node(root)), true) => root,
            _ => panic!("a green tree has one root node"),
        }
    }
}

fn simple(b: &mut Builder, text: &str, ev: &Event) {
    let raw = OrgLanguage::kind_to_raw;
    match *ev {
        Event::Start(k) => b.start_node(raw(k)),
        Event::Finish => b.finish_node(),
        Event::Token(k, s, e) => b.token(raw(k), &text[s..e]),
        Event::HeadlineLine(..) | Event::Inline(..) => {}
    }
}

#[cfg(test)]
mod builder_tests {
    use super::*;
    use crate::kind::SyntaxKind::{PARAGRAPH, TEXT};
    use proptest::prelude::*;

    /// The events of a parse, built by rowan's builder and by ours.
    fn both(events: &[Event], text: &str) -> (GreenNode, GreenNode) {
        let mut ours = Builder::default();
        let mut theirs = rowan::GreenNodeBuilder::new();
        let raw = OrgLanguage::kind_to_raw;
        for ev in events {
            simple(&mut ours, text, ev);
            match *ev {
                Event::Start(k) => theirs.start_node(raw(k)),
                Event::Finish => theirs.finish_node(),
                Event::Token(k, s, e) => theirs.token(raw(k), &text[s..e]),
                Event::HeadlineLine(..) | Event::Inline(..) => {}
            }
        }
        (ours.finish(), theirs.finish())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        /// The same tree, node for node and token for token, as rowan's
        /// builder makes from the same events (objects of generated text).
        #[test]
        fn the_builder_builds_what_rowans_does(
            text in "(x|é|[ \t]|\n|\\*b\\* |=v= |\\[\\[a\\]\\[d\\]\\] |\\[fn:1\\] |<<t>> |a_b |\\$x\\$ |---|\\.\\.\\.){0,40}"
        ) {
            let src = crate::text::Source::new(&text);
            let ix = inline::InlineIndex::build(&src);
            let diagnostics = std::cell::RefCell::new(Vec::new());
            let cx = InlineCx {
                src: &src,
                ix: &ix,
                link_types: default_link_types(),
                radio: None,
                radio_limit: crate::ParseOptions::ORG_TARGET_LINK_REGEXP_LIMIT,
                diagnostics: &diagnostics,
            };
            let mut events = vec![Event::Start(PARAGRAPH)];
            let mut contents = ContentsTable::default();
            let depth = crate::DEFAULT_MAX_DEPTH;
            inline::parse(&cx, 0, text.len(), PARAGRAPH, depth, &mut events, &mut contents);
            events.push(Event::Finish);
            let (ours, theirs) = both(&events, &text);
            prop_assert_eq!(ours, theirs);
        }
    }

    /// Two tokens contending for one slot each come back as themselves,
    /// alternately, however often.
    #[test]
    fn a_contended_slot_returns_each_token_as_itself() {
        let kind = OrgLanguage::kind_to_raw(TEXT);
        let slot = |kind: rowan::SyntaxKind, t: &str| {
            let mut h: u32 = 0x811c_9dc5;
            for b in kind.0.to_le_bytes().iter().chain(t.as_bytes()) {
                h = (h ^ u32::from(*b)).wrapping_mul(0x0100_0193);
            }
            h as usize % TokenCache::SLOTS
        };
        let first = "a0".to_string();
        let other = (1..100_000)
            .map(|i| format!("a{i}"))
            .find(|t| slot(kind, t) == slot(kind, &first))
            .expect("a second text for the slot");
        // (One text under two kinds never shares a slot: the slot is the
        // hash's low bits, which each byte maps one to one. The kind is
        // compared anyway.)
        let mut cache = TokenCache::default();
        for _ in 0..3 {
            for (k, t) in [(kind, &first), (kind, &other)] {
                let got = cache.get(k, t);
                assert_eq!((got.kind(), got.text()), (k, t.as_str()));
            }
        }
        // A token longer than the cache keeps is made as written.
        let long = "x".repeat(TokenCache::MAX_LEN + 1);
        assert_eq!(cache.get(kind, &long).text(), long);
    }
}
