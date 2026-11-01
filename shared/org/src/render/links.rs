//! The link policy: what a link may point at, and what it shows when it may
//! not (ADR-112 D3).

use super::build::{trim_text, Builder, Mode};
use super::objects::radio_key;
use super::tree::{push, text, Attr, Element, Node, Tag};
use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::link_grammar::{classify_link, LinkKind};
use crate::semantics::plain;
use crate::views::{Link, LinkFormat};
use crate::Span;

/// The schemes an external link may have (ADR-112 D3).
const SCHEMES: &[&str] = &["http", "https", "mailto"];

/// Bytes kept as they are in a URL; every other byte is percent-encoded.
fn url_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&'()*+,;=%".contains(&b)
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if url_safe(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The allowlisted scheme `url` starts with (ASCII case-insensitive, at
/// byte 0: a leading blank or control character is not skipped), lower case.
fn scheme(url: &str) -> Option<&'static str> {
    let (s, rest) = url.split_once(':')?;
    let s = SCHEMES.iter().find(|k| k.eq_ignore_ascii_case(s))?;
    (!rest.is_empty()).then_some(*s)
}

/// `url` as an external link target, if it is one: an `http:`, `https:`
/// or `mailto:` URL with something after the scheme, the scheme lower-cased
/// and every byte outside RFC 3986's unreserved, reserved and `%` set
/// percent-encoded (spaces, quotes, `<`, `>`, backslashes, controls,
/// non-ASCII). `None` for anything else.
pub fn safe_external_url(url: &str) -> Option<String> {
    let s = scheme(url)?;
    Some(format!("{s}:{}", percent_encode(&url[s.len() + 1..])))
}

/// A resolver-supplied href, if it is safe: an `http:`, `https:` or
/// `mailto:` URL, or a reference with NO scheme (its first segment, before
/// `/`, `?` or `#`, holds no `:`) that is neither protocol-relative
/// (`//host`) nor only a fragment (`#x`, which names this page).
/// Refused (`None`), rather than repaired, when empty or when it holds
/// anything a URL must percent-encode (blanks, which a browser strips;
/// quotes; `\`, which a browser reads as `/`; controls; non-ASCII): the
/// caller supplies URLs, already encoded.
///
/// Also refused: a character reference, `&#` or `&NAME;` with NAME letters
/// and digits (`javascript&#58;…`, `&colon;`, `&amp;`). This crate's HTML
/// escapes every `&`, so none is decoded here, but a hast consumer that
/// serialises attribute values without escaping `&` would let the browser
/// decode one into a scheme. A plain query (`?a=1&b=2`) is not affected.
pub fn safe_resolver_href(href: &str) -> Option<String> {
    if href.is_empty() || !href.bytes().all(url_safe) || has_char_ref(href) {
        return None;
    }
    if scheme(href).is_some() {
        return safe_external_url(href);
    }
    let first = href.split(['/', '?', '#']).next().unwrap_or("");
    // `#x` alone is a same-page anchor, which only the page may name: the
    // resolver cannot know it exists.
    let ok = !href.starts_with("//") && !href.starts_with('#') && !first.contains(':');
    ok.then(|| href.to_string())
}

/// Whether `s` holds `&#` or `&NAME;` (NAME letters and digits).
fn has_char_ref(s: &str) -> bool {
    s.match_indices('&').any(|(i, _)| {
        let rest = &s[i + 1..];
        let name = rest.len()
            - rest
                .trim_start_matches(|c: char| c.is_ascii_alphanumeric())
                .len();
        rest.starts_with('#') || (name > 0 && rest[name..].starts_with(';'))
    })
}

/// A src block's or inline src's language name usable in a class:
/// `[A-Za-z0-9][A-Za-z0-9_+#.-]*`, at most 32 bytes.
pub fn is_valid_language(lang: &str) -> bool {
    token(lang, 32, b"_+#.-")
}

/// A special block's name: `[A-Za-z0-9][A-Za-z0-9_-]*`, at most 64 bytes.
pub(crate) fn is_valid_block_name(name: &str) -> bool {
    token(name, 64, b"_-")
}

/// A KB link's `rel=`: `[A-Za-z0-9][A-Za-z0-9_.:-]*`, at most 64 bytes.
pub(crate) fn is_valid_rel(rel: &str) -> bool {
    token(rel, 64, b"_.:-")
}

fn token(s: &str, max: usize, extra: &[u8]) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= max
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// Image extensions an external image link may end in.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "webp", "avif"];

fn is_image(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let last = path.rsplit('/').next().unwrap_or("");
    last.rsplit_once('.')
        .is_some_and(|(_, ext)| IMAGE_EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// What a link becomes.
enum Target {
    /// `<a href>`, with KB metadata for a KB link.
    Href(String, Option<Box<crate::SemanticLink>>),
    Image(String),
    /// No target: the description, or the placeholder.
    None,
    /// Shown as plain text (a link inside another link's description).
    Plain,
}

impl Builder<'_> {
    /// The rendered description of `link` (its objects), trimmed. A link
    /// nested in it (inside emphasis, as Org allows) shows its text under
    /// the same policy but is no `<a>`, and a footnote reference in it
    /// shows nothing: the outer `<a>` must hold no other.
    fn description(&mut self, link: &Link) -> Vec<Node> {
        let mut out = Vec::new();
        let outer = std::mem::replace(&mut self.in_link, true);
        for e in link.description().unwrap_or_default() {
            match e {
                rowan::NodeOrToken::Token(t) => push(&mut out, text(t.text())),
                rowan::NodeOrToken::Node(o) => self.deeper(&o, |b| b.object(&o, &mut out)),
            }
        }
        self.in_link = outer;
        trim_text(&mut out);
        out
    }

    pub fn link(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let Some(link) = Link::cast(n.clone()) else {
            return;
        };
        if link.is_radio() {
            return self.radio_link(&link, out);
        }
        if self.in_value {
            // In a property value only a bracket link is a link: another is
            // its text, as a link nested in a description is. A bracket
            // link's description renders as anywhere.
            let plain = self.in_link || !is_bracket(n);
            let outer = std::mem::replace(&mut self.in_link, plain);
            self.in_value = false;
            self.link(n, out);
            self.in_value = true;
            self.in_link = outer;
            return;
        }
        let class = match self.semantic_link(Span::of(n).start) {
            Some(l) => l.clone(),
            None => fallback(&link),
        };
        // What the link shows is only ever RENDERED text (ADR-112 D3, review
        // B1): its description as rendered, and when that is empty (it had
        // none, or every object in it is hidden) what a link without one
        // shows. Never the description's source text, which would spell out
        // an inline src block, a call or an export snippet the export hides.
        let mut desc = self.description(&link);
        if desc.is_empty() {
            // The store form's description, `D` of `[[T|D]]` (ADR-030): part
            // of the path, never parsed as objects, so it holds no construct
            // to hide. Read from the path alone, not from the semantics'
            // `description`, which is the bracket description's plain text.
            if let Some(d) = fallback(&link).description.filter(|d| !d.is_empty()) {
                desc = vec![text(&d)];
            }
        }
        let target = self.link_target(class, desc.is_empty() && !self.in_link);
        let shown = match &target {
            _ if !desc.is_empty() || self.mode == Mode::Text => desc,
            Target::Href(h, Some(l)) => self.resolved_text(h, l),
            // A same-page link shows what it lands on: the heading's title as
            // rendered (Org shows its number or title), never its own path.
            Target::Href(h, None) if h.starts_with('#') => self.local_text(h),
            Target::Href(h, None) => vec![text(h.strip_prefix("mailto:").unwrap_or(h))],
            _ => self.placeholder(),
        };
        let target = match target {
            // Nested in another link's description: its text, unlinked.
            Target::Href(..) if self.in_link => Target::Plain,
            t => t,
        };
        push(out, link_element(target, shown));
    }

    fn placeholder(&self) -> Vec<Node> {
        if self.mode == Mode::Text {
            return Vec::new();
        }
        vec![text(&self.resolver.placeholder)]
    }

    /// A resolved KB link without a description: the resolver's title; for
    /// a link landing on this page, what it lands on; else the placeholder.
    /// Never the target as written: it is source text.
    fn resolved_text(&self, href: &str, l: &crate::SemanticLink) -> Vec<Node> {
        let title = self
            .resolver
            .targets
            .get(&l.target)
            .and_then(|r| r.title.clone());
        match title {
            Some(t) => vec![text(&t)],
            None if href.starts_with('#') => self.local_text(href),
            None => self.placeholder(),
        }
    }

    /// What a same-page anchor `#ID` lands on, as rendered (a heading's
    /// title, a KB item's title, a target's text: see `Table::shown`); the
    /// placeholder when that is empty or unknown.
    fn local_text(&self, href: &str) -> Vec<Node> {
        let id = href.strip_prefix('#').unwrap_or(href);
        match self.table.shown.get(id).filter(|s| !s.is_empty()) {
            Some(s) => vec![text(s)],
            None => self.placeholder(),
        }
    }

    fn link_target(&mut self, class: crate::SemanticLink, no_desc: bool) -> Target {
        match class.kind {
            LinkKind::Web | LinkKind::Mailto => match safe_external_url(&class.target) {
                Some(u) if no_desc && self.opts.images && u.starts_with("http") && is_image(&u) => {
                    Target::Image(u)
                }
                Some(u) => Target::Href(u, None),
                None => Target::None,
            },
            LinkKind::Kb => self.kb_target(class),
            LinkKind::Heading => self.local_target(|t| t.titles.get(&class.target).cloned()),
            LinkKind::CustomId => self.local_target(|t| t.custom_ids.get(&class.target).cloned()),
            _ => Target::None,
        }
    }

    /// A same-page anchor, if the page has it (never in titles: they may be
    /// shown on another page).
    fn local(&self, find: impl FnOnce(&super::build::Table) -> Option<String>) -> Option<String> {
        if self.mode == Mode::Meta {
            return None;
        }
        find(&self.table)
    }

    fn local_target(&self, find: impl FnOnce(&super::build::Table) -> Option<String>) -> Target {
        self.local(find)
            .map_or(Target::None, |id| Target::Href(format!("#{id}"), None))
    }

    /// A KB link's target. Its `#fragment` names a heading by its title
    /// (ADR-030): it becomes the heading's `h-` anchor ([`super::heading_anchor`]
    /// of the fragment, [`super::slug`] being idempotent), appended to the
    /// resolver's href (percent-encoded: a slug may hold non-ASCII
    /// letters, and a browser decodes a fragment before matching an id),
    /// unless that href already has a fragment, which wins. On this page it
    /// lands on that heading when the page has it, and on the node's own
    /// anchor when it does not (never dropped silently). A fragment with no
    /// letter or digit names nothing.
    fn kb_target(&mut self, l: crate::SemanticLink) -> Target {
        let heading = l.fragment.as_deref().and_then(|f| self.fragment_anchor(f));
        if let Some(href) = self
            .resolver
            .targets
            .get(&l.target)
            .and_then(|r| safe_resolver_href(&r.href))
        {
            let href = match &heading {
                Some(h) if !href.contains('#') => format!("{href}#{}", percent_encode(h)),
                _ => href,
            };
            return Target::Href(href, Some(Box::new(l)));
        }
        // Unresolved: a node on this page, or (Org's fuzzy search, for a
        // link without `id:`) a target or headline of this page.
        let target = l.target.clone();
        let fuzzy = !l.path.to_ascii_lowercase().starts_with("id:");
        match self
            .local(|t| {
                let node = t.nodes.get(&target)?;
                Some(match &heading {
                    Some(h) if t.headings.contains(h) => h.clone(),
                    _ => node.clone(),
                })
            })
            .or_else(|| {
                fuzzy
                    .then(|| {
                        self.local(|t| {
                            t.targets
                                .get(&target)
                                .or_else(|| t.titles.get(&target))
                                .cloned()
                        })
                    })
                    .flatten()
            }) {
            Some(h) => Target::Href(format!("#{h}"), Some(Box::new(l))),
            None => Target::None,
        }
    }

    fn radio_link(&mut self, link: &Link, out: &mut Vec<Node>) {
        let desc = self.description(link);
        let key = radio_key(&link.description_text().unwrap_or_default());
        match self.local(|t| t.radios.get(&key).cloned()) {
            Some(h) if !self.in_link => {
                let a = Element::new(Tag::A)
                    .class("org-radio")
                    .attr(Attr::Href, format!("#{h}"));
                push(out, a.with(desc).into());
            }
            _ => desc.into_iter().for_each(|d| push(out, d)),
        }
    }

    /// A `#+TRANSCLUDE:` line: a link to its member.
    pub fn transclusion(&mut self, n: &SyntaxNode, t: &crate::Transclusion) -> Vec<Node> {
        let value = n
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == VALUE);
        if let Some(v) = value.filter(|v| v.text().trim_start().starts_with("[[")) {
            let base = usize::from(v.text_range().start());
            let tree = crate::green::objects(v.text(), PARAGRAPH);
            if let Some(l) = tree.descendants().find(|c| c.kind() == LINK) {
                let mut out = Vec::new();
                self.with_base(base, |b| b.link(&l, &mut out));
                return out;
            }
        }
        let resolved = self.resolver.targets.get(&t.member);
        let href = resolved.and_then(|r| safe_resolver_href(&r.href));
        match href {
            Some(h) => {
                let shown = resolved
                    .and_then(|r| r.title.clone())
                    .unwrap_or(t.member.clone());
                let a = Element::new(Tag::A).class("org-link").attr(Attr::Href, h);
                vec![a.with(vec![text(&shown)]).into()]
            }
            None => {
                let span = Element::new(Tag::Span).class("org-unresolved");
                vec![span.with(self.placeholder()).into()]
            }
        }
    }
}

fn link_element(target: Target, shown: Vec<Node>) -> Node {
    match target {
        Target::Href(href, kb) => {
            let mut a = Element::new(Tag::A)
                .class("org-link")
                .attr(Attr::Href, href);
            if let Some(l) = kb {
                a = kb_attrs(a, &l);
            }
            a.with(shown).into()
        }
        Target::Image(src) => {
            let alt = src.rsplit('/').next().unwrap_or("").to_string();
            Element::new(Tag::Img)
                .attr(Attr::Src, src)
                .attr(Attr::Alt, alt)
                .into()
        }
        Target::None => Element::new(Tag::Span)
            .class("org-unresolved")
            .with(shown)
            .into(),
        Target::Plain => Element::new(Tag::Span).with(shown).into(),
    }
}

/// `data-org-*` from a KB link's query.
fn kb_attrs(a: Element, l: &crate::SemanticLink) -> Element {
    let mut a = a;
    if is_valid_rel(&l.rel) {
        a = a.attr(Attr::DataOrgRel, l.rel.clone());
    }
    a.attr(Attr::DataOrgW, format!("{}", l.weight))
        .attr(Attr::DataOrgC, format!("{}", l.confidence))
}

/// A link the semantics did not read (a value they exclude, an alias):
/// classified with the default link types.
fn fallback(link: &Link) -> crate::SemanticLink {
    let path = plain::link_path_buffer(link.syntax()).unwrap_or_default();
    let c = classify_link(&path);
    crate::SemanticLink {
        from: None,
        kind: c.kind,
        path: plain::link_path(link.syntax()).unwrap_or_default(),
        target: c.target,
        fragment: c.fragment,
        search: c.search,
        rel: c.rel,
        weight: c.weight,
        confidence: c.confidence,
        attrs: c.attrs,
        description: c.description,
        span: Span::of(link.syntax()),
    }
}

/// Whether a link is a bracket link (the only links in a property value).
pub(crate) fn is_bracket(n: &SyntaxNode) -> bool {
    Link::cast(n.clone()).is_some_and(|l| l.format() == LinkFormat::Bracket)
}
