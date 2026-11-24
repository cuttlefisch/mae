//! The grammars, and the names each answers to.

/// A tree-sitter grammar MAE can highlight with.
///
/// `#[non_exhaustive]`: which grammars exist depends on this crate's
/// features, so no other crate may match on the whole set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Grammar {
    Rust,
    Toml,
    Markdown,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    Json,
    Bash,
    Scheme,
    Yaml,
    C,
    Cpp,
    Ruby,
    /// HCL, under the name the editor has always shown for it.
    Terraform,
    Dockerfile,
    #[cfg(feature = "site-grammars")]
    PowerShell,
    #[cfg(feature = "site-grammars")]
    Ini,
    #[cfg(feature = "site-grammars")]
    Sql,
    #[cfg(feature = "site-grammars")]
    Nginx,
}

impl Grammar {
    /// Every grammar this build carries.
    pub const ALL: &'static [Grammar] = &[
        Grammar::Rust,
        Grammar::Toml,
        Grammar::Markdown,
        Grammar::Python,
        Grammar::JavaScript,
        Grammar::TypeScript,
        Grammar::Tsx,
        Grammar::Go,
        Grammar::Json,
        Grammar::Bash,
        Grammar::Scheme,
        Grammar::Yaml,
        Grammar::C,
        Grammar::Cpp,
        Grammar::Ruby,
        Grammar::Terraform,
        Grammar::Dockerfile,
        #[cfg(feature = "site-grammars")]
        Grammar::PowerShell,
        #[cfg(feature = "site-grammars")]
        Grammar::Ini,
        #[cfg(feature = "site-grammars")]
        Grammar::Sql,
        #[cfg(feature = "site-grammars")]
        Grammar::Nginx,
    ];

    /// The grammar's name: what the editor shows, and the language name its
    /// highlight configuration carries.
    pub fn name(self) -> &'static str {
        match self {
            Grammar::Rust => "rust",
            Grammar::Toml => "toml",
            Grammar::Markdown => "markdown",
            Grammar::Python => "python",
            Grammar::JavaScript => "javascript",
            Grammar::TypeScript => "typescript",
            Grammar::Tsx => "tsx",
            Grammar::Go => "go",
            Grammar::Json => "json",
            Grammar::Bash => "bash",
            Grammar::Scheme => "scheme",
            Grammar::Yaml => "yaml",
            Grammar::C => "c",
            Grammar::Cpp => "cpp",
            Grammar::Ruby => "ruby",
            Grammar::Terraform => "terraform",
            Grammar::Dockerfile => "dockerfile",
            #[cfg(feature = "site-grammars")]
            Grammar::PowerShell => "powershell",
            #[cfg(feature = "site-grammars")]
            Grammar::Ini => "ini",
            #[cfg(feature = "site-grammars")]
            Grammar::Sql => "sql",
            #[cfg(feature = "site-grammars")]
            Grammar::Nginx => "nginx",
        }
    }

    /// The compiled grammar.
    pub fn language(self) -> tree_sitter::Language {
        match self {
            Grammar::Rust => tree_sitter_rust::LANGUAGE.into(),
            Grammar::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Grammar::Markdown => tree_sitter_md::LANGUAGE.into(),
            Grammar::Python => tree_sitter_python::LANGUAGE.into(),
            Grammar::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Grammar::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Grammar::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Grammar::Go => tree_sitter_go::LANGUAGE.into(),
            Grammar::Json => tree_sitter_json::LANGUAGE.into(),
            Grammar::Bash => tree_sitter_bash::LANGUAGE.into(),
            Grammar::Scheme => tree_sitter_scheme::LANGUAGE.into(),
            Grammar::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Grammar::C => tree_sitter_c::LANGUAGE.into(),
            Grammar::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Grammar::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Grammar::Terraform => tree_sitter_hcl::LANGUAGE.into(),
            Grammar::Dockerfile => tree_sitter_containerfile::LANGUAGE.into(),
            #[cfg(feature = "site-grammars")]
            Grammar::PowerShell => tree_sitter_powershell::LANGUAGE.into(),
            #[cfg(feature = "site-grammars")]
            Grammar::Ini => tree_sitter_ini::LANGUAGE.into(),
            #[cfg(feature = "site-grammars")]
            Grammar::Sql => tree_sitter_sequel::LANGUAGE.into(),
            #[cfg(feature = "site-grammars")]
            Grammar::Nginx => tree_sitter_nginx::LANGUAGE.into(),
        }
    }

    /// The grammar a language name selects (`rust`, `sh`, `yml`...), as a
    /// source block's `#+begin_src` line, a fenced block or a `mae:
    /// language=` modeline writes it; case is ignored. `None` for a
    /// name no grammar answers to, Org included (the editor highlights Org
    /// without tree-sitter).
    pub fn from_name(name: &str) -> Option<Grammar> {
        Some(match name.to_lowercase().as_str() {
            "rust" => Grammar::Rust,
            "toml" => Grammar::Toml,
            "markdown" | "md" => Grammar::Markdown,
            "python" | "py" => Grammar::Python,
            "javascript" | "js" => Grammar::JavaScript,
            "typescript" | "ts" => Grammar::TypeScript,
            "tsx" => Grammar::Tsx,
            "go" | "golang" => Grammar::Go,
            "json" | "jsonc" => Grammar::Json,
            "bash" | "sh" | "shell" | "zsh" => Grammar::Bash,
            "scheme" | "scm" => Grammar::Scheme,
            "yaml" | "yml" => Grammar::Yaml,
            "c" => Grammar::C,
            "cpp" | "c++" | "cxx" | "cc" => Grammar::Cpp,
            "ruby" | "rb" => Grammar::Ruby,
            "terraform" | "tf" | "hcl" => Grammar::Terraform,
            "dockerfile" => Grammar::Dockerfile,
            #[cfg(feature = "site-grammars")]
            "powershell" | "ps1" | "pwsh" => Grammar::PowerShell,
            #[cfg(feature = "site-grammars")]
            "ini" | "dosini" => Grammar::Ini,
            #[cfg(feature = "site-grammars")]
            "sql" => Grammar::Sql,
            #[cfg(feature = "site-grammars")]
            "nginx" => Grammar::Nginx,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every grammar answers to its own name, so the name the editor shows
    /// for a buffer is one a source block can use.
    #[test]
    fn every_grammar_answers_to_its_own_name() {
        for &g in Grammar::ALL {
            assert_eq!(Grammar::from_name(g.name()), Some(g), "{g:?}");
            assert_eq!(
                Grammar::from_name(&g.name().to_uppercase()),
                Some(g),
                "{g:?}"
            );
        }
        for name in ["org", "emacs-lisp", "", "rustt", " rust"] {
            assert_eq!(Grammar::from_name(name), None, "{name:?}");
        }
    }
}
