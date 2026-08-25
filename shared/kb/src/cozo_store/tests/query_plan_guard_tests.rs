//! The regression guard for the unbound-key defect: the sixteen edits in
//! `cozo_store/` were symptom cleanup, this file is the fix (principle #8).
//!
//! CozoDB compiles `*rel{k}, k = $x` to a full relation scan and
//! `k = $x, *rel{k}` to a prefix seek — see `query_plan_tests.rs` for the
//! mechanism and the plan-level proof. `*rel{k}, is_in(k, $ids)` is the same
//! defect wearing a different hat, and was the single most expensive query in
//! the KB. The difference is invisible in review, costs two orders of
//! magnitude at 20,000 nodes, and had been written sixteen times in this
//! module alone by 2026-08. So the shape must not be *writable*, not merely
//! currently absent.

/// Ordered key columns of every relation declared in `schema.rs`.
///
/// Parsed from the `:create` DDL rather than hardcoded, so adding a key column
/// re-derives the rule instead of silently invalidating it.
fn key_columns_from_schema() -> std::collections::HashMap<String, Vec<String>> {
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cozo_store/schema.rs"),
    )
    .expect("read schema.rs");

    let mut out = std::collections::HashMap::new();
    let mut rest = schema.as_str();
    while let Some(at) = rest.find(":create ") {
        rest = &rest[at + ":create ".len()..];
        let Some(open) = rest.find('{') else { break };
        let name = rest[..open].trim().to_string();
        let Some(close) = rest.find('}') else { break };
        let body = &rest[open + 1..close];
        // Everything before `=>` is the key tuple; with no `=>`, every column
        // is a key.
        let keys = body.split("=>").next().unwrap_or("");
        let cols: Vec<String> = keys
            .split(',')
            .filter_map(|c| {
                let c = c.split(':').next()?.trim();
                (!c.is_empty() && c.chars().all(|ch| ch.is_alphanumeric() || ch == '_'))
                    .then(|| c.to_string())
            })
            .collect();
        if !name.is_empty() && !cols.is_empty() {
            out.insert(name, cols);
        }
        rest = &rest[close..];
    }
    assert!(
        out.get("links").map(Vec::as_slice)
            == Some(&["src".into(), "dst".into(), "rel_type".into()][..]),
        "schema.rs parse drifted — got {:?} for `links`",
        out.get("links")
    );
    out
}

/// One reported offence: relation, the unbound key column, and the rule.
#[derive(Debug, PartialEq)]
struct Offence {
    relation: String,
    column: String,
    excerpt: String,
}

