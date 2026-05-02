//! Language detection: shebang, modeline, file extension, and composite detection.

use super::Language;

/// Detect the language for a file based on its extension.
pub fn language_for_path(path: &std::path::Path) -> Option<Language> {
    // Filename-first match for files without extensions (Makefile, Dockerfile...).
    if matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some(".bashrc" | ".bash_profile" | ".profile" | ".zshrc")
    ) {
        return Some(Language::Bash);
    }
    let ext = path.extension()?.to_str()?;
    Some(match ext {
        "rs" => Language::Rust,
        "toml" => Language::Toml,
        "md" | "markdown" => Language::Markdown,
        "py" | "pyi" | "pyw" => Language::Python,
        "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
        "ts" => Language::TypeScript,
        "tsx" => Language::Tsx,
        "go" => Language::Go,
        "json" | "jsonc" => Language::Json,
        "sh" | "bash" | "zsh" | "ksh" => Language::Bash,
        "scm" | "ss" | "sld" | "sls" => Language::Scheme,
        "yaml" | "yml" => Language::Yaml,
        "org" => Language::Org,
        _ => return None,
    })
}

/// Parse a language ID string (e.g. "rust", "python") into a `Language`.
pub fn language_from_id(id: &str) -> Option<Language> {
    Some(match id.to_lowercase().as_str() {
        "rust" => Language::Rust,
        "toml" => Language::Toml,
        "markdown" | "md" => Language::Markdown,
        "python" | "py" => Language::Python,
        "javascript" | "js" => Language::JavaScript,
        "typescript" | "ts" => Language::TypeScript,
        "tsx" => Language::Tsx,
        "go" | "golang" => Language::Go,
        "json" | "jsonc" => Language::Json,
        "bash" | "sh" | "shell" | "zsh" => Language::Bash,
        "scheme" | "scm" => Language::Scheme,
        "yaml" | "yml" => Language::Yaml,
        "org" => Language::Org,
        _ => return None,
    })
}

/// Detect language from a shebang line (e.g. `#!/usr/bin/env python3`).
pub fn language_from_shebang(first_line: &str) -> Option<Language> {
    let line = first_line.trim();
    if !line.starts_with("#!") {
        return None;
    }
    // Extract the binary name: last path component, with `env [-S]` handling.
    let after_hash_bang = line[2..].trim();
    let binary = if after_hash_bang.contains("/env") {
        // `#!/usr/bin/env python3` or `#!/usr/bin/env -S node`
        after_hash_bang
            .split_whitespace()
            .find(|s| !s.starts_with('/') && !s.starts_with('-'))?
    } else {
        // `#!/bin/bash` -- take the last path component
        after_hash_bang
            .split('/')
            .next_back()?
            .split_whitespace()
            .next()?
    };
    // Strip version suffix: python3.12 -> python, node18 -> node
    let base = binary.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    let base = if base.is_empty() { binary } else { base };
    match base {
        "python" | "pypy" => Some(Language::Python),
        "bash" | "sh" | "zsh" | "ksh" => Some(Language::Bash),
        "node" | "deno" | "bun" => Some(Language::JavaScript),
        "ts-node" | "tsx" => Some(Language::TypeScript),
        _ => None,
    }
}

/// Detect language from a modeline comment in the first or last 5 lines.
/// Pattern: `mae: language=<id>` (case-insensitive on the id).
pub fn language_from_modeline(content: &str) -> Option<Language> {
    use regex::Regex;
    use std::sync::OnceLock;
    static MODELINE: OnceLock<Regex> = OnceLock::new();
    let re = MODELINE.get_or_init(|| Regex::new(r"mae:\s*language=(\w+)").unwrap());

    let lines: Vec<&str> = content.lines().collect();
    let first5 = &lines[..lines.len().min(5)];
    let last5_start = lines.len().saturating_sub(5);
    let last5 = &lines[last5_start..];

    for line in first5.iter().chain(last5.iter()) {
        if let Some(cap) = re.captures(line) {
            if let Some(m) = cap.get(1) {
                return language_from_id(m.as_str());
            }
        }
    }
    None
}

/// Detect language for a buffer using priority: shebang > modeline > extension.
/// This is the primary entry point for language detection on file open.
pub fn language_for_buffer(path: &std::path::Path, content: &str) -> Option<Language> {
    // 1. Shebang (first line)
    if let Some(first_line) = content.lines().next() {
        if let Some(lang) = language_from_shebang(first_line) {
            return Some(lang);
        }
    }
    // 2. Modeline
    if let Some(lang) = language_from_modeline(content) {
        return Some(lang);
    }
    // 3. File extension
    language_for_path(path)
}
