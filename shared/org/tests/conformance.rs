//! The Emacs differential gate (ADR-112 D5): mae-org's elements must equal
//! `org-element`'s — kind, begin, end, contents begin/end and parent — for
//! every file in
//! `conformance/corpus.txt`, except differences listed in
//! `conformance/divergences.toml`. An unlisted difference fails; so does a
//! listed one that no longer occurs (a stale entry hides nothing, so it must
//! go). The goldens are checked in; `conformance/regen.sh` rebuilds them.

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use support::*;

fn conformance_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance")
}

/// Repo-relative paths listed in corpus.txt.
fn corpus_list() -> Vec<String> {
    read(&conformance_dir().join("corpus.txt"))
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn golden_for(rel: &str) -> String {
    read(
        &conformance_dir()
            .join("goldens")
            .join(format!("{rel}.jsonl")),
    )
}

#[derive(Debug, Clone)]
struct Divergence {
    kind: String,
    file: String,
    begin: Option<usize>,
    reason: String,
    test: String,
}

impl Divergence {
    fn covers(&self, file: &str, el: &El) -> bool {
        let file_ok = self.file == file
            || (self.file.ends_with('*') && file.starts_with(self.file.trim_end_matches('*')));
        file_ok
            && (self.kind == "*" || self.kind == el.kind)
            && self.begin.is_none_or(|b| b == el.begin)
    }
}

/// Reads the register: `[[divergence]]` tables of `key = "string"` or
/// `key = integer` lines. Deliberately a tiny subset of TOML, so the test
/// needs no TOML dependency; anything else in the file is an error.
fn load_register() -> Vec<Divergence> {
    let text = read(&conformance_dir().join("divergences.toml"));
    let mut out: Vec<BTreeMap<String, String>> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[divergence]]" {
            out.push(BTreeMap::new());
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("divergences.toml:{}: not `key = value`", n + 1));
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(v);
        out.last_mut()
            .unwrap_or_else(|| panic!("divergences.toml:{}: key outside [[divergence]]", n + 1))
            .insert(k.trim().to_string(), v.to_string());
    }
    out.into_iter()
        .map(|m| {
            let get = |k: &str| {
                m.get(k)
                    .cloned()
                    .unwrap_or_else(|| panic!("divergence entry without `{k}`: {m:?}"))
            };
            Divergence {
                kind: get("kind"),
                file: get("file"),
                begin: m
                    .get("begin")
                    .map(|b| b.parse().expect("begin is an integer")),
                reason: get("reason"),
                test: get("test"),
            }
        })
        .collect()
}

#[test]
fn goldens_agree_with_emacs_except_registered_divergences() {
    let register = load_register();
    let mut used = vec![false; register.len()];
    let mut unlisted = Vec::new();
    let mut per_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for rel in corpus_list() {
        let text = read(&repo_root().join(&rel));
        let golden = golden_for(&rel);
        check_golden_header(&golden, &text).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let oracle: Vec<El> = golden_elements(&golden)
            .into_iter()
            .map(|(e, _)| e)
            .collect();
        let mine = mae_elements(&parse_default(&text));
        let (only_oracle, only_mae) = diff(&oracle, &mine);
        for e in &oracle {
            let entry = per_kind.entry(e.kind.clone()).or_default();
            entry.1 += 1;
            entry.0 += usize::from(!only_oracle.contains(e));
        }
        for (side, e) in only_oracle
            .iter()
            .map(|e| ("oracle", e))
            .chain(only_mae.iter().map(|e| ("mae", e)))
        {
            match register.iter().position(|d| d.covers(&rel, e)) {
                Some(i) => used[i] = true,
                None => unlisted.push(format!("{rel}: {side} only: {e:?}")),
            }
        }
    }
    eprintln!("agreement per org-element kind (oracle elements matched exactly):");
    for (k, (m, t)) in &per_kind {
        eprintln!("  {k:22} {m:5}/{t:5}");
    }
    let stale: Vec<_> = register
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|(d, _)| format!("{d:?}"))
        .collect();
    assert!(
        unlisted.is_empty(),
        "differences not in divergences.toml:\n{}",
        unlisted.join("\n")
    );
    assert!(
        stale.is_empty(),
        "divergences.toml entries that no longer occur:\n{}",
        stale.join("\n")
    );
}

