use mae_core::Editor;

pub fn execute_project_info(editor: &Editor) -> Result<String, String> {
    let info = serde_json::json!({
        "project": editor.project.as_ref().map(|p| serde_json::json!({
            "name": p.name,
            "root": p.root.display().to_string(),
            "has_config": p.config.is_some(),
        })),
        "recent_files": editor.recent_files.list().iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "show_line_numbers": editor.show_line_numbers,
        "relative_line_numbers": editor.relative_line_numbers,
        "word_wrap": editor.word_wrap,
    });
    serde_json::to_string_pretty(&info).map_err(|e| e.to_string())
}

pub fn execute_project_files(args: &serde_json::Value) -> Result<String, String> {
    let pattern = args.get("pattern").and_then(|v| v.as_str());

    // Try git ls-files first
    let output = std::process::Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output();

    let files = match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        _ => {
            // Fallback: list files recursively (limited depth)
            let output = std::process::Command::new("find")
                .args([
                    ".",
                    "-type",
                    "f",
                    "-not",
                    "-path",
                    "./.git/*",
                    "-maxdepth",
                    "5",
                ])
                .output()
                .map_err(|e| format!("Failed to list files: {}", e))?;
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|l| l.strip_prefix("./").unwrap_or(l).to_string())
                .collect::<Vec<_>>()
                .join("\n")
        }
    };

    // Filter by pattern if provided
    if let Some(pat) = pattern {
        let glob = glob::Pattern::new(pat).map_err(|e| format!("Invalid glob: {}", e))?;
        let filtered: Vec<&str> = files
            .lines()
            .filter(|line| {
                glob.matches(line) || glob.matches(line.rsplit('/').next().unwrap_or(line))
            })
            .collect();
        Ok(format!("{} files\n{}", filtered.len(), filtered.join("\n")))
    } else {
        let count = files.lines().count();
        Ok(format!("{} files\n{}", count, files))
    }
}

pub fn execute_project_search(args: &serde_json::Value) -> Result<String, String> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'pattern' argument")?;
    let glob_filter = args.get("glob").and_then(|v| v.as_str());
    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(100) as usize;

    // Try ripgrep first, fall back to grep
    let mut cmd = if which_exists("rg") {
        let mut c = std::process::Command::new("rg");
        c.args(["--line-number", "--no-heading", "--color=never"]);
        if let Some(g) = glob_filter {
            c.args(["--glob", g]);
        }
        c.args(["-m", &max_results.to_string(), pattern]);
        c
    } else {
        let mut c = std::process::Command::new("grep");
        c.args(["-rn", "--color=never"]);
        if let Some(g) = glob_filter {
            c.args(["--include", g]);
        }
        c.args([pattern, "."]);
        c
    };

    let output = cmd.output().map_err(|e| format!("Search failed: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Truncate to max_results lines
    let lines: Vec<&str> = stdout.lines().take(max_results).collect();
    let total = stdout.lines().count();
    let shown = lines.len();

    let mut result = lines.join("\n");
    if total > shown {
        result.push_str(&format!("\n... ({} more results truncated)", total - shown));
    }
    if result.is_empty() {
        result = "No matches found".into();
    }
    Ok(result)
}

pub fn execute_switch_project(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;
    let root = std::path::PathBuf::from(mae_core::file_picker::expand_tilde(path_str));
    if !root.is_dir() {
        return Err(format!("Not a directory: {}", path_str));
    }

    // Delegate to the human path rather than reimplementing a subset of it
    // (principles #3 and #8). Audit #600.5: this used to do only
    // `recent_projects.push` + `editor.project = ...`, skipping the three
    // things `add_project` also does — `update_project_list` (so the AI's
    // switch was never persisted and vanished on restart), `refresh_git_branch`
    // (so the status line kept showing the OLD project's branch), and
    // `lsp.pending_root_change` (so the language server kept the old workspace
    // root, leaving every subsequent `lsp_definition`/`lsp_diagnostics` the AI
    // made scoped to the project it thought it had left).
    editor.add_project(&root.to_string_lossy());

    let name = editor
        .project
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| path_str.to_string());
    Ok(format!("Switched to project '{}' at {}", name, path_str))
}

