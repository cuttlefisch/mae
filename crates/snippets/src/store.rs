//! Snippet store — load and look up snippet definitions by language and trigger.

use std::collections::HashMap;
use std::path::Path;

/// A snippet definition loaded from a file or registered programmatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetDef {
    /// Human-readable name.
    pub name: String,
    /// Trigger prefix (typed text that expands into the snippet).
    pub trigger: String,
    /// Snippet body (VSCode/LSP snippet syntax).
    pub body: String,
    /// Optional description.
    pub description: Option<String>,
}

/// Collection of snippets organized by language.
#[derive(Debug, Clone, Default)]
pub struct SnippetStore {
    /// Map from language identifier to list of snippets.
    snippets: HashMap<String, Vec<SnippetDef>>,
}

impl SnippetStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load snippets from a directory structure: `base_dir/<language>/*.snippet`
    ///
    /// Each `.snippet` file uses a simple format:
    /// ```text
    /// # name: Function definition
    /// # trigger: fn
    /// # description: Create a new function
    /// # --
    /// fn ${1:name}(${2:params}) -> ${3:()} {
    ///     $0
    /// }
    /// ```
    pub fn load_dir(base_dir: &Path) -> Self {
        let mut store = Self::new();
        let Ok(entries) = std::fs::read_dir(base_dir) else {
            return store;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(lang) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let lang = lang.to_string();
            let Ok(files) = std::fs::read_dir(&path) else {
                continue;
            };
            for file in files.flatten() {
                let fpath = file.path();
                if fpath.extension().and_then(|e| e.to_str()) != Some("snippet") {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&fpath) {
                    if let Some(def) = parse_snippet_file(&content) {
                        store.snippets.entry(lang.clone()).or_default().push(def);
                    }
                }
            }
        }
        store
    }

    /// Register a snippet programmatically.
    pub fn add(&mut self, language: &str, def: SnippetDef) {
        self.snippets
            .entry(language.to_string())
            .or_default()
            .push(def);
    }

    /// Look up snippets matching a trigger prefix for a language.
    pub fn lookup(&self, language: &str, prefix: &str) -> Vec<&SnippetDef> {
        let Some(defs) = self.snippets.get(language) else {
            return Vec::new();
        };
        defs.iter()
            .filter(|d| d.trigger.starts_with(prefix))
            .collect()
    }

    /// Get all snippets for a language.
    pub fn for_language(&self, language: &str) -> &[SnippetDef] {
        self.snippets
            .get(language)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Get all registered languages.
    pub fn languages(&self) -> Vec<&str> {
        self.snippets.keys().map(|s| s.as_str()).collect()
    }

    /// Total number of snippets across all languages.
    pub fn len(&self) -> usize {
        self.snippets.values().map(|v| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.snippets.values().all(|v| v.is_empty())
    }
}

/// Parse a `.snippet` file with header metadata and body.
fn parse_snippet_file(content: &str) -> Option<SnippetDef> {
    let mut name = None;
    let mut trigger = None;
    let mut description = None;
    let mut body_start = 0;
    let mut found_separator = false;

    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed == "# --" {
            found_separator = true;
            body_start = content
                .lines()
                .take(i + 1)
                .map(|l| l.len() + 1)
                .sum::<usize>();
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("# name:") {
            name = Some(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("# trigger:") {
            trigger = Some(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("# description:") {
            description = Some(rest.trim().to_string());
        }
    }

    if !found_separator {
        return None;
    }

    let trigger = trigger?;
    let name = name.unwrap_or_else(|| trigger.clone());
    let body = content[body_start..].to_string();

    Some(SnippetDef {
        name,
        trigger,
        body,
        description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_snippet_file_basic() {
        let content = "# name: Function\n# trigger: fn\n# --\nfn ${1:name}() {\n\t$0\n}\n";
        let def = parse_snippet_file(content).unwrap();
        assert_eq!(def.name, "Function");
        assert_eq!(def.trigger, "fn");
        assert!(def.body.starts_with("fn ${1:name}()"));
    }

    #[test]
    fn parse_snippet_file_minimal() {
        let content = "# trigger: if\n# --\nif $1 {\n\t$0\n}\n";
        let def = parse_snippet_file(content).unwrap();
        assert_eq!(def.name, "if"); // falls back to trigger
        assert_eq!(def.trigger, "if");
    }

    #[test]
    fn parse_snippet_file_no_separator() {
        let content = "# trigger: fn\nfn foo() {}";
        assert!(parse_snippet_file(content).is_none());
    }

    #[test]
    fn parse_snippet_file_no_trigger() {
        let content = "# name: Something\n# --\nfoo";
        assert!(parse_snippet_file(content).is_none());
    }

    #[test]
    fn store_add_and_lookup() {
        let mut store = SnippetStore::new();
        store.add(
            "rust",
            SnippetDef {
                name: "Function".into(),
                trigger: "fn".into(),
                body: "fn ${1:name}() {}".into(),
                description: None,
            },
        );
        store.add(
            "rust",
            SnippetDef {
                name: "For loop".into(),
                trigger: "for".into(),
                body: "for ${1:item} in ${2:iter} {}".into(),
                description: None,
            },
        );

        let matches = store.lookup("rust", "fn");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].trigger, "fn");

        let matches = store.lookup("rust", "f");
        assert_eq!(matches.len(), 2); // both "fn" and "for" start with "f"

        let matches = store.lookup("python", "fn");
        assert!(matches.is_empty());
    }

    #[test]
    fn store_for_language() {
        let mut store = SnippetStore::new();
        store.add(
            "go",
            SnippetDef {
                name: "Main".into(),
                trigger: "main".into(),
                body: "func main() {\n\t$0\n}".into(),
                description: None,
            },
        );
        assert_eq!(store.for_language("go").len(), 1);
        assert_eq!(store.for_language("rust").len(), 0);
    }

    #[test]
    fn store_len() {
        let mut store = SnippetStore::new();
        assert!(store.is_empty());
        store.add(
            "rust",
            SnippetDef {
                name: "a".into(),
                trigger: "a".into(),
                body: "$0".into(),
                description: None,
            },
        );
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
    }
}
