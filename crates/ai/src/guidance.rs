//! Always-on AI guidance context: project files (`CLAUDE.md`/`README.md`/...)
//! and a designated "guidance KB" whose content should be treated as
//! standing practices/guidance an agent must follow.
//!
//! Shared by every AI-facing surface — `mae-agent-cli`'s system prompt (the
//! default surface, ADR-049), the legacy embedded `ai_chat` system prompt
//! (`crates/mae/src/bootstrap.rs::build_system_prompt_with_model`), and the
//! MCP `initialize` response's `instructions` field (`shared/mcp`) — so this
//! logic isn't duplicated per surface. Previously only the deprecated
//! `ai_chat` path read project context at all; `mae-agent-cli` had a
//! hardcoded system prompt with no override.

use mae_kb::KbStore;
use std::path::{Path, PathBuf};

const PROJECT_CONTEXT_FILES: &[&str] = &["CLAUDE.md", "README.md", "README.org", ".project"];
const PROJECT_CONTEXT_MAX_CHARS: usize = 8000;

/// MAE's XDG-first data dir (`$XDG_DATA_HOME/mae`, else `~/.local/share/mae`)
/// — mirrors `Editor::mae_data_dir()`'s resolution exactly (CLAUDE.md
/// principle #13: XDG-first on all platforms), so a separate process (e.g.
/// `mae-agent-cli`, which has no `Editor` instance of its own) can find the
/// same `kb-registry.toml` a running editor reads/writes. `None` if neither
/// `XDG_DATA_HOME` nor `HOME` is set.
pub fn default_data_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        Some(PathBuf::from(xdg).join("mae"))
    } else if let Ok(home) = std::env::var("HOME") {
        Some(PathBuf::from(home).join(".local").join("share").join("mae"))
    } else {
        None
    }
}

/// Read the first matching project-context file from `cwd` (`CLAUDE.md` >
/// `README.md` > `README.org` > `.project`), truncated to a bounded size,
/// formatted as a `## Project Context (FILENAME)` markdown section.
/// `None` if no such file exists or none could be read.
pub fn read_project_context(cwd: &Path) -> Option<String> {
    for filename in PROJECT_CONTEXT_FILES {
        let path = cwd.join(filename);
        if !path.exists() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            let truncated = if content.len() > PROJECT_CONTEXT_MAX_CHARS {
                format!("{}...\n[truncated]", &content[..PROJECT_CONTEXT_MAX_CHARS])
            } else {
                content
            };
            return Some(format!(
                "\n## Project Context ({filename})\n```\n{truncated}\n```\n"
            ));
        }
    }
    None
}

/// Read a designated "guidance KB"'s content — standing practices an AI
/// agent should treat as required, not optional. `guidance_kb` names a
/// registered federated KB instance (see `:kb-register`/`kb_register`);
/// empty disables this. Kept deliberately simple for v1: the KB's `index`
/// node body (its root/overview content), not a full crawl or
/// embedding-based summary — and scoped to registered instances only, not
/// `primary` (whose store path/engine resolution is an editor-bootstrap
/// concern this crate doesn't own). Best-effort: any failure (KB not
/// registered, store unopenable, no `index` node) returns `None` rather
/// than erroring — a missing/misconfigured guidance KB must never break
/// session startup.
pub fn read_guidance_kb_context(data_dir: &Path, guidance_kb: &str) -> Option<String> {
    if guidance_kb.is_empty() {
        return None;
    }
    let registry = mae_kb::federation::KbRegistry::load(data_dir);
    let instance = registry.find(guidance_kb)?;
    let store = mae_kb::CozoKbStore::open_with_engine(&instance.db_path, "sqlite")
        .or_else(|_| mae_kb::CozoKbStore::open_with_engine(&instance.db_path, "sled"))
        .ok()?;
    let node = store.get_node("index").ok().flatten()?;
    Some(format!(
        "\n## Required Practices (KB: {guidance_kb})\n{}\n",
        node.body
    ))
}

