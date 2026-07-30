//! Language-server request intents.
//!
//! The editor's synchronous dispatch layer cannot send async LSP requests
//! directly, so commands that require a language server ("go to definition",
//! "find references", "hover", plus the didOpen/didChange/didSave lifecycle)
//! push an `LspIntent` onto the editor's queue. The outer binary drains the
//! queue each event-loop iteration and forwards each intent to the
//! `run_lsp_task`.
//!
//! Keeping this type in `mae-core` avoids a circular dependency: `mae-lsp`
//! depends on nothing from core, and `mae-core` exposes only the simple
//! data required to describe a request.

/// A language-server request or notification pending dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspIntent {
    /// Notify the server a document was opened.
    DidOpen {
        uri: String,
        language_id: String,
        text: String,
    },
    /// Notify the server a document changed (full-text sync).
    DidChange {
        uri: String,
        language_id: String,
        text: String,
    },
    /// Notify the server a document was saved.
    DidSave {
        uri: String,
        language_id: String,
        text: Option<String>,
    },
    /// Notify the server a document was closed.
    DidClose { uri: String, language_id: String },
    /// Request `textDocument/definition`.
    GotoDefinition {
        uri: String,
        language_id: String,
        line: u32,
        character: u32,
    },
    /// Request `textDocument/references`.
    FindReferences {
        uri: String,
        language_id: String,
        line: u32,
        character: u32,
        include_declaration: bool,
    },
    /// Request `textDocument/hover`.
    Hover {
        uri: String,
        language_id: String,
        line: u32,
        character: u32,
    },
    /// Request `textDocument/completion`.
    Completion {
        uri: String,
        language_id: String,
        line: u32,
        character: u32,
    },
    /// Request `textDocument/codeAction`.
    CodeAction {
        uri: String,
        language_id: String,
        line: u32,
        character: u32,
    },
    /// Request `textDocument/prepareRename` to validate the position is renameable.
    PrepareRename {
        uri: String,
        language_id: String,
        line: u32,
        character: u32,
    },
    /// Request `textDocument/rename`.
    Rename {
        uri: String,
        language_id: String,
        line: u32,
        character: u32,
        new_name: String,
    },
    /// Request `textDocument/formatting`.
    Format { uri: String, language_id: String },
    /// Request `textDocument/rangeFormatting`.
    RangeFormat {
        uri: String,
        language_id: String,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
    },
    /// Request `workspace/symbol`.
    WorkspaceSymbol { language_id: String, query: String },
    /// Request `textDocument/documentSymbol`.
    DocumentSymbols { uri: String, language_id: String },
    /// Request `textDocument/documentHighlight`.
    DocumentHighlight {
        uri: String,
        language_id: String,
        line: u32,
        character: u32,
        generation: u64,
    },
    /// Request `textDocument/signatureHelp`.
    SignatureHelp {
        uri: String,
        language_id: String,
        line: u32,
        character: u32,
    },
}

/// Convert a filesystem path to a `file://` URI, matching `mae-lsp`'s
/// `path_to_uri` helper. Duplicated here so the core crate has no dependency
/// on the LSP crate.
pub fn path_to_uri(path: &std::path::Path) -> String {
    let p = path.to_string_lossy();
    if p.starts_with("file://") {
        p.into_owned()
    } else if p.starts_with('/') {
        format!("file://{}", p)
    } else {
        // Relative — resolve against cwd for a stable absolute URI.
        match std::env::current_dir() {
            Ok(cwd) => format!("file://{}/{}", cwd.display(), p),
            Err(_) => format!("file://{}", p),
        }
    }
}

/// Map a file path to an LSP language id — the SOLE authority for LSP
/// `language_id` routing in MAE (ADR-075). Every `LspIntent`/`did_open` call
/// site derives its `language_id` from this function (directly or via a
/// thin wrapper), and `LspManager` routes purely on that string
/// (`HashMap<String, LspServerConfig>` keyed by language id) — completely
/// decoupled from `crate::syntax::Language`/tree-sitter grammar selection.
/// This is intentional, not an accident: a "dialect" of an existing
/// tree-sitter language (e.g. an Ansible playbook, still plain
/// `Language::Yaml` for highlighting) can route to a different LSP server
/// by returning a different string here, without touching tree-sitter or
/// any of this function's ~18 call sites.
///
/// **Single choke point**: any future dialect override (path-heuristic or
/// otherwise) belongs INSIDE this function, never duplicated at a call
/// site — every consumer inherits it for free. `should_auto_complete`
/// (`editor/lsp_ops.rs`) calls this on every keystroke in insert mode, so
/// anything added here must stay pure lexical path-string inspection
/// (`Path::file_name()`/`Path::extension()`/`Path::components()`) — no
/// filesystem I/O (`.exists()`, directory walks), which would make every
/// keystroke pay a syscall.
pub fn language_id_from_path(path: &std::path::Path) -> Option<String> {
    // Filename-first: `Dockerfile` conventionally carries no extension at
    // all, so the extension-only match below can never see it. Shared with
    // `syntax::detection::language_for_path` (tree-sitter grammar
    // selection) so the two registries can't drift on what counts as a
    // Dockerfile, even though they're intentionally decoupled otherwise.
    if crate::syntax::detection::is_dockerfile_filename(path) {
        return Some("dockerfile".to_string());
    }
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let id = match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cxx" | "cc" | "hpp" | "hxx" => "cpp",
        "java" => "java",
        "rb" => "ruby",
        "scm" | "ss" => "scheme",
        "lua" => "lua",
        "sh" | "bash" => "bash",
        "json" => "json",
        "md" => "markdown",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "html" | "htm" => "html",
        "css" => "css",
        "tf" | "tfvars" | "hcl" => "terraform",
        _ => return None,
    };
    if id == "yaml" {
        if let Some(dialect) = ansible_lsp_dialect(path) {
            return Some(dialect.to_string());
        }
    }
    Some(id.to_string())
}

