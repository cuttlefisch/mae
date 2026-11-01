//! The generator: seeded, deterministic small Org documents. Every marker
//! (`qz<n>qz`) is planted at exactly one site, and the site's intent is
//! recorded: its leaf construct, the containers around it, and whether
//! mae-org may show it where Org shows nothing (a plain property value,
//! alias or title: metadata Org never exports).

use super::view::marker;
use std::collections::BTreeMap;

/// SplitMix64: small, seedable, and the same on every platform.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    pub fn chance(&mut self, pct: u64) -> bool {
        self.next_u64() % 100 < pct
    }

    pub fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())]
    }
}

/// Where mae-org may show a marker that Org's export does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allow {
    /// Nowhere.
    Strict,
    /// In metadata channels (and the body with `property_drawers`): a plain
    /// property value, alias or title. With `Some(h)`, only while Org shows
    /// marker `h` (the title of the headline holding it): metadata of a
    /// headline Org hides is hidden too.
    Meta(Option<u32>),
    /// Inside `href` values only (an id link's `#fragment`, by design).
    Href,
    /// Anywhere, while Org shows marker `w` (plain text planted beside it
    /// in the same container): MAE's own syntax for text the author wants
    /// shown (a store-form link's `[[id:X|D]]` description, ADR-030).
    With(u32),
}

/// One planting site.
#[derive(Debug, Clone)]
pub struct Site {
    pub leaf: &'static str,
    pub path: String,
    pub allow: Allow,
    /// The marker of the body line that shows this metadata value to Org
    /// (`P qz…qz {{{property(K)}}}`, `P qz…qz {{{title}}}`): while Org does
    /// not show it (a minimised document, a pruned section), Org never
    /// shows the value, and the site is judged as `Allow::Meta(None)`.
    pub exposure: Option<u32>,
}

pub type Sites = BTreeMap<u32, Site>;

/// A generated document.
#[derive(Debug, Clone)]
pub struct Doc {
    pub name: String,
    pub text: String,
    pub sites: Sites,
}

/// `:exports` values: Org's four, case and typo variants, and none at all.
pub const EXPORTS: &[&str] = &[
    "none", "code", "results", "both", "NONE", "foo", "none foo", "",
];

/// Macros every document defines (name, template). The pairs open and
/// close a construct across arguments, lines or elements.
pub const LIBRARY: &[(&str, &str)] = &[
    ("id", "$1"),
    ("w", "<$1>"),
    ("cat", "$1$2"),
    ("all", "$0"),
    ("hide", "src_sh[:exports none]{echo $1}"),
    ("code", "src_sh[:exports code]{echo $1}"),
    ("snip", "@@html:$1@@"),
    ("res", "{{{results($1)}}}"),
    ("hash", "# $1"),
    ("callm", "#+CALL: f(x=\"$1\")"),
    ("os", "src_sh[:exports none]{echo"),
    ("cs", "}"),
    ("oh", "@@html:"),
    ("ch", "@@"),
    ("of", "[fn::"),
    ("oi", "[cite:@"),
    ("cb", "]"),
    ("ot", "<<"),
    ("ct", ">>"),
    ("ol", "[[https://e.com/l]["),
    ("cl", "]]"),
    ("oc", "#+begin_comment"),
    ("ec", "#+end_comment"),
    ("ob", "#+begin_src sh :exports none"),
    ("eb", "#+end_src"),
    ("ox", "#+begin_export html"),
    ("ex", "#+end_export"),
    ("oq", "#+begin_quote"),
    ("eq", "#+end_quote"),
    ("hc", "* COMMENT"),
    ("nx", ":noexport:"),
    // Glued into a line's first token (`gen_blocks::first_token`).
    ("bc", "_comment"),
    ("wc", "comment"),
    ("kw", "HTML:"),
    ("colon", ":"),
    ("dn", "LOGBOOK:"),
    ("env", "{comment}"),
    ("star", "*"),
    ("dot", "."),
    ("pipe", "|"),
    ("lab", "lz"),
    ("plus", "+"),
    ("en", "END:"),
    ("xh", "export html"),
    ("xe", "export"),
];

