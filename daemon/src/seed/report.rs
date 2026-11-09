//! The dry-run report, as text for an operator and as JSON for a gate.
//!
//! Both forms carry the same sections. Every excluded and every unselected
//! path is listed by name, so nothing leaves the seed without a line saying so.

use serde_json::{json, Value};

use super::plan::{sha256_hex, PlannedNode, SeedPlan};
use super::CONTRACT;

/// The JSON report. Stable keys; see `render_text` for the meaning of each.
pub fn render_json(plan: &SeedPlan, exclude_requested: &[String]) -> Value {
    json!({
        "contract": CONTRACT,
        "dry_run": true,
        "ok": plan.ok(),
        "plan_hash": plan.plan_hash(),
        "source": {
            "repo": plan.source.repo,
            "rev": plan.source.rev,
            "commit": plan.source.commit,
            "tree": plan.source.tree,
        },
        "options": {
            "include": plan.include,
            "exclude": exclude_requested,
            "max_body_bytes": plan.max_body_bytes,
            "require_canonical": plan.require_canonical,
            "code_property": plan.code_property,
        },
        "counts": counts(plan),
        "nodes": plan.nodes.iter().map(node_json).collect::<Vec<_>>(),
        "refusals": plan.refusals.iter().map(|r| json!({
            "path": r.path, "kind": r.kind.as_str(), "detail": r.detail,
        })).collect::<Vec<_>>(),
        "excluded": plan.excluded,
        "non_canonical": plan.nodes.iter().filter_map(|n| n.header_diff.as_ref().map(|(removed, added)| json!({
            "path": n.path, "removed": removed, "added": added,
        }))).collect::<Vec<_>>(),
        "not_selected": plan.not_selected,
        "missing_code": plan.missing_code,
    })
}

fn counts(plan: &SeedPlan) -> Value {
    let canonical = plan.nodes.iter().filter(|n| n.canonical).count();
    json!({
        "nodes": plan.nodes.len(),
        "anchors": plan.anchor_count(),
        "body_bytes": plan.nodes.iter().map(|n| n.file.body.len()).sum::<usize>(),
        "canonical": canonical,
        "non_canonical": plan.nodes.len() - canonical,
        "refusals": plan.refusals.len(),
        "excluded": plan.excluded.len(),
        "not_selected": plan.not_selected.len(),
        "created_at_from_git": plan.nodes.iter().filter(|n| n.created_at_from_git).count(),
        "created_at_received": plan.nodes.iter().filter(|n| !n.created_at_from_git).count(),
        "missing_code": plan.missing_code.len(),
    })
}

fn node_json(n: &PlannedNode) -> Value {
    let f = &n.file;
    json!({
        "path": n.path,
        "blob": n.blob,
        "id": f.id,
        "kind": f.kind(),
        "code": n.code,
        "created_at": n.created_at,
        "created_at_source": if n.created_at_from_git { "git" } else { "received" },
        "title": f.title,
        "filetags": f.filetags,
        "properties": f.properties,
        "anchors": f.anchors.len(),
        "body_bytes": f.body.len(),
        "body_sha256": sha256_hex(f.body.as_bytes()),
        "canonical": n.canonical,
    })
}

/// The text report.
pub fn render_text(plan: &SeedPlan) -> String {
    let mut out = String::new();
    summary(plan, &mut out);
    section(
        &mut out,
        "refusals",
        plan.refusals.iter().map(|r| {
            let path = if r.path.is_empty() { "(run)" } else { &r.path };
            format!("{path}: {}: {}", r.kind.as_str(), r.detail)
        }),
    );
    section(
        &mut out,
        "excluded by --exclude",
        plan.excluded.iter().cloned(),
    );
    let non_canonical = plan.nodes.iter().filter(|n| !n.canonical).map(diff_lines);
    section(
        &mut out,
        "not canonical: N(F) differs from F",
        non_canonical,
    );
    section(
        &mut out,
        "not selected by --include",
        plan.not_selected.iter().cloned(),
    );
    if let Some(key) = &plan.code_property {
        let title = format!("missing document code (--code-property {key})");
        section(&mut out, &title, plan.missing_code.iter().cloned());
    }
    out
}

fn summary(plan: &SeedPlan, out: &mut String) {
    let s = &plan.source;
    let body_total: usize = plan.nodes.iter().map(|n| n.file.body.len()).sum();
    let largest = plan.nodes.iter().max_by_key(|n| n.file.body.len());
    let canonical = plan.nodes.iter().filter(|n| n.canonical).count();
    let from_git = plan.nodes.iter().filter(|n| n.created_at_from_git).count();
    let received = plan.nodes.len() - from_git;
    let result = if plan.ok() {
        format!("OK: would create one KB with {} nodes", plan.nodes.len())
    } else {
        format!(
            "REFUSED: {} refusal(s); nothing would be created",
            plan.refusals.len()
        )
    };
    let lines = [
        "mae-daemon kb seed: DRY RUN (nothing is written)".to_string(),
        format!("contract    {CONTRACT}"),
        format!("source      {} @ {}", s.repo, s.rev),
        format!("commit      {}", s.commit),
        format!("tree        {}", s.tree),
        format!("include     {}", plan.include.join(" ")),
        format!("max body    {} bytes", plan.max_body_bytes),
        format!("result      {result}"),
        format!(
            "plan_hash   {}",
            plan.plan_hash()
                .unwrap_or_else(|| "(none: the plan is refused)".into())
        ),
        format!(
            "nodes       {} (anchors {}, body bytes {})",
            plan.nodes.len(),
            plan.anchor_count(),
            body_total
        ),
        format!(
            "largest     {}",
            largest.map_or("-".into(), |n| format!(
                "{} bytes in {}",
                n.file.body.len(),
                n.path
            ))
        ),
        format!("canonical   {canonical} of {}", plan.nodes.len()),
        format!("created_at  {from_git} from git, {received} at receipt time (not reproducible)"),
    ];
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
}

fn section(out: &mut String, title: &str, items: impl Iterator<Item = String>) {
    let items: Vec<String> = items.collect();
    out.push_str(&format!("\n== {title} ({}) ==\n", items.len()));
    for i in items {
        out.push_str(&i);
        out.push('\n');
    }
}

fn diff_lines(n: &PlannedNode) -> String {
    let mut s = n.path.clone();
    if let Some((removed, added)) = &n.header_diff {
        for (sign, lines) in [('-', removed), ('+', added)] {
            for l in lines {
                s.push_str(&format!("\n  {sign} {}", show_line(l)));
            }
        }
    }
    s
}

fn show_line(l: &str) -> String {
    match l.strip_suffix('\n') {
        Some(body) => body.to_string(),
        None => format!("{l}  (no newline at end of file)"),
    }
}
