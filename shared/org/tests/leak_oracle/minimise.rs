//! Delta debugging (Zeller's ddmin) of a failing document: over lines,
//! then over whitespace-separated tokens. Every round's candidates are
//! exported by ONE Emacs process, and each is judged as the oracle judges
//! it; a candidate holds if it still fails the same way. A budget of Emacs
//! runs bounds the time a minimisation can take (a candidate that makes
//! Emacs loop costs a whole timeout).

use super::channels::mae_view;
use super::emacs::Emacs;
use super::gen::Sites;
use super::judge::{judge, Verdict};

/// What the minimised document must keep doing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// Leak a marker of this category (`judge::category`).
    Leak(String),
    /// Fail in Emacs with this error class while MAE renders.
    ErrorRenders(String),
}

/// Whether `verdict` still shows `target`.
pub fn holds(target: &Target, verdict: &Verdict) -> bool {
    match (target, verdict) {
        (Target::Leak(cat), Verdict::Compared { leaks, .. }) => {
            leaks.iter().any(|l| &l.category == cat)
        }
        (Target::ErrorRenders(c), Verdict::EmacsErrorMaeRenders { class, registered }) => {
            registered.is_none() && class == c
        }
        _ => false,
    }
}

/// Judges each candidate text with one Emacs run.
fn test_all(texts: &[String], sites: &Sites, target: &Target, emacs: &Emacs) -> Vec<bool> {
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let outs = emacs.export(&refs);
    texts
        .iter()
        .zip(outs.iter())
        .map(|(t, e)| holds(target, &judge(e, &mae_view(t), sites, t)))
        .collect()
}

/// The minimal document found within `budget` Emacs runs, and the runs.
pub fn minimise(
    text: &str,
    sites: &Sites,
    target: &Target,
    emacs: &Emacs,
    budget: usize,
) -> (String, usize) {
    let mut runs = 0;
    let mut test = |c: &[String]| {
        if runs >= budget {
            return vec![false; c.len()];
        }
        runs += 1;
        test_all(c, sites, target, emacs)
    };
    let lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let lines = ddmin(lines, &mut test);
    let toks: Vec<String> = lines.iter().flat_map(|l| tokens(l)).collect();
    let text = ddmin(toks, &mut test).concat();
    (text, runs)
}

/// A line's maximal non-blank runs, each with the blanks after it.
fn tokens(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut prev_blank = true;
    for c in line.chars() {
        let blank = c.is_whitespace();
        if !blank && prev_blank || out.is_empty() {
            out.push(String::new());
        }
        if let Some(t) = out.last_mut() {
            t.push(c);
        }
        prev_blank = blank;
    }
    out
}

/// ddmin over `units`, `test` judging candidate texts in one batch.
fn ddmin(mut units: Vec<String>, test: &mut impl FnMut(&[String]) -> Vec<bool>) -> Vec<String> {
    let mut n = 2;
    while units.len() >= 2 {
        let size = units.len().div_ceil(n).max(1);
        let parts: Vec<Vec<String>> = units.chunks(size).map(<[String]>::to_vec).collect();
        let complements: Vec<Vec<String>> =
            (0..parts.len()).map(|i| complement(&parts, i)).collect();
        let mut cands: Vec<String> = parts.iter().map(|p| p.concat()).collect();
        cands.extend(complements.iter().map(|c| c.concat()));
        let ok = test(&cands);
        if let Some(i) = (0..parts.len()).find(|&i| ok[i]) {
            units = parts[i].clone();
            n = 2;
        } else if let Some(i) = (0..parts.len()).find(|&i| ok[parts.len() + i]) {
            units = complements[i].clone();
            n = (n - 1).max(2);
        } else if n >= units.len() {
            break;
        } else {
            n = (n * 2).min(units.len());
        }
    }
    units
}

fn complement(parts: &[Vec<String>], skip: usize) -> Vec<String> {
    parts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != skip)
        .flat_map(|(_, p)| p.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddmin_finds_the_two_units_that_matter() {
        let units: Vec<String> = "a b c d e f g h".split(' ').map(str::to_string).collect();
        let mut t = |c: &[String]| -> Vec<bool> {
            c.iter()
                .map(|s| s.contains('c') && s.contains('g'))
                .collect()
        };
        assert_eq!(ddmin(units, &mut t).concat(), "cg");
    }

    #[test]
    fn tokens_rejoin_to_the_line() {
        assert_eq!(tokens("  a bc\n").concat(), "  a bc\n");
    }
}