/// Inline construct pairs: (label, opener macro, opener, closer macro,
/// closer).
pub const INLINE_PAIRS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "pair:inline-src",
        "os",
        "src_sh[:exports none]{echo",
        "cs",
        "}",
    ),
    ("pair:snippet", "oh", "@@html:", "ch", "@@"),
    ("pair:footnote", "of", "[fn::", "cb", "]"),
    ("pair:cite", "oi", "[cite:@", "cb", "]"),
    ("pair:target", "ot", "<<", "ct", ">>"),
    ("pair:link", "ol", "[[https://e.com/l][", "cl", "]]"),
];

/// Block pairs: (label, opener by macro, opener, closer by macro, closer).
/// The `-glued` ones glue the macro into the line's first token
/// (`#+END{{{bc}}}`, `:{{{en}}}`), so the line's kind is the expansion's.
pub const REGION_PAIRS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "region:comment",
        "{{{oc}}}",
        "#+begin_comment",
        "{{{ec}}}",
        "#+end_comment",
    ),
    (
        "region:src",
        "{{{ob}}}",
        "#+begin_src sh :exports none",
        "{{{eb}}}",
        "#+end_src",
    ),
    (
        "region:export",
        "{{{ox}}}",
        "#+begin_export html",
        "{{{ex}}}",
        "#+end_export",
    ),
    (
        "region:quote",
        "{{{oq}}}",
        "#+begin_quote",
        "{{{eq}}}",
        "#+end_quote",
    ),
    (
        "region:comment-glued",
        "#+BEGIN{{{bc}}}",
        "#+begin_comment",
        "#+END{{{bc}}}",
        "#+end_comment",
    ),
    (
        "region:export-glued",
        "#+BEGIN_{{{xh}}}",
        "#+begin_export html",
        "#+END_{{{xe}}}",
        "#+end_export",
    ),
    (
        "region:drawer-glued",
        ":{{{dn}}}",
        ":LOGBOOK:",
        ":{{{en}}}",
        ":END:",
    ),
    (
        "region:latex-glued",
        "\\begin{{{env}}}",
        "\\begin{comment}",
        "\\end{{{env}}}",
        "\\end{comment}",
    ),
];

/// The generator's state for one document.
pub struct Gen {
    pub rng: Rng,
    pub sites: Sites,
    pub next: u32,
    pub path: Vec<&'static str>,
    pub depth: usize,
    pub max_depth: usize,
    pub targets: Vec<String>,
    pub headings: Vec<String>,
    pub custom_ids: Vec<String>,
    pub fn_labels: Vec<String>,
    pub keywords: Vec<String>,
    pub props: Vec<String>,
    pub nodes: u32,
    /// The title marker of the headline being generated.
    pub heading: Option<u32>,
    /// The exposure marker of the metadata value being generated.
    pub exposure: Option<u32>,
}

impl Gen {
    pub fn new(seed: u64, max_depth: usize) -> Gen {
        Gen {
            rng: Rng::new(seed),
            sites: Sites::new(),
            next: 1,
            path: Vec::new(),
            depth: 0,
            max_depth,
            targets: Vec::new(),
            headings: Vec::new(),
            custom_ids: Vec::new(),
            fn_labels: Vec::new(),
            keywords: Vec::new(),
            props: Vec::new(),
            nodes: 0,
            heading: None,
            exposure: None,
        }
    }

    /// A fresh marker planted at a `leaf` site.
    pub fn mark(&mut self, leaf: &'static str, allow: Allow) -> String {
        let n = self.next;
        self.next += 1;
        let path = self.path.join(">");
        let exposure = self.exposure;
        self.sites.insert(
            n,
            Site {
                leaf,
                path,
                allow,
                exposure,
            },
        );
        marker(n)
    }

    pub fn m(&mut self, leaf: &'static str) -> String {
        self.mark(leaf, Allow::Strict)
    }

