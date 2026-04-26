//! AI Git tools — structured git operations.
//!
//! Tools for status, diff, log, commit, stage, push, pull.
//! These return structured data (JSON) or raw text for AI reasoning.

use mae_core::Editor;
use serde_json::json;
use std::process::Command;

/// Run a git command in the project root and return (success, stdout, stderr).
fn run_git(editor: &Editor, args: &[&str]) -> (bool, String, String) {
    let root = editor
        .active_project_root()
        .map(|p| p.to_path_buf())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();

    match Command::new("git").args(args).current_dir(&root).output() {
        Ok(output) => {
            let success = output.status.success();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            (success, stdout, stderr)
        }
        Err(e) => (false, String::new(), format!("failed to spawn git: {}", e)),
    }
}

pub fn execute_git_status(editor: &Editor) -> Result<String, String> {
    let (ok, stdout, stderr) = run_git(editor, &["status", "--porcelain", "--branch"]);
    if !ok {
        return Err(format!("git status failed: {}", stderr));
    }

    let mut lines = stdout.lines();
    let branch_line = lines.next().unwrap_or("## unknown");
    let branch = branch_line.strip_prefix("## ").unwrap_or(branch_line);

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    for line in lines {
        if line.len() < 4 {
            continue;
        }
        let status = &line[0..2];
        let path = &line[3..];

        match status {
            "M " | "A " | "D " | "R " | "C " => staged.push(path),
            " M" | " D" | " R" | " C" => unstaged.push(path),
            "??" => untracked.push(path),
            "MM" | "AM" | "DM" => {
                staged.push(path);
                unstaged.push(path);
            }
            _ => unstaged.push(path),
        }
    }

    let res = json!({
        "branch": branch,
        "staged": staged,
        "unstaged": unstaged,
        "untracked": untracked,
    });
    Ok(res.to_string())
}

pub fn execute_git_diff(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let mut git_args = vec!["diff"];
    if let Some(staged) = args.get("staged").and_then(|v| v.as_bool()) {
        if staged {
            git_args.push("--staged");
        }
    }
    if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
        git_args.push(path);
    }

    let (ok, stdout, stderr) = run_git(editor, &git_args);
    if !ok {
        return Err(format!("git diff failed: {}", stderr));
    }
    Ok(stdout)
}

pub fn execute_git_log(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .to_string();
    let mut git_args = vec!["log", "--oneline", "-n", &limit];

    if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
        git_args.push(path);
    }

    let (ok, stdout, stderr) = run_git(editor, &git_args);
    if !ok {
        return Err(format!("git log failed: {}", stderr));
    }
    Ok(stdout)
}

pub fn execute_git_stage(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let paths = args
        .get("paths")
        .and_then(|v| v.as_array())
        .ok_or("Missing 'paths' array")?;

    let mut git_args = vec!["add"];
    for p in paths {
        if let Some(s) = p.as_str() {
            git_args.push(s);
        }
    }

    let (ok, _stdout, stderr) = run_git(editor, &git_args);
    if !ok {
        return Err(format!("git add failed: {}", stderr));
    }
    Ok(format!("Staged {} paths", paths.len()))
}

pub fn execute_git_unstage(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let paths = args
        .get("paths")
        .and_then(|v| v.as_array())
        .ok_or("Missing 'paths' array")?;

    let mut git_args = vec!["reset", "HEAD", "--"];
    for p in paths {
        if let Some(s) = p.as_str() {
            git_args.push(s);
        }
    }

    let (ok, _stdout, stderr) = run_git(editor, &git_args);
    if !ok {
        return Err(format!("git reset failed: {}", stderr));
    }
    Ok(format!("Unstaged {} paths", paths.len()))
}

pub fn execute_git_commit(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let message = args
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'message' argument")?;

    let (ok, stdout, stderr) = run_git(editor, &["commit", "-m", message]);
    if !ok {
        return Err(format!("git commit failed: {}", stderr));
    }
    Ok(stdout)
}

pub fn execute_git_push(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let mut git_args = vec!["push"];
    if let Some(remote) = args.get("remote").and_then(|v| v.as_str()) {
        git_args.push(remote);
    }
    if let Some(branch) = args.get("branch").and_then(|v| v.as_str()) {
        git_args.push(branch);
    }

    let (ok, stdout, stderr) = run_git(editor, &git_args);
    if !ok {
        return Err(format!("git push failed: {}", stderr));
    }
    Ok(stdout)
}