#[test]
fn every_listed_file_has_a_golden_and_every_golden_is_listed() {
    let listed: std::collections::BTreeSet<String> = corpus_list().into_iter().collect();
    let goldens_dir = conformance_dir().join("goldens");
    let mut on_disk = std::collections::BTreeSet::new();
    let mut stack = vec![goldens_dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("goldens dir").flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(rel) = p
                .strip_prefix(&goldens_dir)
                .ok()
                .and_then(|r| r.to_str())
                .and_then(|r| r.strip_suffix(".jsonl"))
            {
                on_disk.insert(rel.replace('\\', "/"));
            }
        }
    }
    assert_eq!(
        listed, on_disk,
        "corpus.txt and goldens/ disagree: run conformance/regen.sh"
    );
    // Every synthetic fixture is under the gate.
    for f in fixtures() {
        assert!(
            listed.contains(&rel(&f)),
            "fixture {} is not in corpus.txt",
            rel(&f)
        );
    }
}

/// Emacs decodes a CRLF file with DOS line endings and strips a BOM, so the
/// LF golden — shifted — is the oracle for every file's CRLF and BOM variants.
/// No divergence may be registered against these.
#[test]
fn crlf_and_bom_variants_agree_with_the_lf_goldens() {
    let register = load_register();
    let mut checked = 0;
    for rel in corpus_list() {
        let text = read(&repo_root().join(&rel));
        if text.contains('\r') || text.starts_with('\u{feff}') {
            continue; // stored with CR or a BOM: its golden already covers it
        }
        // A file with a registered divergence is checked against mae-org's
        // own LF parse: the variant must not ADD a difference.
        let diverges = register.iter().any(|d| {
            d.file == rel
                || (d.file.ends_with('*') && rel.starts_with(d.file.trim_end_matches('*')))
        });
        let base: Vec<El> = if diverges {
            mae_elements(&parse_default(&text))
        } else {
            golden_elements(&golden_for(&rel))
                .into_iter()
                .map(|(e, _)| e)
                .collect()
        };
        let want: Vec<El> = base.iter().map(|e| lf_to_crlf(&text, e)).collect();
        let (a, b) = diff(&want, &mae_elements(&parse_default(&to_crlf(&text))));
        assert!(
            a.is_empty() && b.is_empty(),
            "{rel} as CRLF:\n oracle only {a:?}\n mae only {b:?}"
        );
        let want: Vec<El> = base.iter().map(|e| shift_el(e, |o| o + 3)).collect();
        let (a, b) = diff(
            &want,
            &mae_elements(&parse_default(&format!("\u{feff}{text}"))),
        );
        assert!(
            a.is_empty() && b.is_empty(),
            "{rel} with BOM:\n oracle only {a:?}\n mae only {b:?}"
        );
        checked += 1;
    }
    assert!(checked > 80, "only {checked} files checked");
}

#[test]
fn every_divergence_names_a_reason_and_an_existing_pinning_test() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = String::new();
    for dir in ["tests", "src"] {
        let mut stack = vec![crate_dir.join(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("source dir").flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    sources.push_str(&read(&p));
                }
            }
        }
    }
    let register = load_register();
    assert!(!register.is_empty());
    for d in register {
        assert!(
            d.reason.len() > 20,
            "divergence without a real reason: {d:?}"
        );
        assert!(
            sources.contains(&format!("fn {}(", d.test)),
            "pinning test `{}` does not exist: {d:?}",
            d.test
        );
    }
}

