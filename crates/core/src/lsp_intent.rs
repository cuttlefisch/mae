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
    /// Request `workspace/symbol`.
    WorkspaceSymbol { language_id: String, query: String },
    /// Request `textDocument/documentSymbol`.
    DocumentSymbols { uri: String, language_id: String },
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

/// Map a file extension (or filename) to an LSP language id.
/// Mirrors `mae-lsp`'s helper for a consistent set of languages.
pub fn language_id_from_path(path: &std::path::Path) -> Option<String> {
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
        "json" => "json",
        "md" => "markdown",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "html" | "htm" => "html",
        "css" => "css",
        _ => return None,
    };
    Some(id.to_string())
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
}