pub fn execute_git_pull(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let mut git_args = vec!["pull"];
    if let Some(remote) = args.get("remote").and_then(|v| v.as_str()) {
        git_args.push(remote);
    }
    if let Some(branch) = args.get("branch").and_then(|v| v.as_str()) {
        git_args.push(branch);
    }

    let (ok, stdout, stderr) = run_git(editor, &git_args);
    if !ok {
        return Err(format!("git pull failed: {}", stderr));
    }
    Ok(stdout)
}

pub fn execute_git_checkout(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let branch = args
        .get("branch")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'branch' argument")?;
    let create = args
        .get("create")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut git_args = vec!["checkout"];
    if create {
        git_args.push("-b");
    }
    git_args.push(branch);

    let (ok, stdout, stderr) = run_git(editor, &git_args);
    if !ok {
        return Err(format!("git checkout failed: {}", stderr));
    }
    Ok(stdout)
}

pub fn execute_git_stash_push(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let mut git_args = vec!["stash", "push"];
    if let Some(msg) = args.get("message").and_then(|v| v.as_str()) {
        git_args.push("-m");
        git_args.push(msg);
    }

    let (ok, stdout, stderr) = run_git(editor, &git_args);
    if !ok {
        return Err(format!("git stash push failed: {}", stderr));
    }
    Ok(if stdout.is_empty() {
        "No local changes to save".into()
    } else {
        stdout
    })
}

pub fn execute_git_stash_pop(editor: &Editor) -> Result<String, String> {
    let (ok, stdout, stderr) = run_git(editor, &["stash", "pop"]);
    if !ok {
        return Err(format!("git stash pop failed: {}", stderr));
    }
    Ok(stdout)
}

pub fn execute_git_stash_list(editor: &Editor) -> Result<String, String> {
    let (ok, stdout, stderr) = run_git(editor, &["stash", "list"]);
    if !ok {
        return Err(format!("git stash list failed: {}", stderr));
    }
    Ok(if stdout.is_empty() {
        "No stashes".into()
    } else {
        stdout
    })
}

pub fn execute_git_branch_list(editor: &Editor) -> Result<String, String> {
    let (ok, stdout, stderr) = run_git(editor, &["branch", "-a", "--no-color"]);
    if !ok {
        return Err(format!("git branch failed: {}", stderr));
    }
    Ok(stdout)
}

pub fn execute_git_branch_delete(
    editor: &Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let branch = args
        .get("branch")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'branch' argument")?;
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

    let flag = if force { "-D" } else { "-d" };
    let (ok, stdout, stderr) = run_git(editor, &["branch", flag, branch]);
    if !ok {
        return Err(format!("git branch delete failed: {}", stderr));
    }
    Ok(stdout)
}

pub fn execute_git_merge(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let branch = args
        .get("branch")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'branch' argument")?;

    let (ok, stdout, stderr) = run_git(editor, &["merge", branch]);
    if !ok {
        return Err(format!("git merge failed: {}", stderr));
    }
    Ok(stdout)
}

pub fn execute_github_pr_status(editor: &Editor) -> Result<String, String> {
    let (ok1, stdout1, stderr1) = run_gh(editor, &["pr", "status"]);
    let (ok2, stdout2, stderr2) = run_gh(editor, &["pr", "checks"]);

    let mut out = String::new();
    if ok1 {
        out.push_str("--- PR Status ---\n");
        out.push_str(&stdout1);
        out.push('\n');
    } else {
        out.push_str(&format!("gh pr status failed: {}\n", stderr1));
    }

    if ok2 {
        out.push_str("--- PR Checks ---\n");
        out.push_str(&stdout2);
    } else {
        out.push_str(&format!("gh pr checks failed: {}\n", stderr2));
    }

    Ok(out)
}

pub fn execute_github_pr_create(
    editor: &Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'title' argument")?;
    let body = args
        .get("body")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'body' argument")?;

    let (ok, stdout, stderr) = run_gh(editor, &["pr", "create", "--title", title, "--body", body]);
    if !ok {
        return Err(format!("gh pr create failed: {}", stderr));
    }
    Ok(stdout)
}

fn run_gh(editor: &Editor, args: &[&str]) -> (bool, String, String) {
    let root = editor
        .active_project_root()
        .map(|p| p.to_path_buf())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();

    match Command::new("gh").args(args).current_dir(&root).output() {
        Ok(output) => {
            let success = output.status.success();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            (success, stdout, stderr)
        }
        Err(e) => (false, String::new(), format!("failed to spawn gh: {}", e)),
    }
}