/// Find every `*rel{...}` atom whose key prefix could have been bound before
/// the atom but is instead equated *after* it.
///
/// Deliberately schema-aware rather than a plain grep: a post-filter on a
/// column that is NOT a bindable key prefix (`links_to`'s `dst`,
/// `links_typed`'s trailing `rel_type`) is correct and must not be reported,
/// while a post-filter on one that IS must be. `join_is_prefix`
/// (`cozo-0.7.6/src/query/ra.rs`) is the rule being encoded: the bound key
/// positions must be exactly `0..n`.
fn scan_for_post_filtered_keys(
    source: &str,
    keys: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<Offence> {
    let mut offences = Vec::new();
    let mut i = 0;
    while let Some(rel_at) = source[i..].find(":= ").map(|p| i + p) {
        // A rule body runs from `:=` to the next `\n` that begins a Cozo
        // clause (`:rm`/`:put`/`:order`/`:limit`/`:replace`) or the end of the
        // string literal.
        let body_start = rel_at + 3;
        let mut body_end = source.len();
        for pat in [
            "\\n:", "\n:", " :order", " :limit", " :rm", " :put", "\"", "#",
        ] {
            if let Some(p) = source[body_start..].find(pat) {
                body_end = body_end.min(body_start + p);
            }
        }
        // Normalise whitespace runs to single spaces so the shapes below match
        // regardless of how the Rust literal wraps. `get_node` was written
        // across four lines with the equality on its own — a line-oriented
        // grep, and any matcher working on the raw text, misses exactly that.
        let body: String = {
            let mut out = String::with_capacity(body_end - body_start);
            let mut in_ws = false;
            for ch in source[body_start..body_end].chars() {
                if ch.is_whitespace() {
                    in_ws = true;
                } else {
                    if in_ws && !out.is_empty() {
                        out.push(' ');
                    }
                    in_ws = false;
                    out.push(ch);
                }
            }
            out
        };
        let body = body.as_str();
        i = body_end.max(rel_at + 3);

        // Every stored-relation atom in this rule body, in source order.
        let mut atoms: Vec<(usize, String, usize)> = Vec::new(); // (start, name, end)
        let mut j = 0;
        while let Some(star) = body[j..].find('*').map(|p| j + p) {
            let name_start = star + 1;
            let name_end = name_start
                + body[name_start..]
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(body.len() - name_start);
            let name = &body[name_start..name_end];
            if body[name_end..].trim_start().starts_with('{') {
                let brace = name_end + body[name_end..].find('{').unwrap();
                if let Some(close) = body[brace..].find('}') {
                    atoms.push((star, name.to_string(), brace + close + 1));
                    j = brace + close + 1;
                    continue;
                }
            }
            j = name_start;
        }

        for (start, name, end) in atoms {
            let Some(key_cols) = keys.get(&name) else {
                continue;
            };
            let atom = &body[start..end];
            let before = &body[..start];
            let after = &body[end..];

            // Columns already bound when this atom compiles: inline binds in
            // the atom itself, plus any `col = ...` unification earlier in the
            // body (cozo's `seen_variables` is populated in source order).
            let mut bound: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for col in key_cols {
                if atom.contains(&format!("{col}:")) {
                    bound.insert(col);
                }
                for pat in [format!("{col} = "), format!("{col}=")] {
                    if before.contains(&pat) {
                        bound.insert(col);
                    }
                }
            }

            for col in key_cols {
                if bound.contains(col.as_str()) {
                    continue;
                }
                // Two shapes, both of which leave the key unbound at the
                // relation atom: an equality *after* it, and an `is_in`
                // membership test anywhere in the body. `is_in` is
                // `right.contains(left)` — a linear `Vec` probe run once per
                // scanned row — so it is strictly worse than the equality.
                let post_filtered = [format!(", {col} = "), format!(",{col} = ")]
                    .iter()
                    .any(|pat| after.contains(pat.as_str()))
                    || [format!("is_in({col},"), format!("is_in({col} ,")]
                        .iter()
                        .any(|pat| body.contains(pat.as_str()));
                if !post_filtered {
                    continue;
                }
                // Would binding it have produced a prefix? Only if `bound` plus
                // this column is exactly the leading `n` key columns.
                let mut candidate = bound.clone();
                candidate.insert(col);
                let is_prefix = key_cols
                    .iter()
                    .take(candidate.len())
                    .all(|c| candidate.contains(c.as_str()));
                if is_prefix {
                    offences.push(Offence {
                        relation: name.clone(),
                        column: col.clone(),
                        excerpt: body.trim().chars().take(140).collect(),
                    });
                }
            }
        }
    }
    offences
}

/// The guard itself, and the reason this file exists: the sixteen edits are
/// symptom cleanup, this is the fix (principle #8). A post-filtered key prefix
/// costs a full relation scan, so it must not be *writable*, not merely
/// currently absent.
///
/// **Coverage boundary, stated rather than assumed:** this scans `mae-kb`'s
/// own `src/`, which is where `schema.rs` (the key definitions) and every
/// relation-owning query live. Datalog written *outside* this crate against
/// the same relations is not covered — `crates/ai/src/tool_impls/kb.rs`'s
/// `kb_view_query` had the identical defect and was fixed by hand. Extending
/// the scan across the workspace needs the schema map to be reachable from
/// there first.
#[test]
fn cozo_store_never_post_filters_a_bindable_key_prefix() {
    let keys = key_columns_from_schema();
    let mut offences = Vec::new();
    let mut scanned = 0;
    let mut queue = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = queue.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src/") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                queue.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            // Test modules are excluded: this very file carries the banned
            // shapes on purpose, as fixtures for
            // `the_guard_catches_the_form_it_exists_to_ban`, and test queries
            // are not hot paths.
            if path.components().any(|c| c.as_os_str() == "tests") {
                continue;
            }
            scanned += 1;
            let source = std::fs::read_to_string(&path).expect("read source");
            for offence in scan_for_post_filtered_keys(&source, &keys) {
                offences.push(format!(
                    "{}: *{}{{...}}, {} = ...  <-- bind it BEFORE the atom\n    {}",
                    path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&path)
                        .display(),
                    offence.relation,
                    offence.column,
                    offence.excerpt
                ));
            }
        }
    }
    assert!(
        scanned >= 20,
        "only scanned {scanned} files — path is wrong"
    );
    assert!(
        offences.is_empty(),
        "{} Datalog rule(s) post-filter a key column that could have been bound before the \
         relation atom. Cozo compiles that to a FULL RELATION SCAN (compile.rs's \
         `seen_variables` gate). Rewrite `*rel{{k}}, k = $x` as `k = $x, *rel{{k}}`:\n\n{}",
        offences.len(),
        offences.join("\n\n")
    );
}

