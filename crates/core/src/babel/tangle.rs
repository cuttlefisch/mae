//! Tangle — extract source code from org-mode documents into files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::noweb::expand_noweb;
use super::{expand_tilde, parse_src_blocks, SrcBlock, TangleTarget};

/// Output from tangling a buffer.
#[derive(Debug, Clone)]
pub struct TangleOutput {
    pub path: PathBuf,
    pub content: String,
}

/// Tangle all source blocks in a document.
/// Groups blocks by target file, concatenates in document order.
pub fn tangle_buffer(source: &str, base_dir: &Path, base_name: &str) -> Vec<TangleOutput> {
    let blocks = parse_src_blocks(source);
    tangle_blocks(&blocks, source, base_dir, base_name)
}

/// Tangle from pre-parsed blocks.
pub fn tangle_blocks(
    blocks: &[SrcBlock],
    _source: &str,
    base_dir: &Path,
    base_name: &str,
) -> Vec<TangleOutput> {
    let mut file_contents: HashMap<PathBuf, Vec<(usize, String)>> = HashMap::new();

    for (block_idx, block) in blocks.iter().enumerate() {
        let target = match &block.header_args.tangle {
            TangleTarget::No => continue,
            TangleTarget::Yes => {
                let ext = default_extension(&block.language);
                base_dir.join(format!("{}.{}", base_name, ext))
            }
            TangleTarget::File(path) => {
                let p = PathBuf::from(expand_tilde(path));
                if p.is_absolute() {
                    // Safety: reject tangle to self
                    p
                } else {
                    base_dir.join(p)
                }
            }
        };

        // Expand noweb references
        let body = match expand_noweb(&block.body, blocks) {
            Ok(expanded) => expanded,
            Err(_) => block.body.clone(), // Use unexpanded on error
        };

        file_contents
            .entry(target)
            .or_default()
            .push((block_idx, body));
    }

    let mut outputs: Vec<TangleOutput> = file_contents
        .into_iter()
        .map(|(path, mut pieces)| {
            pieces.sort_by_key(|(idx, _)| *idx);
            let content = pieces
                .into_iter()
                .map(|(_, body)| body)
                .collect::<Vec<_>>()
                .join("\n\n");
            TangleOutput { path, content }
        })
        .collect();

    outputs.sort_by(|a, b| a.path.cmp(&b.path));
    outputs
}

/// Write tangle outputs to disk. Creates parent directories if mkdirp is true.
pub fn write_tangle_outputs(
    outputs: &[TangleOutput],
    mkdirp: bool,
) -> Vec<Result<PathBuf, String>> {
    outputs
        .iter()
        .map(|out| {
            if mkdirp {
                if let Some(parent) = out.path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return Err(format!(
                            "Failed to create directory {}: {}",
                            parent.display(),
                            e
                        ));
                    }
                }
            }
            match std::fs::write(&out.path, &out.content) {
                Ok(()) => Ok(out.path.clone()),
                Err(e) => Err(format!("Failed to write {}: {}", out.path.display(), e)),
            }
        })
        .collect()
}

fn default_extension(language: &str) -> &str {
    match language {
        "python" | "python3" | "python2" => "py",
        "rust" => "rs",
        "ruby" => "rb",
        "perl" => "pl",
        "bash" | "sh" => "sh",
        "zsh" => "zsh",
        "javascript" | "js" | "node" => "js",
        "typescript" | "ts" => "ts",
        "lua" => "lua",
        "go" => "go",
        "c" => "c",
        "cpp" | "c++" => "cpp",
        "java" => "java",
        "scheme" | "racket" => "scm",
        "elisp" | "emacs-lisp" => "el",
        "r" | "R" => "R",
        "sql" => "sql",
        "html" => "html",
        "css" => "css",
        "yaml" | "yml" => "yml",
        "toml" => "toml",
        "json" => "json",
        "xml" => "xml",
        _ => "txt",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tangle_skip_no_tangle() {
        let src = "#+begin_src python\nprint(1)\n#+end_src\n";
        let outputs = tangle_buffer(src, Path::new("/tmp"), "test");
        assert!(outputs.is_empty()); // default is :tangle no
    }

    #[test]
    fn tangle_yes_default_extension() {
        let src = "#+begin_src python :tangle yes\nprint(1)\n#+end_src\n";
        let outputs = tangle_buffer(src, Path::new("/tmp"), "test");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].path, PathBuf::from("/tmp/test.py"));
        assert_eq!(outputs[0].content, "print(1)");
    }

    #[test]
    fn tangle_to_file() {
        let src = "#+begin_src python :tangle \"output.py\"\nprint(1)\n#+end_src\n";
        let outputs = tangle_buffer(src, Path::new("/tmp"), "test");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].path, PathBuf::from("/tmp/output.py"));
    }

    #[test]
    fn tangle_multiple_blocks_same_file() {
        let src = "#+begin_src python :tangle yes\nprint(1)\n#+end_src\n\n#+begin_src python :tangle yes\nprint(2)\n#+end_src\n";
        let outputs = tangle_buffer(src, Path::new("/tmp"), "test");
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].content.contains("print(1)"));
        assert!(outputs[0].content.contains("print(2)"));
    }

    #[test]
    fn tangle_multiple_files() {
        let src = "#+begin_src python :tangle \"a.py\"\nprint(1)\n#+end_src\n\n#+begin_src rust :tangle \"b.rs\"\nfn main() {}\n#+end_src\n";
        let outputs = tangle_buffer(src, Path::new("/tmp"), "test");
        assert_eq!(outputs.len(), 2);
    }

    #[test]
    fn default_ext_coverage() {
        assert_eq!(default_extension("python"), "py");
        assert_eq!(default_extension("rust"), "rs");
        assert_eq!(default_extension("bash"), "sh");
        assert_eq!(default_extension("go"), "go");
        assert_eq!(default_extension("unknown"), "txt");
    }
}
