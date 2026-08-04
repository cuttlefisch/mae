//! Shared `web_fetch` policy — the rules about what may be fetched and what
//! comes back, kept in one place so the two transports cannot drift.
//!
//! There are two transports because there have to be:
//! - `AgentSession::execute_web_fetch` (`session/run_loop.rs`) runs on the
//!   agent's tokio task and is `async`;
//! - `executor::session_exec::execute_web_fetch` runs synchronously on the
//!   main thread, where `dispatch_tool` holds a `!Send` `&mut Editor` and
//!   cannot await (ADR-091).
//!
//! What must NOT be duplicated is the *policy*: which URL schemes are
//! accepted, whether HTML is stripped, and how much body is returned. A
//! second copy of that is a second thing to keep in sync, and the copy that
//! drifts is the one that accepts more (principle #8). Both transports call
//! [`validate_url`] and [`shape_body`].
//!
//! @stability: experimental

/// Request timeout, seconds. Shared so a fetch cannot hang one surface
/// longer than the other.
pub const TIMEOUT_SECS: u64 = 30;

/// User-agent sent by both transports.
pub const USER_AGENT: &str = "MAE";

/// Maximum body returned to the model, bytes. Beyond this the text is cut on
/// a character boundary and marked as truncated.
pub const MAX_BODY_BYTES: usize = 32_768;

/// Scheme allow-list. `file://` and friends are rejected here rather than
/// left to the HTTP client: `web_fetch` is a *web* tool, and a permissive
/// scheme check is a local-file read wearing a URL.
pub fn validate_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("Missing 'url' argument".into());
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!(
            "Invalid URL scheme: only http:// and https:// are supported, got: {url}"
        ));
    }
    Ok(())
}

/// Turn a fetched response into the text handed to the model: strip HTML when
/// the content type says HTML, truncate to [`MAX_BODY_BYTES`] on a character
/// boundary, and prefix the status line.
pub fn shape_body(status: u16, content_type: &str, body: String) -> String {
    let text = if content_type.contains("html") {
        strip_html(&body)
    } else {
        body
    };
    let text = if text.len() > MAX_BODY_BYTES {
        // ADR-087 class: a fixed byte cut can land mid-character and panic.
        let boundary = mae_core::grapheme::floor_char_boundary(&text, MAX_BODY_BYTES);
        format!("{}...\n[truncated at 32KB]", &text[..boundary])
    } else {
        text
    };
    format!("HTTP {status} ({content_type})\n\n{text}")
}

/// Strip tags, script/style contents, and the common HTML entities from
/// `html`, then collapse runs of blank lines.
pub fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut chars = html.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '<' {
            // Check for script/style open/close tags
            let rest: String = chars.clone().take(20).collect();
            let rest_lower = rest.to_ascii_lowercase();
            if rest_lower.starts_with("script") {
                in_script = true;
            } else if rest_lower.starts_with("/script") {
                in_script = false;
            } else if rest_lower.starts_with("style") {
                in_style = true;
            } else if rest_lower.starts_with("/style") {
                in_style = false;
            }
            in_tag = true;
            continue;
        }
        if ch == '>' {
            in_tag = false;
            continue;
        }
        if in_tag || in_script || in_style {
            continue;
        }
        // Decode HTML entities
        if ch == '&' {
            let entity: String = chars
                .clone()
                .take_while(|c| *c != ';' && *c != ' ' && *c != '<')
                .collect();
            if entity.len() < 10 {
                let decoded = match entity.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "nbsp" => Some(' '),
                    "#39" | "apos" => Some('\''),
                    _ => None,
                };
                if let Some(decoded_char) = decoded {
                    result.push(decoded_char);
                    // Advance past entity + semicolon
                    for _ in 0..entity.len() {
                        chars.next();
                    }
                    if chars.peek() == Some(&';') {
                        chars.next();
                    }
                    continue;
                }
            }
            result.push('&');
            continue;
        }
        result.push(ch);
    }

    // Collapse excessive whitespace
    let mut collapsed = String::with_capacity(result.len());
    let mut blank_lines = 0;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_lines += 1;
            if blank_lines <= 1 {
                collapsed.push('\n');
            }
        } else {
            blank_lines = 0;
            collapsed.push_str(trimmed);
            collapsed.push('\n');
        }
    }

    collapsed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The allow-list is the security-relevant half of this module: a
    /// permissive scheme check turns a web tool into a local-file read.
    /// Attacker-first — the rejections are the point, the two acceptances
    /// exist to prove the check is not simply refusing everything.
    #[test]
    fn only_http_and_https_are_accepted() {
        for bad in [
            "",
            "file:///etc/passwd",
            "ftp://example.com/x",
            "data:text/html,<script>",
            "javascript:alert(1)",
            "HTTP://example.com",
            " http://example.com",
            "//example.com",
            "example.com",
            "mae://join/deadbeef",
        ] {
            assert!(validate_url(bad).is_err(), "validate_url accepted {bad:?}");
        }
        for good in ["http://example.com", "https://example.com/a?b=c#d"] {
            assert!(validate_url(good).is_ok(), "validate_url rejected {good:?}");
        }
    }

    /// Truncation must not panic on a multi-byte boundary — the ADR-087
    /// defect class. Exercised at every offset around the cut, with a
    /// character whose UTF-8 encoding straddles it, rather than one
    /// hand-picked string that happens to align.
    #[test]
    fn truncation_never_splits_a_character() {
        for pad in 0..8usize {
            let body = format!("{}{}", "a".repeat(MAX_BODY_BYTES - pad), "é".repeat(64));
            let shaped = shape_body(200, "text/plain", body);
            assert!(shaped.contains("[truncated at 32KB]"), "pad={pad}");
            // The mere fact this is a valid `String` after slicing is the
            // assertion; a bad cut would have panicked above.
            assert!(shaped.is_char_boundary(shaped.len()));
        }
    }

    #[test]
    fn short_bodies_are_returned_whole_and_unstripped() {
        let shaped = shape_body(404, "text/plain", "<b>not html</b>".into());
        assert!(shaped.starts_with("HTTP 404 (text/plain)"));
        assert!(
            shaped.contains("<b>not html</b>"),
            "non-HTML content type must not be stripped: {shaped}"
        );
    }

    #[test]
    fn html_content_type_is_stripped() {
        let shaped = shape_body(
            200,
            "text/html; charset=utf-8",
            "<html><style>x{}</style><script>evil()</script><p>Hello &amp; bye</p></html>".into(),
        );
        assert!(shaped.contains("Hello & bye"), "{shaped}");
        assert!(!shaped.contains("evil()"), "script survived: {shaped}");
        assert!(!shaped.contains("x{}"), "style survived: {shaped}");
    }
}
