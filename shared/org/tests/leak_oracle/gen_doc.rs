//! The generator's documents: the zeroth section (file drawer, keywords,
//! `#+PROPERTY` header arguments, tag selection, the macro library),
//! headlines with property drawers and subtrees, and the postamble.

use super::gen::{Allow, Doc, Gen, EXPORTS};

const TAGS: &[&str] = &["noexport", "ARCHIVE", "t1", "sel", "ex"];

impl Gen {
    /// A body line showing a metadata value to Org (`call` is
    /// `{{{property(K)}}}` or `{{{title}}}`), with a marker of its own: the
    /// value's markers are judged strictly only while Org shows that marker.
    fn exposure_line(&mut self, call: String) -> (u32, String) {
        let n = self.next;
        let m = self.m("exposure");
        (n, format!("P {m} {call}"))
    }

    /// A plain metadata value, allowed in metadata only while `heading`
    /// shows.
    fn meta_value(&mut self, leaf: &'static str, heading: Option<u32>) -> String {
        let m = self.mark(leaf, Allow::Meta(heading));
        format!("v {m}")
    }

    fn header_args(&mut self) -> String {
        let lang = self.pick(&["", ":sh", ":elisp"]);
        let plus = if self.chance(30) { "+" } else { "" };
        let e = self.pick(EXPORTS);
        let e = if e.is_empty() { "none" } else { e };
        format!("header-args{lang}{plus}: :exports {e}")
    }

    /// The headline line: TODO, priority, COMMENT, title, tags.
    fn headline_line(&mut self, level: usize, anchor: &str) -> String {
        let todo = if self.chance(25) {
            self.pick(&["TODO ", "DONE "])
        } else {
            ""
        };
        let prio = if self.chance(15) { "[#A] " } else { "" };
        let comment = if self.chance(12) { "COMMENT " } else { "" };
        let rest = if self.chance(40) {
            String::new()
        } else {
            format!(" {}", self.within("headline-title", Gen::inline))
        };
        if rest.is_empty() && comment.is_empty() {
            self.headings.push(anchor.to_string());
        }
        let mut tags: Vec<&str> = Vec::new();
        while self.chance(25) && tags.len() < 2 {
            tags.push(self.pick(TAGS));
        }
        let tags = if tags.is_empty() {
            String::new()
        } else {
            format!(" :{}:", tags.join(":"))
        };
        let stars = "*".repeat(level);
        format!("{stars} {todo}{prio}{comment}{anchor}{rest}{tags}\n")
    }

    /// The property drawer and the lines exposing its values through
    /// `{{{property}}}` (put in the body).
    fn property_drawer(&mut self, anchor: u32) -> (String, String) {
        let mut d = Vec::new();
        let mut expose = Vec::new();
        if self.chance(70) {
            d.push(format!(":ID: n{}", self.nodes));
            self.nodes += 1;
        }
        if self.chance(25) {
            let cid = if self.chance(50) {
                self.m("custom-id")
            } else {
                format!("c{}", self.next)
            };
            self.custom_ids.push(cid.clone());
            d.push(format!(":CUSTOM_ID: {cid}"));
        }
        self.plain_properties(anchor, &mut d, &mut expose);
        if self.chance(35) {
            let k = format!("Q{}", self.next);
            let (x, line) = self.exposure_line(Gen::call("property", std::slice::from_ref(&k)));
            let v = self.exposed(x, |g| g.within("property-value", Gen::phrases));
            d.push(format!(":{k}: {v}"));
            expose.push(line);
        }
        if self.chance(25) {
            d.push(format!(":{}", self.header_args()));
        }
        if d.is_empty() {
            return (String::new(), String::new());
        }
        let body: String = expose.iter().map(|l| format!("{l}\n")).collect();
        (format!(":PROPERTIES:\n{}\n:END:\n", d.join("\n")), body)
    }

    fn plain_properties(&mut self, anchor: u32, d: &mut Vec<String>, expose: &mut Vec<String>) {
        if self.chance(50) {
            let k = format!("P{}", self.next);
            d.push(format!(
                ":{k}: {}",
                self.meta_value("property", Some(anchor))
            ));
            if self.chance(20) {
                d.push(format!(
                    ":{k}+: {}",
                    self.meta_value("property-plus", Some(anchor))
                ));
            }
            self.props.push(k.clone());
            if self.chance(50) {
                expose.push(self.exposure_line(Gen::call("property", &[k])).1);
            }
        }
        if self.chance(25) {
            if self.chance(50) {
                d.push(format!(
                    ":ALIASES: {}",
                    self.meta_value("alias", Some(anchor))
                ));
            } else {
                let (x, line) = self.exposure_line(Gen::call("property", &["ALIASES".into()]));
                let v = self.exposed(x, |g| g.within("alias-value", Gen::inline));
                d.push(format!(":ALIASES: {v}"));
                expose.push(line);
            }
        }
    }

