//! Tallies and the printed report: per-category leaks, losses, fail-closed
//! documents, Emacs errors MAE renders, hangs, and when each known leak
//! shape (a)–(e) was first found.

use super::judge::{Leak, Verdict};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::time::Duration;

/// One judged document.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub doc: usize,
    pub verdict: Verdict,
    /// When the verdict was known, since the run started.
    pub at: Duration,
}

/// The confirmed leak shapes the oracle must rediscover:
/// (a) a macro pair across a comment block, (b) a macro in a headline
/// title reaching its anchor, (c) a target name, (d) a citation key,
/// (e) a `?rel=` value.
/// Inside a citation every leaf is shape (d)'s, and only its key counts.
pub fn shape(l: &Leak) -> Option<char> {
    let leaf = l.leaf;
    let in_id = l
        .channels
        .iter()
        .any(|(c, id_only)| *id_only || c.ends_with(".anchor"));
    if leaf.ends_with("cite-key") {
        Some('d')
    } else if l.category.starts_with("citation>") {
        None
    } else if leaf.ends_with("link-rel") {
        Some('e')
    } else if leaf.ends_with("target-name") || leaf == "c-target" {
        Some('c')
    } else if l.path.contains("region:comment") || leaf == "a-secret" {
        Some('a')
    } else if ((l.path.contains("headline-title") && l.path.contains("pair:"))
        || leaf == "b-secret")
        && in_id
    {
        Some('b')
    } else {
        None
    }
}

/// First discovery of a shape among the generated documents.
#[derive(Debug, Default, Clone)]
pub struct ShapeStats {
    pub first_index: Option<usize>,
    pub first_at: Option<Duration>,
    pub docs: usize,
    pub fixed: bool,
}

#[derive(Debug, Default)]
pub struct Tally {
    pub docs: usize,
    pub compared: usize,
    pub clean: usize,
    pub leak_docs: usize,
    pub fail_closed: usize,
    pub both_fail: usize,
    pub missing: usize,
    pub hangs: BTreeMap<&'static str, usize>,
    /// category → (documents, markers).
    pub leaks: BTreeMap<String, (usize, usize)>,
    pub lost: BTreeMap<&'static str, usize>,
    /// (error class, registered divergence) → documents.
    pub error_renders: BTreeMap<(String, Option<&'static str>), usize>,
    pub shapes: BTreeMap<char, ShapeStats>,
}

impl Tally {
    /// `fixed` documents come first in `outcomes`' indexing.
    pub fn of(outcomes: &[Outcome], fixed: usize) -> Tally {
        let mut t = Tally::default();
        for o in outcomes {
            t.docs += 1;
            t.add(o, fixed);
        }
        t
    }

    fn add(&mut self, o: &Outcome, fixed: usize) {
        match &o.verdict {
            Verdict::Compared { leaks, lost } => {
                self.compared += 1;
                self.add_leaks(leaks, o, fixed);
                for (_, leaf) in lost {
                    *self.lost.entry(leaf).or_default() += 1;
                }
            }
            Verdict::FailClosed => self.fail_closed += 1,
            Verdict::BothFail => self.both_fail += 1,
            Verdict::EmacsErrorMaeRenders { class, registered } => {
                *self
                    .error_renders
                    .entry((class.clone(), *registered))
                    .or_default() += 1;
            }
            Verdict::EmacsHang(why) => *self.hangs.entry(why).or_default() += 1,
            Verdict::Missing => self.missing += 1,
        }
    }

    fn add_leaks(&mut self, leaks: &[Leak], o: &Outcome, fixed: usize) {
        if leaks.is_empty() {
            self.clean += 1;
            return;
        }
        self.leak_docs += 1;
        let mut seen = std::collections::BTreeSet::new();
        for l in leaks {
            let e = self.leaks.entry(l.category.clone()).or_default();
            e.1 += 1;
            if seen.insert(&l.category) {
                e.0 += 1;
            }
        }
        let shapes: std::collections::BTreeSet<char> = leaks.iter().filter_map(shape).collect();
        for s in shapes {
            let st = self.shapes.entry(s).or_default();
            if o.doc < fixed {
                st.fixed = true;
                continue;
            }
            st.docs += 1;
            let i = o.doc - fixed;
            st.first_index = Some(st.first_index.map_or(i, |f| f.min(i)));
            st.first_at = Some(st.first_at.map_or(o.at, |f| f.min(o.at)));
        }
    }

    /// Leaks plus Emacs errors MAE renders outside the register.
    pub fn failures(&self) -> usize {
        let unregistered: usize = self
            .error_renders
            .iter()
            .filter(|((_, r), _)| r.is_none())
            .map(|(_, n)| n)
            .sum();
        self.leak_docs + unregistered + self.missing
    }

    pub fn render(&self, generated: usize, elapsed: Duration) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "== leak oracle: {} documents ({generated} generated) in {elapsed:.1?}",
            self.docs
        );
        let _ = writeln!(
            s,
            "compared {} | clean {} | LEAKING {} | fail-closed {} | both fail {} | missing {} | hangs {:?}",
            self.compared, self.clean, self.leak_docs, self.fail_closed, self.both_fail, self.missing, self.hangs
        );
        let _ = writeln!(s, "-- LEAKS by category (documents, markers):");
        for (leaf, (d, m)) in &self.leaks {
            let _ = writeln!(s, "   {leaf:<40} {d:>6} {m:>6}");
        }
        let _ = writeln!(s, "-- Emacs errors, MAE renders (FAIL unless registered):");
        for ((class, reg), n) in &self.error_renders {
            let tag = reg.map_or("FAIL".to_string(), |r| format!("registered:{r}"));
            let _ = writeln!(s, "   {n:>6} [{tag}] {class}");
        }
        let _ = writeln!(s, "-- lost (MAE hides, Emacs shows; allowed) by leaf:");
        for (leaf, n) in &self.lost {
            let _ = writeln!(s, "   {leaf:<28} {n:>6}");
        }
        self.render_shapes(&mut s);
        s
    }

    fn render_shapes(&self, s: &mut String) {
        let _ = writeln!(s, "-- known shapes (a)-(e):");
        for c in ['a', 'b', 'c', 'd', 'e'] {
            let st = self.shapes.get(&c).cloned().unwrap_or_default();
            let first = st.first_index.map_or("not found".into(), |i| {
                format!("first at generated doc #{i}")
            });
            let at = st
                .first_at
                .map_or(String::new(), |d| format!(" after {d:.1?}"));
            let _ = writeln!(
                s,
                "   ({c}) unaided: {first}{at}, in {} generated docs; fixed case found: {}",
                st.docs, st.fixed
            );
        }
    }
}

/// `text` around marker `m` (for a repro).
pub fn around(text: &str, m: u32) -> String {
    let needle = super::view::marker(m);
    let lower = text.to_ascii_lowercase();
    let Some(i) = lower.find(&needle) else {
        return String::new();
    };
    let lo = text.floor_char_boundary(i.saturating_sub(100));
    let hi = text.ceil_char_boundary((i + needle.len() + 100).min(text.len()));
    text[lo..hi].to_string()
}