/// Build the full guidance-context block (project files + designated
/// guidance KB) for injection into an AI agent's system prompt or MCP
/// `instructions`. `None` if neither is configured — a pure no-op default,
/// so existing behavior for users who haven't opted in is unchanged.
pub fn build_guidance_context(
    cwd: &Path,
    data_dir: Option<&Path>,
    guidance_kb: &str,
) -> Option<String> {
    let mut out = String::new();
    if let Some(ctx) = read_project_context(cwd) {
        out.push_str(&ctx);
    }
    if let Some(data_dir) = data_dir {
        if let Some(ctx) = read_guidance_kb_context(data_dir, guidance_kb) {
            out.push_str(&ctx);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_data_dir_prefers_xdg_data_home() {
        let _lock = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_DATA_HOME", "/tmp/mae-test-xdg-data");
        assert_eq!(
            default_data_dir(),
            Some(PathBuf::from("/tmp/mae-test-xdg-data/mae"))
        );
        match prev {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }

    #[test]
    fn read_project_context_none_when_no_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_project_context(tmp.path()).is_none());
    }

    #[test]
    fn read_project_context_prefers_claude_md_over_readme() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("README.md"), "readme content").unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "claude content").unwrap();
        let ctx = read_project_context(tmp.path()).unwrap();
        assert!(ctx.contains("CLAUDE.md"));
        assert!(ctx.contains("claude content"));
        assert!(!ctx.contains("readme content"));
    }

    #[test]
    fn read_project_context_falls_back_through_the_priority_list() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".project"), "project content").unwrap();
        let ctx = read_project_context(tmp.path()).unwrap();
        assert!(ctx.contains(".project"));
        assert!(ctx.contains("project content"));
    }

    #[test]
    fn read_project_context_truncates_oversized_files() {
        let tmp = tempfile::tempdir().unwrap();
        let big = "x".repeat(PROJECT_CONTEXT_MAX_CHARS + 500);
        std::fs::write(tmp.path().join("CLAUDE.md"), &big).unwrap();
        let ctx = read_project_context(tmp.path()).unwrap();
        assert!(ctx.contains("[truncated]"));
        assert!(ctx.len() < big.len());
    }

    #[test]
    fn read_guidance_kb_context_none_when_unset() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_guidance_kb_context(tmp.path(), "").is_none());
    }

    #[test]
    fn read_guidance_kb_context_none_when_kb_not_registered() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_guidance_kb_context(tmp.path(), "no-such-kb").is_none());
    }

    #[test]
    fn read_guidance_kb_context_returns_index_node_body() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("guidance.cozo");
        let store = mae_kb::CozoKbStore::open_with_engine(&db_path, "sqlite").unwrap();
        store.seed_type_system().unwrap();
        store
            .insert_node(&mae_kb::Node::new(
                "index",
                "Practices Index",
                mae_kb::NodeKind::Index,
                "Always write tests first.",
            ))
            .unwrap();
        drop(store);

        let mut registry = mae_kb::federation::KbRegistry::default();
        registry.instances.push(mae_kb::federation::KbInstance {
            uuid: "uuid-guidance".into(),
            name: "dev-practices".into(),
            org_dir: std::path::PathBuf::new(),
            db_path,
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
        });
        std::fs::write(
            tmp.path().join("kb-registry.toml"),
            toml::to_string(&registry).unwrap(),
        )
        .unwrap();

        let ctx = read_guidance_kb_context(tmp.path(), "dev-practices").unwrap();
        assert!(ctx.contains("dev-practices"));
        assert!(ctx.contains("Always write tests first."));
    }

    #[test]
    fn build_guidance_context_none_when_nothing_configured() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(build_guidance_context(tmp.path(), Some(tmp.path()), "").is_none());
        assert!(build_guidance_context(tmp.path(), None, "").is_none());
    }

    #[test]
    fn build_guidance_context_combines_both_sections() {
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(cwd.path().join("CLAUDE.md"), "project rules").unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let db_path = data_dir.path().join("guidance.cozo");
        let store = mae_kb::CozoKbStore::open_with_engine(&db_path, "sqlite").unwrap();
        store.seed_type_system().unwrap();
        store
            .insert_node(&mae_kb::Node::new(
                "index",
                "Index",
                mae_kb::NodeKind::Index,
                "kb guidance body",
            ))
            .unwrap();
        drop(store);
        let mut registry = mae_kb::federation::KbRegistry::default();
        registry.instances.push(mae_kb::federation::KbInstance {
            uuid: "uuid-guidance".into(),
            name: "dev-practices".into(),
            org_dir: std::path::PathBuf::new(),
            db_path,
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
        });
        std::fs::write(
            data_dir.path().join("kb-registry.toml"),
            toml::to_string(&registry).unwrap(),
        )
        .unwrap();

        let ctx =
            build_guidance_context(cwd.path(), Some(data_dir.path()), "dev-practices").unwrap();
        assert!(ctx.contains("project rules"));
        assert!(ctx.contains("kb guidance body"));
    }
}
