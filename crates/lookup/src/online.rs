//! Online documentation URL builders.

/// Build a documentation URL for a symbol in a given language.
///
/// Returns None if we don't know where to look for that language.
pub fn docs_url(word: &str, language: &str) -> Option<String> {
    let encoded = url_encode(word);
    match language {
        "rust" => Some(format!("https://docs.rs/releases/search?query={}", encoded)),
        "python" => Some(format!(
            "https://docs.python.org/3/search.html?q={}",
            encoded
        )),
        "javascript" | "typescript" | "javascriptreact" | "typescriptreact" => Some(format!(
            "https://developer.mozilla.org/en-US/search?q={}",
            encoded
        )),
        "go" => Some(format!("https://pkg.go.dev/search?q={}", encoded)),
        "c" | "cpp" => Some(format!(
            "https://en.cppreference.com/mwiki/index.php?search={}",
            encoded
        )),
        "java" => Some(format!("https://docs.oracle.com/search/?q={}", encoded)),
        "ruby" => Some(format!("https://ruby-doc.org/search.html?q={}", encoded)),
        _ => {
            // Fallback to DevDocs
            Some(format!("https://devdocs.io/#q={}", encoded))
        }
    }
}

/// Build a DevDocs URL for a specific language scope.
pub fn devdocs_url(word: &str, language: &str) -> String {
    let scope = match language {
        "javascript" | "typescript" => "javascript",
        "cpp" => "cpp",
        "c" => "c",
        "python" => "python~3",
        "go" => "go",
        "rust" => "rust",
        "ruby" => "ruby~3",
        _ => language,
    };
    format!("https://devdocs.io/#q={} {}", scope, url_encode(word))
}

fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(ch),
            ' ' => result.push('+'),
            _ => {
                for byte in ch.to_string().as_bytes() {
                    result.push('%');
                    result.push_str(&format!("{:02X}", byte));
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_docs_url() {
        let url = docs_url("HashMap", "rust").unwrap();
        assert!(url.contains("docs.rs"));
        assert!(url.contains("HashMap"));
    }

    #[test]
    fn python_docs_url() {
        let url = docs_url("asyncio", "python").unwrap();
        assert!(url.contains("docs.python.org"));
    }

    #[test]
    fn js_docs_url() {
        let url = docs_url("Promise", "javascript").unwrap();
        assert!(url.contains("developer.mozilla.org"));
    }

    #[test]
    fn go_docs_url() {
        let url = docs_url("context", "go").unwrap();
        assert!(url.contains("pkg.go.dev"));
    }

    #[test]
    fn cpp_docs_url() {
        let url = docs_url("vector", "cpp").unwrap();
        assert!(url.contains("cppreference"));
    }

    #[test]
    fn unknown_language_devdocs() {
        let url = docs_url("something", "haskell").unwrap();
        assert!(url.contains("devdocs.io"));
    }

    #[test]
    fn devdocs_scoped() {
        let url = devdocs_url("fetch", "javascript");
        assert!(url.contains("devdocs.io"));
        assert!(url.contains("javascript"));
    }

    #[test]
    fn url_encode_special_chars() {
        let encoded = url_encode("std::vector<int>");
        assert!(encoded.contains("std"));
        assert!(!encoded.contains('<'));
    }
}
