//! # Module: pkg/git.rs — Git operations for package management
//!
//! Shells out to `git(1)` for clone, fetch, and checkout operations.
//! Uses `--depth 1` shallow clones (Doom's full clones are its biggest perf pain point).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Parsed package source.
#[derive(Debug, Clone, PartialEq)]
pub enum PackageSource {
    /// `github:user/repo` shorthand.
    GitHub { user: String, repo: String },
    /// Full git URL.
    GitUrl(String),
    /// `path:./relative` or `path:/absolute` — local module for development.
    Local(PathBuf),
}

impl PackageSource {
    /// Parse a source spec string.
    ///
    /// Accepted formats:
    /// - `github:user/repo`
    /// - `https://...` or `git@...` (any git URL)
    /// - `path:./relative` or `path:/absolute` (local module)
    pub fn parse(spec: &str) -> Result<Self, String> {
        if let Some(rest) = spec.strip_prefix("github:") {
            let parts: Vec<&str> = rest.splitn(2, '/').collect();
            if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
                return Err(format!(
                    "Invalid github source: '{}' (expected github:user/repo)",
                    spec
                ));
            }
            Ok(PackageSource::GitHub {
                user: parts[0].to_string(),
                repo: parts[1].to_string(),
            })
        } else if let Some(rest) = spec.strip_prefix("path:") {
            if rest.is_empty() {
                return Err("Empty path in path: source".to_string());
            }
            Ok(PackageSource::Local(PathBuf::from(rest)))
        } else if spec.starts_with("https://")
            || spec.starts_with("git@")
            || spec.starts_with("ssh://")
        {
            Ok(PackageSource::GitUrl(spec.to_string()))
        } else {
            Err(format!(
                "Unknown source format: '{}' (expected github:user/repo, path:..., or git URL)",
                spec
            ))
        }
    }

    /// Return the git clone URL.  For `Local` sources, returns the path string.
    pub fn clone_url(&self) -> String {
        match self {
            PackageSource::GitHub { user, repo } => {
                format!("https://github.com/{}/{}.git", user, repo)
            }
            PackageSource::GitUrl(url) => url.clone(),
            PackageSource::Local(path) => path.display().to_string(),
        }
    }

    /// Infer a package name from the source.
    pub fn inferred_name(&self) -> String {
        match self {
            PackageSource::GitHub { repo, .. } => {
                repo.strip_prefix("mae-").unwrap_or(repo).to_string()
            }
            PackageSource::GitUrl(url) => {
                let name = url.rsplit('/').next().unwrap_or("unknown");
                let name = name.strip_suffix(".git").unwrap_or(name);
                name.strip_prefix("mae-").unwrap_or(name).to_string()
            }
            PackageSource::Local(path) => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                name.strip_prefix("mae-").unwrap_or(name).to_string()
            }
        }
    }

    /// Returns true if this is a local path source.
    pub fn is_local(&self) -> bool {
        matches!(self, PackageSource::Local(_))
    }
}

/// Shallow clone a git repository to the target directory.
pub fn shallow_clone(url: &str, target: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(target)
        .output()
        .map_err(|e| format!("Failed to run git clone: {} (is git installed?)", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Clean up partial clone
        let _ = std::fs::remove_dir_all(target);
        Err(format!("git clone failed: {}", stderr.trim()))
    }
}

/// Fetch latest from origin in an existing repo.
pub fn fetch_latest(repo: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args(["fetch", "--depth", "1", "origin"])
        .current_dir(repo)
        .output()
        .map_err(|e| format!("Failed to run git fetch: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git fetch failed: {}", stderr.trim()))
    }
}

/// Get the HEAD SHA of a repository.
pub fn head_sha(repo: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .map_err(|e| format!("Failed to run git rev-parse: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err("Failed to get HEAD SHA".to_string())
    }
}

/// Checkout a specific SHA in a repository.
pub fn checkout_sha(repo: &Path, sha: &str) -> Result<(), String> {
    let output = Command::new("git")
        .args(["checkout", sha])
        .current_dir(repo)
        .output()
        .map_err(|e| format!("Failed to run git checkout: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git checkout {} failed: {}", sha, stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_source() {
        let src = PackageSource::parse("github:someuser/mae-org-roam").unwrap();
        assert_eq!(
            src,
            PackageSource::GitHub {
                user: "someuser".to_string(),
                repo: "mae-org-roam".to_string()
            }
        );
    }

    #[test]
    fn parse_git_url() {
        let src = PackageSource::parse("https://github.com/user/repo.git").unwrap();
        assert_eq!(
            src,
            PackageSource::GitUrl("https://github.com/user/repo.git".to_string())
        );
    }

    #[test]
    fn parse_local_source() {
        let src = PackageSource::parse("path:./my-module").unwrap();
        assert_eq!(src, PackageSource::Local(PathBuf::from("./my-module")));
        assert!(src.is_local());
        assert_eq!(src.inferred_name(), "my-module");
    }

    #[test]
    fn parse_local_absolute() {
        let src = PackageSource::parse("path:/home/user/mae-fancy").unwrap();
        assert_eq!(
            src,
            PackageSource::Local(PathBuf::from("/home/user/mae-fancy"))
        );
        assert_eq!(src.inferred_name(), "fancy");
    }

    #[test]
    fn parse_invalid_source() {
        assert!(PackageSource::parse("foobar").is_err());
        assert!(PackageSource::parse("github:").is_err());
        assert!(PackageSource::parse("github:user").is_err());
        assert!(PackageSource::parse("github:/repo").is_err());
        assert!(PackageSource::parse("path:").is_err());
    }

    #[test]
    fn clone_url_github() {
        let src = PackageSource::GitHub {
            user: "user".to_string(),
            repo: "mae-theme".to_string(),
        };
        assert_eq!(src.clone_url(), "https://github.com/user/mae-theme.git");
    }

    #[test]
    fn inferred_name_strips_prefix() {
        let src = PackageSource::GitHub {
            user: "user".to_string(),
            repo: "mae-org-roam".to_string(),
        };
        assert_eq!(src.inferred_name(), "org-roam");

        let src2 = PackageSource::GitHub {
            user: "user".to_string(),
            repo: "my-theme".to_string(),
        };
        assert_eq!(src2.inferred_name(), "my-theme");
    }

    #[test]
    fn inferred_name_from_git_url() {
        let src = PackageSource::GitUrl("https://github.com/user/mae-fancy.git".to_string());
        assert_eq!(src.inferred_name(), "fancy");
    }
}