/// The guard's own adversarial test: a scanner that never fires is worthless.
/// Feeds it the exact shapes it must catch and the exact shapes it must not.
#[test]
fn the_guard_catches_the_form_it_exists_to_ban() {
    let keys = key_columns_from_schema();

    let must_fire = [
        // The classic: single-column key, post-filtered.
        r#"?[id, title] := *nodes{id, title}, id = $id"#,
        // Multi-line, as `get_node` was written — a line-oriented grep misses this.
        "?[id, title]\n    := *nodes{id, title},\n       id = $id",
        // Second key column post-filtered while the first is already bound.
        r#"?[title] := id = $id, *node_versions{id, version, title}, version = $version"#,
        // Inline-bound first column, post-filtered second.
        r#"?[position] := *meta_members{meta_id: $m, member_id, position}, member_id = $x"#,
        // A `:rm` head, which is where the temptation to post-filter is highest.
        "?[src, dst, rel_type] := *links{src, dst, rel_type}, src = $id\n:rm links {src, dst, rel_type}",
        // The `is_in` variant — the shape that cost 59 ms per KB search.
        r#"?[id, title, body] := *nodes{id, title, body}, is_in(id, $ids)"#,
    ];
    for src in must_fire {
        let found = scan_for_post_filtered_keys(src, &keys);
        assert!(!found.is_empty(), "guard failed to fire on:\n{src}");
    }

    let must_not_fire = [
        // The fixed form.
        r#"?[id, title] := id = $id, *nodes{id, title}"#,
        r#"?[title] := *nodes{id: $id, title}"#,
        // `dst` is key position 1 with `src` unbound — binding it is NOT a
        // prefix, so the post-filter is correct and must not be flagged.
        r#"?[src, dst, rel_type] := *links{src, dst, rel_type}, dst = $id"#,
        // `rel_type` is position 2 with `dst` unbound — likewise correct.
        r#"?[src, dst, rel_type] := src = $id, *links{src, dst, rel_type}, rel_type = $rel_type"#,
        // A non-key column is never seekable.
        r#"?[id] := *nodes{id, title}, title = $t"#,
        // A relation this module does not declare.
        r#"?[a] := *unknown_rel{a}, a = $x"#,
        // The `is_in` replacement: a candidate relation joined on the key.
        "cand[id] <- $ids\n?[id, title, body] := cand[id], *nodes{id, title, body}",
        // `is_in` on a non-key column cannot seek either way.
        r#"?[id] := *nodes{id, title}, is_in(title, $ts)"#,
    ];
    for src in must_not_fire {
        let found = scan_for_post_filtered_keys(src, &keys);
        assert!(
            found.is_empty(),
            "guard false-positived on:\n{src}\n{found:?}"
        );
    }
}
