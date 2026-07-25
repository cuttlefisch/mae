//! CI staleness gate (ADR-059 Phase E): fails if a commit changes an ADR file's structured
//! header fields (`Status`/`Extends`/`Relates to`/`Depends on`/`Supersedes`) without also
//! regenerating `assets/mae-adr.cozo` (i.e. running `make adr-kb` and committing the
//! result) — but does NOT fail on a prose-only change to an ADR's body, which the ADR KB's
//! generated node content also embeds but which this check deliberately does not require a
//! rebuild for (ADR-059 Phase C's own explicit "must not over-trigger on ordinary editorial
//! fixes" requirement).
//!
//! Scoped precisely to the 5 structured fields, not a blanket "did the file change at all"
//! check — comparing the *parsed metadata*, not the raw diff, is what lets this distinguish
//! a header edit from a prose edit without false-positiving on the common case (fixing a
//! typo in an ADR's Context section).
//!
//! Usage: `verify-adr-kb-sync [--base <git-ref>]` (default base: `HEAD~1`).

use mae_kb::adr_parse::{parse_adr_str, AdrMetadata};
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let base = args
        .iter()
        .position(|a| a == "--base")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "HEAD~1".to_string());

    let adr_dir = PathBuf::from("docs/adr");
    if !adr_dir.is_dir() {
        eprintln!("docs/adr/ not found -- run from the workspace root.");
        std::process::exit(2);
    }

    let changed = git_changed_adr_files(&base);
    if changed.is_empty() {
        println!("No docs/adr/*.md files changed relative to {base} — nothing to check.");
        return;
    }

    let mut header_changed_files: Vec<String> = Vec::new();
    for rel_path in &changed {
        let old_content = git_show(&base, rel_path);
        let new_content = std::fs::read_to_string(rel_path).unwrap_or_default();

        let old_meta = old_content
            .as_deref()
            .and_then(|c| parse_adr_str(c, rel_path).ok());
        let new_meta = parse_adr_str(&new_content, rel_path).ok();

        match (old_meta, new_meta) {
            (Some(old), Some(new)) if structured_fields_differ(&old, &new) => {
                header_changed_files.push(rel_path.clone());
            }
            (None, Some(_)) => {
                // A brand-new ADR file — its header is new by definition, requires a regen
                // to actually appear in the ADR KB.
                header_changed_files.push(rel_path.clone());
            }
            _ => {}
        }
    }

    if header_changed_files.is_empty() {
        println!(
            "{} ADR file(s) changed, none touched a structured header field \
             (Status/Extends/Relates to/Depends on/Supersedes) — prose-only, no regen required.",
            changed.len()
        );
        return;
    }

    let checksum_changed = git_diff_touches(&base, "assets/mae-adr.cozo.sha256");
    if checksum_changed {
        println!(
            "Header field(s) changed in {} file(s) and assets/mae-adr.cozo.sha256 was \
             updated in the same range — ADR KB is in sync.",
            header_changed_files.len()
        );
        return;
    }

    eprintln!("❌ ADR KB is stale relative to header changes in:");
    for f in &header_changed_files {
        eprintln!("   - {f}");
    }
    eprintln!(
        "\nassets/mae-adr.cozo.sha256 was NOT updated relative to {base}. Run 'make adr-kb' \
         and commit the regenerated assets/mae-adr.cozo + assets/mae-adr.cozo.sha256."
    );
    std::process::exit(1);
}

/// Which docs/adr/*.md files differ between `base` and the working tree.
fn git_changed_adr_files(base: &str) -> Vec<String> {
    let output = Command::new("git")
        .args(["diff", "--name-only", base, "--", "docs/adr/*.md"])
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// `git show <base>:<path>` — `None` if the path didn't exist at `base` (a new file).
fn git_show(base: &str, path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["show", &format!("{base}:{path}")])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

/// Whether `path` differs between `base` and the working tree.
fn git_diff_touches(base: &str, path: &str) -> bool {
    Command::new("git")
        .args(["diff", "--quiet", base, "--", path])
        .status()
        .map(|s| !s.success())
        .unwrap_or(true) // if we can't tell, fail safe (assume it needs checking)
}

/// Compare only the 5 structured relationship/status fields Phase E's gate is scoped to —
/// deliberately excludes `title`/`slug`/`tracking` (a title rewording or an issue-number
/// update in Tracking doesn't require an ADR-KB rebuild the way a Status/Extends/etc.
/// change does).
fn structured_fields_differ(old: &AdrMetadata, new: &AdrMetadata) -> bool {
    old.status_raw != new.status_raw
        || old.extends != new.extends
        || old.relates_to != new.relates_to
        || old.depends_on != new.depends_on
        || old.supersedes != new.supersedes
}
