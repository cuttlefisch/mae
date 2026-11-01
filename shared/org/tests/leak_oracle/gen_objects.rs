//! The generator's containers of objects: macros, links, targets,
//! footnotes, citations and construct pairs opened and closed by macros.

use super::gen::{Allow, Gen, INLINE_PAIRS, LIBRARY};

/// Odd `&w=` values.
const WEIGHTS: &[&str] = &["1e9;x", "-1", "0.5", "NaN", "2", "1,5", "x"];

impl Gen {
    /// A call of a library macro with generated arguments.
    pub fn user_macro(&mut self) -> String {
        let name = self.pick(&["id", "w", "cat", "all", "hide", "code", "snip", "res"]);
        let n = if matches!(name, "cat" | "all") { 2 } else { 1 };
        self.within("macro-arg", |g| {
            let args: Vec<String> = (0..n).map(|_| g.inline()).collect();
            Gen::call(name, &args)
        })
    }

    /// A built-in macro, or (rarely) one Org cannot expand. Never one
    /// that reads the title it stands in (`title`, `keyword(TITLE)` in
    /// `#+TITLE`, `property(ITEM)` in a headline): Org expands those
    /// forever.
    pub fn builtin_macro(&mut self) -> String {
        let in_title = self
            .path
            .iter()
            .any(|p| matches!(*p, "doc-title" | "headline-title"));
        match self.rng.below(12) {
            4 | 7 | 8 if in_title => Gen::call("id", &[self.m("macro-arg")]),
            0 | 1 if !self.keywords.is_empty() => {
                let k = self.pick_string(|g| &g.keywords);
                Gen::call("keyword", &[k])
            }
            2 | 3 if !self.props.is_empty() => {
                let p = self.pick_string(|g| &g.props);
                Gen::call("property", &[p])
            }
            4 => Gen::call("title", &[]),
            5 => Gen::call("n", &[]),
            6 => Gen::call("n", &["c".into(), "5".into()]),
            7 => Gen::call("keyword", &["TITLE".into()]),
            8 => Gen::call("property", &["ITEM".into()]),
            9 if self.chance(20) => Gen::call("nosuch", &[]),
            10 if self.chance(20) => Gen::call("circ", &[]),
            _ => Gen::call("id", &[self.m("macro-arg")]),
        }
    }

    pub fn pick_string(&mut self, f: impl Fn(&Gen) -> &Vec<String>) -> String {
        let n = f(self).len();
        let i = self.rng.below(n);
        f(self)[i].clone()
    }

    /// One link: web, mail, file, other, KB (`id:`), or to a target,
    /// headline or custom id planted earlier.
    pub fn link(&mut self) -> String {
        self.within("link", |g| match g.rng.below(12) {
            0 => g.web_link(),
            1 => g.rel_link("https://e.com/p"),
            2 | 3 => g.kb_link(),
            4 => g.id_fragment_link(),
            5 => g.local_link(),
            6 => g.store_form_link(),
            7 => g.misc_link(),
            8 => g.nested_link(),
            9 => g.id_path_link(),
            _ => g.local_link(),
        })
    }

    fn description(&mut self) -> String {
        if self.chance(30) {
            String::new()
        } else {
            let d = self.within("link-desc", Gen::inline);
            format!("[d {d}]")
        }
    }

    fn web_link(&mut self) -> String {
        let m = self.m("web-path");
        let d = self.description();
        format!("[[https://e.com/{m}]{d}]")
    }

    fn weight(&mut self) -> String {
        if self.chance(20) {
            self.m("link-w")
        } else {
            self.pick(WEIGHTS).to_string()
        }
    }

    fn rel_link(&mut self, base: &str) -> String {
        let rel = if self.chance(70) {
            self.m("link-rel")
        } else {
            "parent".into()
        };
        let w = self.weight();
        let d = self.description();
        format!("[[{base}?rel={rel}&w={w}]{d}]")
    }

    fn node_ref(&mut self) -> String {
        format!("n{}", self.rng.below(4))
    }

    fn kb_link(&mut self) -> String {
        let node = self.node_ref();
        self.rel_link(&format!("id:{node}"))
    }

    fn id_fragment_link(&mut self) -> String {
        let node = self.node_ref();
        let f = self.mark("id-fragment", Allow::Href);
        let d = self.description();
        format!("[[id:{node}#{f}]{d}]")
    }

    fn id_path_link(&mut self) -> String {
        let m = self.m("id-path");
        let d = self.description();
        format!("[[id:{m}]{d}]")
    }

