//! The generator's elements: blocks, results, drawers, tables, lists,
//! keywords, regions opened and closed by macros, and line-start macro
//! expansions that make structure.

use super::gen::{Gen, REGION_PAIRS};

impl Gen {
    /// One element (lines, each ending in a newline).
    pub fn block(&mut self) -> String {
        if self.deep() {
            return format!("P {}\n", self.inline());
        }
        match self.rng.below(22) {
            0..=3 => self.paragraph(),
            4 => self.comment_line(),
            5 => self.comment_block(),
            6 | 7 => self.src_block(),
            8 => self.call_line(),
            9 => self.named_results(),
            10 => self.drawer(),
            11 => self.table(),
            12 => self.list(),
            13 => self.greater_block(),
            14 => self.export_block(),
            15 => self.keyword_line(),
            16 | 17 => self.region_pair(),
            18 => self.line_macro(),
            19 => self.fixed_width(),
            20 => self.item_node(),
            _ => self.footnote_definition(),
        }
    }

    /// One to three elements, separated by blank lines.
    pub fn blocks(&mut self) -> String {
        let n = 1 + self.rng.below(3);
        let parts: Vec<String> = (0..n).map(|_| self.block()).collect();
        parts.join("\n")
    }

    pub fn paragraph(&mut self) -> String {
        let a = self.phrases();
        if self.chance(30) {
            let b = self.phrases();
            format!("P {a}\nP {b}\n")
        } else {
            format!("P {a}\n")
        }
    }

    fn comment_line(&mut self) -> String {
        self.within("comment-line", |g| format!("# {}\n", g.phrases()))
    }

    fn comment_block(&mut self) -> String {
        self.within("comment-block", |g| {
            format!("#+begin_comment\nP {}\n#+end_comment\n", g.phrases())
        })
    }

    fn block_exports(&mut self) -> (&'static str, String) {
        let e = self.pick(super::gen::EXPORTS);
        let h = if e.is_empty() {
            String::new()
        } else {
            format!(" :exports {e}")
        };
        (if e.is_empty() { "inherited" } else { e }, h)
    }

    fn src_block(&mut self) -> String {
        let (e, hdr) = self.block_exports();
        let lang = self.pick(&["sh", "elisp", "python"]);
        let named = self.chance(30);
        let name = format!("b{}", self.next);
        self.within(e, |g| {
            let body = g.m("src-body");
            let mut s = String::new();
            if named {
                s.push_str(&format!("#+NAME: {name}\n"));
            }
            s.push_str(&format!(
                "#+begin_src {lang}{hdr}\necho {body}\n#+end_src\n"
            ));
            if g.chance(60) {
                let r = g.m("src-results");
                let label = if named && g.chance(50) {
                    format!(" {name}")
                } else {
                    String::new()
                };
                s.push_str(&format!("\n#+RESULTS:{label}\n: {r}\n"));
            }
            s
        })
    }

    fn call_line(&mut self) -> String {
        if self.chance(5) {
            return "#+CALL: nosuch()\n".into();
        }
        let (e, hdr) = self.block_exports();
        self.within(e, |g| {
            let arg = g.m("call-arg");
            let mut s = format!("#+CALL: f(x=\"{arg}\"){hdr}\n");
            if g.chance(60) {
                let r = g.m("call-results");
                s.push_str(&format!("\n#+RESULTS:\n: {r}\n"));
            }
            s
        })
    }

    fn named_results(&mut self) -> String {
        let name = if self.chance(50) { "f" } else { "nobody" };
        let r = self.m("named-results");
        format!("#+RESULTS: {name}\n: {r}\n")
    }

    fn drawer(&mut self) -> String {
        let name = self.pick(&["NOTE", "LOGBOOK", "STUFF"]);
        self.within("drawer", |g| {
            format!(":{name}:\nP {}\n:END:\n", g.phrases())
        })
    }

    fn table(&mut self) -> String {
        let mut s = String::new();
        if self.chance(40) {
            let c = self.within("caption", Gen::inline);
            s.push_str(&format!("#+CAPTION: {c}\n"));
        }
        self.within("table", |g| {
            let (a, b, c) = (g.inline(), g.inline(), g.inline());
            s.push_str(&format!("| {a} | {b} |\n|---+---|\n| {c} | x |\n"));
        });
        s
    }