    /// Runs `f` with every marker it plants exposed by the line holding
    /// marker `line`.
    pub fn exposed<T>(&mut self, line: u32, f: impl FnOnce(&mut Gen) -> T) -> T {
        let saved = self.exposure.replace(line);
        let r = f(self);
        self.exposure = saved;
        r
    }

    /// Runs `f` inside container `label`, one level deeper.
    pub fn within<T>(&mut self, label: &'static str, f: impl FnOnce(&mut Gen) -> T) -> T {
        self.path.push(label);
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        self.path.pop();
        r
    }

    pub fn deep(&self) -> bool {
        self.depth >= self.max_depth
    }

    pub fn chance(&mut self, pct: u64) -> bool {
        self.rng.chance(pct)
    }

    pub fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        self.rng.pick(xs)
    }

    /// A `{{{name(args)}}}` call.
    pub fn call(name: &str, args: &[String]) -> String {
        if args.is_empty() {
            format!("{{{{{{{name}}}}}}}")
        } else {
            format!("{{{{{{{name}({})}}}}}}", args.join(","))
        }
    }

    /// One inline phrase (Org objects).
    pub fn inline(&mut self) -> String {
        if self.deep() {
            let m = self.m("text");
            return format!("t {m}");
        }
        match self.rng.below(20) {
            0 | 1 => self.text(),
            2 => self.emphasis(),
            3 => self.verbatim(),
            4 | 5 => self.inline_src(),
            6 => self.inline_call(),
            7 => self.snippet(),
            8 => self.results_macro(),
            9 | 10 => self.user_macro(),
            11 => self.builtin_macro(),
            12 | 13 => self.link(),
            14 => self.target(),
            15 => self.footnote(),
            16 => self.citation(),
            17 | 18 => self.inline_pair(),
            _ => self.latex(),
        }
    }

    /// One to three phrases.
    pub fn phrases(&mut self) -> String {
        let n = 1 + self.rng.below(3);
        let parts: Vec<String> = (0..n).map(|_| self.inline()).collect();
        parts.join(" ")
    }

    fn text(&mut self) -> String {
        let m = self.m("text");
        format!("w {m}")
    }

    fn emphasis(&mut self) -> String {
        let c = self.pick(&['*', '/', '_', '+']);
        let inner = self.within("emphasis", Gen::inline);
        format!("{c}e {inner} e{c}")
    }

    fn verbatim(&mut self) -> String {
        if self.chance(50) {
            format!("={}=", self.m("verbatim"))
        } else {
            format!("~{}~", self.m("code"))
        }
    }

    fn exports_header(&mut self) -> (&'static str, String) {
        let e = self.pick(EXPORTS);
        let h = if e.is_empty() {
            String::new()
        } else {
            format!("[:exports {e}]")
        };
        (if e.is_empty() { "inherited" } else { e }, h)
    }

    fn inline_src(&mut self) -> String {
        let (e, hdr) = self.exports_header();
        let lang = self.pick(&["sh", "elisp"]);
        self.within(e, |g| {
            let m = g.m("inline-src");
            let mut s = format!("src_{lang}{hdr}{{echo {m}}}");
            if g.chance(50) {
                let r = g.m("inline-src-results");
                s.push_str(&format!(" {{{{{{results(={r}=)}}}}}}"));
            }
            s
        })
    }

    fn inline_call(&mut self) -> String {
        let (e, hdr) = self.exports_header();
        self.within(e, |g| {
            let m = g.m("inline-call-arg");
            let mut s = format!("call_f{hdr}(x=\"{m}\")");
            if g.chance(60) {
                let r = g.m("inline-call-results");
                s.push_str(&format!(" {{{{{{results(={r}=)}}}}}}"));
            }
            s
        })
    }

    fn snippet(&mut self) -> String {
        let backend = self.pick(&["html", "latex", "md"]);
        format!("@@{backend}:{}@@", self.m("export-snippet"))
    }

    fn results_macro(&mut self) -> String {
        let m = self.m("results-macro");
        Gen::call("results", &[m])
    }

    fn latex(&mut self) -> String {
        format!("\\({}\\)", self.m("latex"))
    }
}