pub fn execute_save_memory(args: &serde_json::Value) -> Result<String, String> {
    let fact = args
        .get("fact")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'fact' argument".to_string())?;

    let mae_dir = std::env::current_dir()
        .map_err(|e| format!("failed to get current dir: {}", e))?
        .join(".mae/memory");

    if !mae_dir.exists() {
        std::fs::create_dir_all(&mae_dir)
            .map_err(|e| format!("failed to create memory dir: {}", e))?;
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let filename = format!("memory_{}.txt", timestamp);
    let path = mae_dir.join(filename);

    std::fs::write(&path, fact).map_err(|e| format!("failed to write memory: {}", e))?;

    Ok(format!("Fact remembered in {}", path.display()))
}

pub fn execute_create_plan(args: &serde_json::Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'name' argument".to_string())?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'content' argument".to_string())?;

    let plan_dir = std::env::current_dir()
        .map_err(|e| format!("failed to get current dir: {}", e))?
        .join(".mae/plans");

    if !plan_dir.exists() {
        std::fs::create_dir_all(&plan_dir)
            .map_err(|e| format!("failed to create plans dir: {}", e))?;
    }

    let filename = if name.ends_with(".md") {
        name.to_string()
    } else {
        format!("{}.md", name)
    };
    let path = plan_dir.join(filename);

    std::fs::write(&path, content).map_err(|e| format!("failed to write plan: {}", e))?;

    Ok(format!("Plan created: {}", path.display()))
}

pub fn execute_update_plan(args: &serde_json::Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'name' argument".to_string())?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'content' argument".to_string())?;

    let plan_dir = std::env::current_dir()
        .map_err(|e| format!("failed to get current dir: {}", e))?
        .join(".mae/plans");

    let filename = if name.ends_with(".md") {
        name.to_string()
    } else {
        format!("{}.md", name)
    };
    let path = plan_dir.join(filename);

    if !path.exists() {
        return Err(format!("Plan not found: {}", path.display()));
    }

    std::fs::write(&path, content).map_err(|e| format!("failed to update plan: {}", e))?;

    Ok(format!("Plan updated: {}", path.display()))
}

/// Check if a command exists on PATH.
fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod switch_project_tests {
    use super::*;

    /// Audit #600.5 — the AI's `switch_project` did only
    /// `recent_projects.push` + `editor.project = ...`. The human `add-project`
    /// path additionally persists the project list, refreshes the git branch,
    /// and arms `lsp.pending_root_change`. So after the AI switched projects,
    /// its own subsequent `lsp_definition`/`lsp_diagnostics` calls were still
    /// answered by a language server rooted in the PREVIOUS project — the AI
    /// peer silently operating on stale semantic data, which is exactly the
    /// asymmetry principle #3 exists to prevent.
    ///
    /// Asserts on the observable side effects, not on "it returned Ok".
    #[test]
    fn switching_projects_arms_everything_the_human_path_does() {
        let a = tempfile::tempdir().expect("project a");
        let b = tempfile::tempdir().expect("project b");

        let mut editor = Editor::new();
        // Start in project A via the human path, so the "before" state is a
        // realistic mid-session one rather than a blank editor.
        editor.add_project(&a.path().to_string_lossy());
        editor.lsp.pending_root_change = None; // consumed by the bridge

        let result = execute_switch_project(
            &mut editor,
            &serde_json::json!({ "path": b.path().to_string_lossy() }),
        )
        .expect("switch should succeed");
        assert!(result.contains("Switched to project"), "{result}");

        assert_eq!(
            editor.project.as_ref().map(|p| p.root.clone()),
            Some(b.path().to_path_buf()),
            "the active project must be B"
        );
        // The effects that were missing.
        assert_eq!(
            editor.lsp.pending_root_change.as_deref(),
            Some(format!("file://{}", b.path().display()).as_str()),
            "the LSP workspace root must be re-armed, or every later lsp_* call \
             the AI makes answers from the OLD project"
        );
        // NB: `recent_projects` is deliberately NOT asserted here —
        // `RecentProjects::push` silently rejects paths under the system temp
        // dir, so a tempdir fixture can never observe it. The LSP root above is
        // the effect that actually mattered for the AI peer.
    }

    /// The negative case: a path that is not a directory must be refused
    /// outright, leaving the current project untouched — not partially applied.
    #[test]
    fn a_non_directory_is_refused_without_disturbing_the_current_project() {
        let a = tempfile::tempdir().expect("project a");
        let file = a.path().join("not-a-dir.txt");
        std::fs::write(&file, "x").unwrap();

        let mut editor = Editor::new();
        editor.add_project(&a.path().to_string_lossy());
        let before_root = editor.project.as_ref().map(|p| p.root.clone());
        editor.lsp.pending_root_change = None;

        for bad in [
            file.to_string_lossy().to_string(),
            a.path()
                .join("does-not-exist")
                .to_string_lossy()
                .to_string(),
            String::new(),
        ] {
            let err = execute_switch_project(&mut editor, &serde_json::json!({ "path": bad }))
                .expect_err("must refuse");
            assert!(err.contains("Not a directory"), "{err}");
        }

        assert_eq!(
            editor.project.as_ref().map(|p| p.root.clone()),
            before_root,
            "a refused switch must leave the active project alone"
        );
        assert!(
            editor.lsp.pending_root_change.is_none(),
            "a refused switch must not re-root the language server"
        );
    }
}