    /// A headline with its drawer, body and (while not too deep) children.
    pub fn headline(&mut self, level: usize) -> String {
        let anchor_n = self.next;
        let anchor = self.m("headline-anchor");
        let saved = (self.heading, self.props.clone());
        self.heading = Some(anchor_n);
        self.props.clear();
        let mut s = self.headline_line(level, &anchor);
        let (drawer, exposed) = self.property_drawer(anchor_n);
        s.push_str(&drawer);
        s.push_str(&exposed);
        if self.chance(85) {
            s.push_str(&self.within("section", Gen::blocks));
        }
        let children = if level < 3 && !self.deep() {
            self.rng.below(3)
        } else {
            0
        };
        for _ in 0..children {
            s.push_str(&self.within("subtree", |g| g.headline(level + 1)));
        }
        (self.heading, self.props) = saved;
        s
    }

    /// The file's property drawer (it must come first).
    fn file_drawer(&mut self) -> String {
        if !self.chance(50) {
            return String::new();
        }
        let mut d = vec![":ID: n9".to_string()];
        if self.chance(40) {
            d.push(format!(":ALIASES: {}", self.meta_value("file-alias", None)));
        }
        if self.chance(40) {
            d.push(format!(":FP: {}", self.meta_value("file-property", None)));
        }
        format!(":PROPERTIES:\n{}\n:END:\n", d.join("\n"))
    }

    /// `#+TITLE`, `#+aliases`, `#+PROPERTY`, `#+EXCLUDE_TAGS`,
    /// `#+SELECT_TAGS`; the lines exposing the title go to `body`.
    fn zeroth_keywords(&mut self, body: &mut String) -> String {
        let mut s = String::new();
        if self.chance(60) {
            if self.chance(50) {
                s.push_str(&format!(
                    "#+TITLE: {}\n",
                    self.meta_value("doc-title", None)
                ));
            } else {
                let (x, line) = self.exposure_line(Gen::call("title", &[]));
                let t = self.exposed(x, |g| g.within("doc-title", Gen::phrases));
                s.push_str(&format!("#+TITLE: {t}\n"));
                body.push_str(&format!("{line}\n\n"));
            }
        }
        if self.chance(20) {
            s.push_str(&format!(
                "#+aliases: {}\n",
                self.meta_value("doc-alias", None)
            ));
        }
        if self.chance(35) {
            s.push_str(&format!("#+PROPERTY: {}\n", self.header_args()));
        }
        if self.chance(10) {
            s.push_str("#+EXCLUDE_TAGS: ex\n");
        }
        if self.chance(10) {
            s.push_str("#+SELECT_TAGS: sel\n");
        }
        s
    }

    /// The postamble: footnote definitions (rarely in a COMMENT subtree,
    /// rarely missing), and rarely a headline made by a macro.
    fn postamble(&mut self) -> String {
        let mut s = String::new();
        let labels = std::mem::take(&mut self.fn_labels);
        let defined: Vec<String> = labels.into_iter().filter(|_| !self.rng.chance(3)).collect();
        if !defined.is_empty() {
            let comment = if self.chance(5) { "COMMENT " } else { "" };
            s.push_str(&format!("* {comment}Notes {}\n", self.m("notes-title")));
            for l in defined {
                let d = self.within("fn-definition", Gen::phrases);
                s.push_str(&format!("[fn:{l}] {d}\n\n"));
            }
        }
        if self.chance(8) {
            let title = self.m("macro-headline");
            let body = self.m("macro-headline-body");
            s.push_str(&format!("{} {title}\nP {body}\n", Gen::call("hc", &[])));
        }
        s
    }
}

/// The called block every `call_f`/`#+CALL: f` refers to.
fn called_block(g: &mut Gen) -> String {
    let e = g.pick(&["none", "code", "results", "both"]);
    format!("#+NAME: f\n#+begin_src sh :var x=\"\" :exports {e}\necho visible\n#+end_src\n\n")
}

/// Document number `index` of the corpus for `seed`.
pub fn document(seed: u64, index: u64, max_depth: usize) -> Doc {
    let mut g = Gen::new(seed ^ index.wrapping_mul(0x2545_f491_4f6c_dd1d), max_depth);
    let mut s = g.file_drawer();
    let mut zeroth = String::new();
    s.push_str(&g.zeroth_keywords(&mut zeroth));
    let library_first = g.chance(85);
    if library_first {
        s.push_str(&Gen::library());
    }
    s.push('\n');
    s.push_str(&zeroth);
    s.push_str(&called_block(&mut g));
    if g.chance(60) {
        s.push_str(&g.within("zeroth", Gen::blocks));
        s.push('\n');
    }
    for _ in 0..1 + g.rng.below(3) {
        s.push_str(&g.headline(1));
        s.push('\n');
    }
    s.push_str(&g.postamble());
    if !library_first {
        s.push('\n');
        s.push_str(&Gen::library());
    }
    Doc {
        name: format!("g{index:06}"),
        text: s,
        sites: g.sites,
    }
}