/// The typed views must report what Emacs reports, for every headline,
/// keyword, property and drawer in the goldens: a selective oracle for the
/// views, not just the spans.
#[test]
fn typed_views_agree_with_the_oracles_properties() {
    let mut compared = BTreeMap::<&str, usize>::new();
    for rel in corpus_list() {
        let text = read(&repo_root().join(&rel));
        if text.contains('\r') || text.starts_with('\u{feff}') {
            continue;
        }
        let doc = parse_default(&text);
        for (el, v) in golden_elements(&golden_for(&rel)) {
            let node = doc.syntax().descendants().find(|n| {
                let r = n.text_range();
                usize::from(r.start()) == el.begin
                    && usize::from(r.end()) == el.end
                    && KIND_MAP
                        .iter()
                        .any(|(k, s)| *k == n.kind() && *s == el.kind)
            });
            let Some(node) = node else { continue }; // span differences are the other test's job
            if check_view(&el.kind, node, &v, &rel) {
                *compared
                    .entry(match el.kind.as_str() {
                        "headline" => "headline",
                        "keyword" => "keyword",
                        "node-property" => "node-property",
                        _ => "drawer/block",
                    })
                    .or_default() += 1;
            }
        }
    }
    eprintln!("view fields compared: {compared:?}");
    // Floors just under what the goldens hold (458/391/100/52 at
    // 2026-10-31), so a matcher that silently stops finding nodes fails.
    assert!(compared.get("headline").copied().unwrap_or(0) >= 450);
    assert!(compared.get("keyword").copied().unwrap_or(0) >= 380);
    assert!(compared.get("node-property").copied().unwrap_or(0) >= 95);
    assert!(compared.get("drawer/block").copied().unwrap_or(0) >= 45);
}

fn opt_str(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

fn check_view(kind: &str, node: mae_org::SyntaxNode, v: &serde_json::Value, rel: &str) -> bool {
    use mae_org::*;
    match kind {
        "headline" => {
            let h = Headline::cast(node).expect("headline view");
            let tags: Vec<String> = v["tags"]
                .as_array()
                .map(|a| a.iter().filter_map(opt_str).collect())
                .unwrap_or_default();
            assert_eq!(
                h.level() as u64,
                v["level"].as_u64().unwrap_or(0),
                "{rel} level"
            );
            assert_eq!(
                h.title(),
                v["raw_title"].as_str().unwrap_or(""),
                "{rel} title"
            );
            assert_eq!(h.todo(), opt_str(&v["todo"]), "{rel} todo");
            assert_eq!(
                h.priority().map(String::from),
                opt_str(&v["priority"]),
                "{rel} priority"
            );
            assert_eq!(h.tags(), tags, "{rel} tags");
            assert_eq!(h.is_commented(), v["commentedp"] == true, "{rel} commented");
        }
        "keyword" => {
            let k = Keyword::cast(node).expect("keyword view");
            assert_eq!(
                k.key().to_uppercase(),
                v["key"].as_str().unwrap_or(""),
                "{rel} key"
            );
            assert_eq!(k.value(), v["value"].as_str().unwrap_or(""), "{rel} value");
        }
        "node-property" => {
            let parent = node
                .parent()
                .and_then(PropertyDrawer::cast)
                .expect("node property in a property drawer");
            let props = parent.properties();
            let p = props
                .iter()
                .find(|p| p.node == node)
                .expect("property view");
            assert_eq!(
                p.key_as_written(),
                v["key"].as_str().unwrap_or(""),
                "{rel} property key"
            );
            assert_eq!(
                p.value,
                v["value"].as_str().unwrap_or(""),
                "{rel} property value"
            );
        }
        "special-block" => {
            assert_eq!(
                Block::cast(node).expect("block view").name(),
                v["block_type"].as_str().unwrap_or(""),
                "{rel} block type"
            );
        }
        "drawer" => {
            let name = node.children().next().and_then(|l| {
                l.children_with_tokens()
                    .filter_map(|t| t.into_token())
                    .find(|t| t.kind() == SyntaxKind::NAME)
            });
            assert_eq!(
                name.map(|t| t.text().to_string()),
                opt_str(&v["name"]),
                "{rel} drawer name"
            );
        }
        _ => return false,
    }
    true
}
