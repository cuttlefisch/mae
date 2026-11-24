//! The dry-run report, as text for an operator and as JSON for a gate.
//!
//! Both forms carry the same sections. Every excluded and every unselected
//! path is listed by name, so nothing leaves the seed without a line saying so.

use serde_json::{json, Value};

use super::plan::{sha256_hex, PlannedNode, SeedPlan, TimeSource};
use super::update::{NodeChange, OutsideEdit, UpdateDiff};
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
            "allow_received_times": plan.allow_received_times,
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
        "received_times": plan.received_times,
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
        "created_by_id": count(plan, |n| n.created_source == TimeSource::GitId),
        "created_received": count(plan, |n| n.created_source == TimeSource::Received),
        "modified_received": count(plan, |n| n.modified_source == TimeSource::Received),
        "history_blobs_read": plan.history_blobs_read,
        "history_diff_entries": plan.history_diff_entries,
        "objects_verified": plan.objects_verified,
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
        "created_at_source": n.created_source.as_str(),
        "modified_at": n.modified_at,
        "modified_at_source": n.modified_source.as_str(),
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
    render(plan, None)
}

/// The text report of `kb seed --update --dry-run`: the plan, then how it
/// differs from the KB as applied. `discard_edits` is whether the apply would
/// be given `--discard-edits`.
pub fn render_update_text(plan: &SeedPlan, d: &UpdateDiff, discard_edits: bool) -> String {
    let mut out = render(plan, Some(d));
    update_sections(d, discard_edits, &mut out);
    out
}

fn render(plan: &SeedPlan, update: Option<&UpdateDiff>) -> String {
    let mut out = String::new();
    summary(plan, update, &mut out);
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
    if plan.allow_received_times {
        let title = "receipt times, left out of plan_hash (--allow-received-times)";
        section(&mut out, title, plan.received_times.iter().cloned());
    }
    if let Some(key) = &plan.code_property {
        let title = format!("missing document code (--code-property {key})");
        section(&mut out, &title, plan.missing_code.iter().cloned());
    }
    out
}

fn summary(plan: &SeedPlan, update: Option<&UpdateDiff>, out: &mut String) {
    let s = &plan.source;
    let body_total: usize = plan.nodes.iter().map(|n| n.file.body.len()).sum();
    let largest = plan.nodes.iter().max_by_key(|n| n.file.body.len());
    let canonical = plan.nodes.iter().filter(|n| n.canonical).count();
    let by_id = count(plan, |n| n.created_source == TimeSource::GitId);
    let created_received = count(plan, |n| n.created_source == TimeSource::Received);
    let modified_received = count(plan, |n| n.modified_source == TimeSource::Received);
    let result = if let (true, Some(d)) = (plan.ok(), update) {
        format!(
            "OK: would update KB '{}' to {} nodes: {} added, {} changed, {} removed, {} \
             unchanged",
            d.kb_id,
            plan.nodes.len(),
            d.added.len(),
            d.changed.len(),
            d.removed.len(),
            d.unchanged
        )
    } else if plan.ok() {
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
        format!(
            "created_at  by :ID: first seen in history for {by_id}, receipt time for \
             {created_received} (not reproducible)"
        ),
        format!(
            "modified_at by git log -1 for {}, receipt time for {modified_received} (not \
             reproducible); {} history blobs read",
            plan.nodes.len() - modified_received,
            plan.history_blobs_read
        ),
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

fn count(plan: &SeedPlan, pred: impl Fn(&PlannedNode) -> bool) -> usize {
    plan.nodes.iter().filter(|n| pred(n)).count()
}

/// Whether `kb seed --update` would apply `d`: it refuses while a node is
/// edited outside the seed, unless `--discard-edits`.
pub fn update_would_apply(d: &UpdateDiff, discard_edits: bool) -> bool {
    d.edited.is_empty() || discard_edits
}

fn update_sections(d: &UpdateDiff, discard_edits: bool, out: &mut String) {
    let a = &d.applied;
    out.push_str(&format!(
        "\n== update of KB '{}' ==\nfrom plan   {} (rev {}, commit {}), applied {}\n",
        d.kb_id,
        a.plan_hash,
        a.rev,
        a.commit,
        a.applied_at.map_or("-".into(), crate::rfc3339::utc)
    ));
    if let Some(h) = &d.in_progress {
        out.push_str(&format!(
            "in progress an update to plan {h} was cut short; only that plan resumes it\n"
        ));
    }
    section(out, "added", d.added.iter().map(change_line));
    section(out, "changed", d.changed.iter().map(change_line));
    section(out, "removed", d.removed.iter().map(change_line));
    let title = "edited outside the seed since its last apply";
    section(out, title, d.edited.iter().map(edit_line));
    if !d.edited.is_empty() {
        out.push_str(if discard_edits {
            "--discard-edits: the apply overwrites these; each edit stays in its node's history\n"
        } else {
            "the apply refuses while these exist; --discard-edits overwrites them (each edit \
             stays in its node's history)\n"
        });
    }
}

fn change_line(c: &NodeChange) -> String {
    let mut s = format!("{} {}", c.node_id, c.path);
    if let Some(from) = &c.from_path {
        s.push_str(&format!(" (moved from {from})"));
    }
    if c.returns {
        s.push_str(" (its document returns, with its history)");
    }
    if !c.fields.is_empty() {
        s.push_str(&format!(": {}", c.fields.join(", ")));
    }
    s
}

fn edit_line(e: &OutsideEdit) -> String {
    let mut s = format!("{} {}", e.node_id, e.path);
    if let Some(f) = &e.fields {
        s.push_str(&format!(": {}", f.join(", ")));
    }
    if !e.editors.is_empty() {
        s.push_str(&format!("; by {}", e.editors.join(", ")));
    }
    if let Some(n) = &e.note {
        s.push_str(&format!("; {n}"));
    }
    s
}

/// The JSON report of `kb seed --update --dry-run`: [`render_json`] with an
/// `update` object.
pub fn render_update_json(
    plan: &SeedPlan,
    exclude_requested: &[String],
    d: &UpdateDiff,
    discard_edits: bool,
) -> Value {
    let mut report = render_json(plan, exclude_requested);
    let change = |c: &NodeChange| {
        json!({"node_id": c.node_id, "path": c.path, "from_path": c.from_path,
               "fields": c.fields, "returns": c.returns})
    };
    report["update"] = json!({
        "kb_id": d.kb_id,
        "from": {
            "plan_hash": d.applied.plan_hash,
            "rev": d.applied.rev,
            "commit": d.applied.commit,
            "applied_at": d.applied.applied_at,
        },
        "in_progress": d.in_progress,
        "added": d.added.iter().map(change).collect::<Vec<_>>(),
        "changed": d.changed.iter().map(change).collect::<Vec<_>>(),
        "removed": d.removed.iter().map(change).collect::<Vec<_>>(),
        "unchanged": d.unchanged,
        "edited_outside_seed": d.edited.iter().map(|e| json!({
            "node_id": e.node_id, "path": e.path, "fields": e.fields,
            "editors": e.editors, "note": e.note,
        })).collect::<Vec<_>>(),
        "would_apply": update_would_apply(d, discard_edits),
    });
    report
}
