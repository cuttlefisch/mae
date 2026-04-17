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

/// Check if a command exists on PATH.
fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