    fn store_form_link(&mut self) -> String {
        let node = self.node_ref();
        let beside = self.next;
        let w = self.m("text");
        let m = self.mark("id-store-desc", Allow::With(beside));
        format!("w {w} [[id:{node}|{m}]]")
    }

    /// A link to a target, headline or custom id planted earlier (else a
    /// web link).
    fn local_link(&mut self) -> String {
        let d = self.description();
        let pools = [
            (!self.targets.is_empty(), 0),
            (!self.headings.is_empty(), 1),
            (!self.custom_ids.is_empty(), 2),
        ];
        let live: Vec<usize> = pools.iter().filter(|p| p.0).map(|p| p.1).collect();
        if live.is_empty() {
            return self.web_link();
        }
        let path = match self.pick(&live) {
            0 => self.pick_string(|g| &g.targets),
            1 => format!("*{}", self.pick_string(|g| &g.headings)),
            _ => format!("#{}", self.pick_string(|g| &g.custom_ids)),
        };
        format!("[[{path}]{d}]")
    }

    fn misc_link(&mut self) -> String {
        match self.rng.below(5) {
            0 => format!("[[mailto:{}@e.com][m]]", self.m("mailto")),
            1 => format!(
                "[[file:/x/{}.org]{}]",
                self.m("file-path"),
                self.description()
            ),
            2 => format!("[[./{}.png]]", self.m("file-path")),
            3 => format!("<https://e.com/{}>", self.m("web-path")),
            _ => format!("[[foo:{}]{}]", self.m("other-path"), self.description()),
        }
    }

    fn nested_link(&mut self) -> String {
        let m = self.m("nested-link-path");
        let t = self.m("nested-link-text");
        format!("[[https://e.com][see [[file:/x/{m}][x]] {t}]]")
    }

    /// A dedicated `<<target>>` or a radio `<<<target>>>`.
    pub fn target(&mut self) -> String {
        if self.chance(25) {
            let m = self.m("radio-target");
            return format!("<<<r {m}>>>");
        }
        let m = self.m("target-name");
        self.targets.push(m.clone());
        format!("<<{m}>>")
    }

    /// A footnote reference: by label (defined later, in a hidden subtree,
    /// or never), inline anonymous, or inline named.
    pub fn footnote(&mut self) -> String {
        match self.rng.below(4) {
            0 => {
                let l = if self.chance(40) {
                    self.m("fn-label")
                } else {
                    format!("l{}", self.next)
                };
                self.fn_labels.push(l.clone());
                format!("[fn:{l}]")
            }
            1 => {
                let l = self.m("fn-label");
                let body = self.within("fn-named-inline", Gen::inline);
                format!("[fn:{l}: {body}]")
            }
            _ => {
                let body = self.within("fn-inline", Gen::inline);
                format!("[fn:: {body}]")
            }
        }
    }

    /// A citation with a style, a prefix, keys and a suffix.
    pub fn citation(&mut self) -> String {
        let style = self.pick(&["", "/t", "/n/b", "/na"]);
        let pre = if self.chance(50) {
            self.within("cite-prefix", Gen::inline) + " "
        } else {
            String::new()
        };
        let k1 = self.m("cite-key");
        let k2 = if self.chance(30) {
            format!(";@{}", self.m("cite-key"))
        } else {
            String::new()
        };
        let suf = if self.chance(50) {
            " ".to_string() + &self.within("cite-suffix", Gen::inline)
        } else {
            String::new()
        };
        format!("[cite{style}:{pre}@{k1}{k2}{suf}]")
    }

    /// A construct opened and closed across a macro boundary: at least one
    /// side is a macro.
    pub fn inline_pair(&mut self) -> String {
        let (label, om, o, cm, c) = self.pick(INLINE_PAIRS);
        let by_macro = self.rng.below(3);
        let open = if by_macro != 1 {
            Gen::call(om, &[])
        } else {
            o.to_string()
        };
        let close = if by_macro != 0 {
            Gen::call(cm, &[])
        } else {
            c.to_string()
        };
        self.within(label, |g| {
            let m = g.m("pair-content");
            format!("{open} {m}{close}")
        })
    }

    /// The `#+MACRO:` lines of the library and the failing macros.
    pub fn library() -> String {
        let mut s: String = LIBRARY
            .iter()
            .map(|(n, t)| format!("#+MACRO: {n} {t}\n"))
            .collect();
        s.push_str("#+MACRO: circ {{{circ2}}}\n#+MACRO: circ2 {{{circ}}}\n");
        s
    }
}
