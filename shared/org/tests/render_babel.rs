//! Babel's `:exports`, against what Emacs's own export does (ADR-112 D3,
//! O5: privacy-relevant regions first).
//!
//! Every case below was exported by `emacs -Q --batch` (Emacs 30.2, Org
//! 9.7.11) with `org-export-use-babel` on, evaluation made impossible
//! (`:eval never-export` in the default header arguments and
//! `org-babel-execute-src-block` overridden to signal an error), and
//! `org-babel-get-src-block-info` printing each block's resolved
//! `:exports`. `hidden` lists every word Emacs's HTML left out, `shown`
//! every word it kept. Cases where MAE hides MORE than Org, deliberately,
//! are marked and listed in `conformance/render-divergences.toml`.

use mae_org::{parse, render_html, semantics, text, ParseOptions, RenderOptions, Resolver};

struct Case {
    name: &'static str,
    doc: &'static str,
    hidden: &'static [&'static str],
    shown: &'static [&'static str],
}

const CASES: &[Case] = &[
    Case {
        name: "a #+PROPERTY header-args hides code and results",
        doc: "#+PROPERTY: header-args :exports none\n#+begin_src sh\nSECRET01\n#+end_src\n\n#+RESULTS:\n: RES01\n\nafter\n",
        hidden: &["SECRET01", "RES01"],
        shown: &["after"],
    },
    Case {
        name: "#+HEADER",
        doc: "#+HEADER: :exports none\n#+begin_src sh\nSECRET02\n#+end_src\n\nafter\n",
        hidden: &["SECRET02"],
        shown: &["after"],
    },
    Case {
        name: "a subtree's header-args is inherited",
        doc: "* H\n:PROPERTIES:\n:header-args: :exports none\n:END:\n** Child\n#+begin_src sh\nSECRET03\n#+end_src\n",
        hidden: &["SECRET03"],
        shown: &["Child"],
    },
    Case {
        name: "header-args:LANG applies to its language only",
        doc: "#+PROPERTY: header-args:sh :exports none\n#+begin_src sh\nSECRET04\n#+end_src\n\n#+begin_src python\nSHOWN04\n#+end_src\n",
        hidden: &["SECRET04"],
        shown: &["SHOWN04"],
    },
    Case {
        name: "the block's own arguments override the property",
        doc: "#+PROPERTY: header-args :exports none\n#+begin_src sh :exports code\nSHOWN05\n#+end_src\n",
        hidden: &[],
        shown: &["SHOWN05"],
    },
    Case {
        name: "#+HEADER overrides the block line",
        doc: "#+HEADER: :exports code\n#+begin_src sh :exports none\nWHICH06\n#+end_src\n",
        hidden: &[],
        shown: &["WHICH06"],
    },
    Case {
        name: "a plain header-args ends inheritance; header-args+ appends",
        doc: "* H\n:PROPERTIES:\n:header-args: :exports none\n:END:\n** Child stops\n:PROPERTIES:\n:header-args: :results silent\n:END:\n#+begin_src sh\nSHOWN07\n#+end_src\n** Child appends\n:PROPERTIES:\n:header-args+: :results silent\n:END:\n#+begin_src sh\nSECRET07\n#+end_src\n",
        hidden: &["SECRET07"],
        shown: &["SHOWN07"],
    },
    Case {
        name: "#+PROPERTY: header-args+ appends",
        doc: "#+PROPERTY: header-args :exports none\n#+PROPERTY: header-args+ :results silent\n#+begin_src sh\nSECRET08\n#+end_src\n",
        hidden: &["SECRET08"],
        shown: &[],
    },
    Case {
        name: "a second plain #+PROPERTY replaces the first",
        doc: "#+PROPERTY: header-args :exports none\n#+PROPERTY: header-args :results silent\n#+begin_src sh\nWHICH08B\n#+end_src\n",
        hidden: &[],
        shown: &["WHICH08B"],
    },
    Case {
        name: "existing results: none, code, results, both",
        doc: "#+begin_src sh :exports none\nSECRET09\n#+end_src\n\n#+RESULTS:\n: RES09none\n\n#+begin_src sh :exports code\nSHOWN09\n#+end_src\n\n#+RESULTS:\n: RES09code\n\n#+begin_src sh :exports results\nSECRET09r\n#+end_src\n\n#+RESULTS:\n: SHOWNRES09\n\n#+begin_src sh :exports both\nSHOWN09b\n#+end_src\n\n#+RESULTS:\n: SHOWNRES09b\n",
        hidden: &["SECRET09", "RES09none", "RES09code", "SECRET09r"],
        shown: &["SHOWN09", "SHOWNRES09", "SHOWN09b", "SHOWNRES09b"],
    },
    Case {
        name: "named results anywhere; adjacent anonymous results",
        doc: "#+NAME: blk\n#+begin_src sh :exports code\nSHOWN10\n#+end_src\n\npara between\n\n#+RESULTS: blk\n: RES10named\n\n#+begin_src sh :exports none\nSECRET10\n#+end_src\n#+RESULTS:\n: RES10adjacent\n",
        hidden: &["RES10named", "SECRET10", "RES10adjacent"],
        shown: &["SHOWN10", "para between"],
    },
    Case {
        name: "a removed block takes its caption, targets and footnotes",
        doc: "#+CAPTION: CAPSECRET <<tgt11>> note[fn:1]\n#+NAME: n11\n#+begin_src sh :exports none\nSECRET11\n#+end_src\n\n#+CAPTION: CAPR11 <<tgt11b>> [fn:2]\n#+begin_src sh :exports results\nSECRET11r\n#+end_src\n\n#+RESULTS:\n: SHOWNRES11\n\n#+CAPTION: CAPCODE11\n#+begin_src sh :exports code\nSHOWN11\n#+end_src\n\n[[tgt11][link to tgt]]\n\n[fn:1] FNSECRET11\n[fn:2] FNSECRET11b\n",
        hidden: &["CAPSECRET", "tgt11", "SECRET11", "CAPR11", "SECRET11r", "FNSECRET11", "org-footnote"],
        shown: &["SHOWNRES11", "CAPCODE11", "SHOWN11", "link to tgt"],
    },
    Case {
        name: "inline src blocks export results by default",
        doc: "default src_sh{echo CODE12a} x {{{results(=RES12a=)}}}\n\ncode src_sh[:exports code]{echo CODE12b} {{{results(=RES12b=)}}}\n\nnone src_sh[:exports none]{echo CODE12c} {{{results(=RES12c=)}}}\n\nboth src_sh[:exports both]{echo CODE12d} {{{results(=RES12d=)}}}\n",
        hidden: &["CODE12a", "CODE12c", "RES12b", "RES12c"],
        shown: &["RES12a", "CODE12b", "CODE12d", "RES12d"],
    },
    Case {
        name: "quoted and bracketed text does not split",
        doc: "#+begin_src sh :var x=\":exports none\"\nQUOTED13\n#+end_src\n\n#+begin_src sh :exports \"none\"\nQSTR13\n#+end_src\n",
        hidden: &["QSTR13"],
        shown: &["QUOTED13"],
    },
    Case {
        name: "the file's property drawer",
        doc: ":PROPERTIES:\n:header-args: :exports none\n:END:\n#+begin_src sh\nSECRET15\n#+end_src\n* H\n#+begin_src sh\nSECRET15b\n#+end_src\n",
        hidden: &["SECRET15", "SECRET15b"],
        shown: &[],
    },
    Case {
        name: "switches before the arguments",
        doc: "#+begin_src sh -n -r :exports none\nSECRET16\n#+end_src\n\n#+begin_src sh -l \"(ref:%s)\" :exports none\nSECRET16b\n#+end_src\n",
        hidden: &["SECRET16", "SECRET16b"],
        shown: &[],
    },
    Case {
        name: "#+HEADER lines apply in order",
        doc: "#+HEADER: :exports none\n#+HEADER: :exports code\n#+begin_src sh\nWHICH17\n#+end_src\n\n#+HEADERS: :exports code\n#+HEADERS: :exports none\n#+begin_src sh\nWHICH17b\n#+end_src\n",
        hidden: &["WHICH17b"],
        shown: &["WHICH17"],
    },
    // Deliberate divergences: Emacs SHOWS these (it warns "Unknown value of
    // src block parameter :exports" and exports the block as written).
    Case {
        name: "DIVERGENCE: values are matched case-insensitively, unknown words ignored",
        doc: "#+begin_src sh :exports NONE\nCASE13\n#+end_src\n\n#+begin_src sh :exports none foo\nUNK13\n#+end_src\n",
        hidden: &["CASE13", "UNK13"],
        shown: &[],
    },
    Case {
        name: "DIVERGENCE: a value Org would evaluate hides code and results",
        doc: "#+begin_src sh :exports (if t \"code\" \"none\")\nLISP\n#+end_src\n\n#+RESULTS:\n: LISPRES\n",
        hidden: &["LISP", "LISPRES"],
        shown: &[],
    },
];

fn outputs(doc: &str) -> Vec<(&'static str, String)> {
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    let html = render_html(&d, &sem, &RenderOptions::default(), &Resolver::default());
    vec![("html", html), ("text", text(&d, &sem))]
}

#[test]
fn exports_decide_what_is_rendered_as_emacs_decides() {
    let mut failures = Vec::new();
    for c in CASES {
        for (surface, out) in outputs(c.doc) {
            for h in c.hidden {
                if out.contains(h) {
                    failures.push(format!("{}: {h} shown in {surface}:\n{out}", c.name));
                }
            }
            for s in c.shown {
                if surface == "html" && !out.contains(s) {
                    failures.push(format!("{}: {s} missing from {surface}:\n{out}", c.name));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn nothing_is_evaluated_and_shown_code_is_verbatim() {
    let doc = "#+begin_src sh :exports both\necho $(id)\n#+end_src\n";
    let d = parse(doc, &ParseOptions::default());
    let html = render_html(
        &d,
        &semantics(&d),
        &RenderOptions::default(),
        &Resolver::default(),
    );
    assert!(html.contains("echo $(id)"), "{html}");
    assert!(!html.contains("uid="), "{html}");
}