/// Client-side path heuristic for routing a YAML file to
/// `ansible-language-server` instead of the generic `yaml-language-server`
/// (ADR-075 Phase 4). Replicates `ansible-language-server`'s own detection
/// convention (`vscode-ansible#582` confirms there is no server-side
/// content-sniffing to defer to — filename/path heuristics are the real
/// mechanism upstream uses too): a `site.yml`/`site.yaml` filename, a
/// filename containing "playbook", an ancestor path component that is
/// EXACTLY `playbooks` (not merely a substring — `playbooks-archive/` must
/// not match), or the double-extension `.ansible.yml`/`.ansible.yaml`
/// convention.
///
/// Pure lexical path inspection only — no filesystem I/O — matching
/// `language_id_from_path`'s own hot-path constraint (this function is
/// reached on every keystroke via `should_auto_complete`).
fn ansible_lsp_dialect(path: &std::path::Path) -> Option<&'static str> {
    let file_name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if file_name == "site.yml" || file_name == "site.yaml" {
        return Some("ansible");
    }
    if file_name.contains("playbook") {
        return Some("ansible");
    }
    if file_name.ends_with(".ansible.yml") || file_name.ends_with(".ansible.yaml") {
        return Some("ansible");
    }
    let has_playbooks_ancestor = path
        .components()
        .any(|c| c.as_os_str().eq_ignore_ascii_case("playbooks"));
    if has_playbooks_ancestor {
        return Some("ansible");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn path_to_uri_absolute_path() {
        let p = PathBuf::from("/tmp/foo.rs");
        assert_eq!(path_to_uri(&p), "file:///tmp/foo.rs");
    }

    #[test]
    fn path_to_uri_idempotent_for_uri() {
        let p = PathBuf::from("file:///tmp/foo.rs");
        assert_eq!(path_to_uri(&p), "file:///tmp/foo.rs");
    }

    #[test]
    fn language_id_rust() {
        let p = PathBuf::from("/tmp/main.rs");
        assert_eq!(language_id_from_path(&p).as_deref(), Some("rust"));
    }

    #[test]
    fn language_id_python() {
        let p = PathBuf::from("test.py");
        assert_eq!(language_id_from_path(&p).as_deref(), Some("python"));
    }

    #[test]
    fn language_id_unknown() {
        let p = PathBuf::from("file.xyz");
        assert_eq!(language_id_from_path(&p), None);
    }

    #[test]
    fn language_id_scheme() {
        let p = PathBuf::from("init.scm");
        assert_eq!(language_id_from_path(&p).as_deref(), Some("scheme"));
    }

    #[test]
    fn language_id_terraform() {
        for name in ["main.tf", "terraform.tfvars", "network.hcl"] {
            assert_eq!(
                language_id_from_path(&PathBuf::from(name)).as_deref(),
                Some("terraform"),
                "{name} should resolve to terraform"
            );
        }
    }

    /// Dockerfile has no extension at all, the exact case the shared
    /// `is_dockerfile_filename` helper (also used by
    /// `syntax::detection::language_for_path`) exists to catch.
    #[test]
    fn language_id_dockerfile() {
        for name in ["Dockerfile", "Dockerfile.prod", "app.dockerfile"] {
            assert_eq!(
                language_id_from_path(&PathBuf::from(name)).as_deref(),
                Some("dockerfile"),
                "{name} should resolve to dockerfile"
            );
        }
    }

    /// Adversarial (principle #14): a plain file merely mentioning "docker"
    /// in its name must NOT false-positive as a Dockerfile.
    #[test]
    fn language_id_docker_compose_is_not_dockerfile() {
        let p = PathBuf::from("docker-compose.yml");
        assert_eq!(language_id_from_path(&p).as_deref(), Some("yaml"));
    }

    #[test]
    fn language_id_ansible_dialect_positive_cases() {
        let cases = [
            "playbooks/site.yml",
            "site.yaml",
            "deploy-playbook.yml",
            "roles/webserver/tasks/playbook_main.yaml",
            "config.ansible.yml",
            "/home/user/project/playbooks/deploy.yaml",
        ];
        for path in cases {
            assert_eq!(
                language_id_from_path(&PathBuf::from(path)).as_deref(),
                Some("ansible"),
                "{path} should resolve to ansible"
            );
        }
    }

    /// Adversarial (principle #14): plain YAML files, including ones that
    /// superficially resemble the positive cases, must NOT false-positive.
    /// `playbooks-archive/` is the critical case -- an ancestor-component
    /// check (not substring) must not match a directory that merely
    /// CONTAINS "playbooks" as a prefix.
    #[test]
    fn language_id_ansible_dialect_negative_cases() {
        let cases = [
            "values.yaml",
            "k8s/deployment.yaml",
            "docker-compose.yml",
            "playbooks-archive/old.yaml",
            ".github/workflows/ci.yml",
        ];
        for path in cases {
            assert_eq!(
                language_id_from_path(&PathBuf::from(path)).as_deref(),
                Some("yaml"),
                "{path} should resolve to plain yaml, not ansible"
            );
        }
    }
}