    fn list(&mut self) -> String {
        self.within("list", |g| {
            let a = g.phrases();
            let tag = g.m("list-tag");
            let b = g.inline();
            let c = g.inline();
            format!("- {a}\n- t {tag} :: {b}\n  - {c}\n")
        })
    }

    fn greater_block(&mut self) -> String {
        let (label, kind) = self.pick(&[
            ("quote", "quote"),
            ("verse", "verse"),
            ("center", "center"),
            ("example", "example"),
        ]);
        self.within(label, |g| {
            let body = if kind == "example" {
                g.m("example")
            } else {
                g.phrases()
            };
            format!("#+begin_{kind}\nP {body}\n#+end_{kind}\n")
        })
    }

    fn export_block(&mut self) -> String {
        let m = self.m("export");
        match self.rng.below(3) {
            0 => format!("#+begin_export html\n{m}\n#+end_export\n"),
            1 => format!("#+HTML: {m}\n"),
            _ => format!("#+LATEX: {m}\n"),
        }
    }

    fn keyword_line(&mut self) -> String {
        let k = format!("K{}", self.next);
        self.keywords.push(k.clone());
        self.within("keyword-value", |g| format!("#+{k}: {}\n", g.phrases()))
    }

    fn fixed_width(&mut self) -> String {
        format!(": {}\n", self.m("fixed-width"))
    }

    /// A block opened and closed by literal lines or macros, with content,
    /// closers and openers interleaved in a random order.
    pub fn region_pair(&mut self) -> String {
        let (label, om, o, cm, c) = self.pick(REGION_PAIRS);
        let open = if self.chance(70) {
            Gen::call(om, &[])
        } else {
            o.to_string()
        };
        self.within(label, |g| {
            let mut s = format!("{open}\n");
            let mut content = false;
            for _ in 0..2 + g.rng.below(3) {
                let line = match g.rng.below(4) {
                    0 => Gen::call(cm, &[]),
                    1 => c.to_string(),
                    2 if g.chance(30) => Gen::call(om, &[]),
                    _ => {
                        content = true;
                        format!("P {}", g.m("region-content"))
                    }
                };
                s.push_str(&line);
                s.push('\n');
            }
            if !content {
                s.push_str(&format!("P {}\n", g.m("region-content")));
            }
            if g.chance(60) {
                s.push_str(c);
                s.push('\n');
            }
            s
        })
    }

    /// A macro at the start of a line whose expansion makes structure.
    fn line_macro(&mut self) -> String {
        self.within("line-macro", |g| match g.rng.below(5) {
            0 => format!("{}\n", Gen::call("hash", &[g.m("hash-arg")])),
            1 => format!("{} {}\n", Gen::call("id", &["#".into()]), g.m("after-hash")),
            2 => {
                let open = Gen::call("id", &["#+begin_comment".into()]);
                format!("{open}\nP {}\n#+end_comment\n", g.m("region-content"))
            }
            3 => format!("{}\n", Gen::call("callm", &[g.m("call-arg")])),
            _ => {
                let open = Gen::call("id", &["#+begin_src sh :exports none".into()]);
                format!("{open}\necho {}\n#+end_src\n", g.m("src-body"))
            }
        })
    }

    /// A list item that is a KB node (a drawer with an `:ID:`).
    fn item_node(&mut self) -> String {
        let anchor_n = self.next;
        let anchor = self.m("item-title");
        let id = format!("i{}", self.next);
        let value = self.mark("item-property", super::gen::Allow::Meta(Some(anchor_n)));
        let body = self.within("item-body", Gen::inline);
        format!(
            "- {anchor} item\n  :PROPERTIES:\n  :ID: {id}\n  :IP: v {value}\n  :END:\n  {body}\n"
        )
    }

    /// The definition of a label referenced earlier (or a fresh one).
    fn footnote_definition(&mut self) -> String {
        let label = if self.fn_labels.is_empty() {
            format!("l{}", self.next)
        } else {
            self.fn_labels.remove(0)
        };
        self.within("fn-definition", |g| {
            format!("[fn:{label}] {}\n", g.phrases())
        })
    }
}
