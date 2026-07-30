//! Live, network-shareable HTML KB view (ADR-073, Phase E, #547) — a single
//! self-contained HTML/CSS/JS page served by the OAuth HTTPS listener
//! (`daemon/src/oauth.rs`) at `GET /kb/{kb_id}/view`.
//!
//! Deliberately dependency-free of `mae-canvas`/`mae-core`/`mae-gui` (ADR-073
//! D2) — this module only generates text. All data access goes through the
//! EXISTING `kb/query.*` surface (ADR-053, `mae_daemon::kb_query`) unchanged; the
//! page's own client-side JS polls it on an interval using the same bearer
//! token the page itself was fetched with (D3). This is v1's whole "live"
//! story — poll-based, not push (see ADR-074 for the deferred SSE upgrade) —
//! and the page says so explicitly, never implying otherwise (tracker gate
//! G1: no silent capability overstatement).
//!
//! Per explicit product decision (ADR-073 D4), this ships a lighter-polish
//! subset than the native editor's chord diagram: a node list + a content
//! pane, no fuzzy search, no history panel, no hover popovers.

/// Parse `/kb/{kb_id}/view` out of a request path. Returns `None` for
/// anything else, including a `kb_id` containing an embedded `/` (which
/// would otherwise ambiguously match a deeper path) or an empty `kb_id`.
pub fn parse_view_path(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/kb/")?;
    let kb_id = rest.strip_suffix("/view")?;
    if kb_id.is_empty() || kb_id.contains('/') {
        None
    } else {
        Some(kb_id)
    }
}

/// Render the self-contained HTML page for `kb_id`. `token` is embedded as a
/// JSON-escaped JS string literal (never raw-interpolated) so the page's own
/// `fetch()` polling calls can present it as a real `Authorization: Bearer`
/// header — `kb/query.*` itself stays header-only; only THIS route's initial
/// GET accepts the token via query-string fallback (see
/// `oauth::extract_view_bearer_token`'s doc comment for why a plain browser
/// navigation has no other way to present it).
///
/// `kb_id` and `token` are both passed through `serde_json::to_string` before
/// embedding — never manually string-interpolated into the `<script>` body —
/// so neither can break out of its JS string literal regardless of content
/// (defense in depth: `kb_id` reaches here only after `parse_view_path`'s
/// no-`/` check and the caller's own access gate, but this function makes no
/// assumption about that and is safe on its own).
pub fn render_page(kb_id: &str, token: &str) -> String {
    let kb_id_js = js_string_literal(kb_id);
    let token_js = js_string_literal(token);
    let kb_id_html = html_escape(kb_id);

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{kb_id_html} — MAE KB view</title>
<style>
  :root {{ color-scheme: light dark; }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0; display: flex; height: 100vh; font-family: system-ui, sans-serif;
    background: Canvas; color: CanvasText;
  }}
  #sidebar {{
    flex: 0 0 300px; overflow-y: auto; border-right: 1px solid color-mix(in srgb, CanvasText 20%, transparent);
    padding: 0.5rem;
  }}
  #sidebar h2 {{ font-size: 0.85rem; text-transform: uppercase; opacity: 0.6; margin: 0.5rem 0.25rem; }}
  #node-list {{ list-style: none; margin: 0; padding: 0; }}
  #node-list li {{
    padding: 0.4rem 0.5rem; border-radius: 6px; cursor: pointer; font-size: 0.9rem;
  }}
  #node-list li:hover {{ background: color-mix(in srgb, CanvasText 8%, transparent); }}
  #node-list li.selected {{ background: color-mix(in srgb, CanvasText 15%, transparent); font-weight: 600; }}
  #main {{ flex: 1 1 auto; overflow-y: auto; padding: 1.5rem 2rem; }}
  #main h1 {{ margin-top: 0; }}
  #main pre {{ white-space: pre-wrap; word-break: break-word; font-family: inherit; }}
  #status {{ font-size: 0.75rem; opacity: 0.55; padding: 0.5rem; border-top: 1px solid color-mix(in srgb, CanvasText 20%, transparent); }}
  #empty {{ opacity: 0.6; padding: 1rem; }}
</style>
</head>
<body>
  <div id="sidebar">
    <h2>{kb_id_html}</h2>
    <ul id="node-list"></ul>
    <div id="status">Auto-refreshes every {poll_secs}s — a live view, not a push feed (v1 polls; see the MAE roadmap for push).</div>
  </div>
  <div id="main">
    <div id="empty">Select a node from the list on the left.</div>
    <div id="node-content" style="display:none">
      <h1 id="node-title"></h1>
      <pre id="node-body"></pre>
    </div>
  </div>
<script>
(function() {{
  "use strict";
  var KB_ID = {kb_id_js};
  var TOKEN = {token_js};
  var POLL_MS = {poll_ms};
  var selected = null;

  function rpc(method, params) {{
    return fetch("/", {{
      method: "POST",
      headers: {{
        "Content-Type": "application/json",
        "Authorization": "Bearer " + TOKEN
      }},
      body: JSON.stringify({{jsonrpc: "2.0", id: 1, method: method, params: params}})
    }}).then(function(r) {{ return r.json(); }});
  }}

  function renderNodeList(nodes) {{
    var ul = document.getElementById("node-list");
    ul.innerHTML = "";
    nodes.forEach(function(id) {{
      var li = document.createElement("li");
      li.textContent = id;
      li.dataset.nodeId = id;
      if (id === selected) li.className = "selected";
      li.addEventListener("click", function() {{ selectNode(id); }});
      ul.appendChild(li);
    }});
  }}

  function renderNode(node) {{
    document.getElementById("empty").style.display = "none";
    document.getElementById("node-content").style.display = "";
    document.getElementById("node-title").textContent = node.title || node.node_id;
    if (node.encryption === "e2e") {{
      document.getElementById("node-body").textContent =
        "(this KB is end-to-end encrypted — this lightweight view cannot decrypt it; " +
        "use a full MAE client with your member key instead)";
    }} else {{
      document.getElementById("node-body").textContent = node.body || "";
    }}
  }}

  function selectNode(id) {{
    selected = id;
    Array.prototype.forEach.call(document.querySelectorAll("#node-list li"), function(li) {{
      li.className = (li.dataset.nodeId === id) ? "selected" : "";
    }});
    rpc("kb/query.get", {{kb_id: KB_ID, node_id: id}}).then(function(resp) {{
      if (resp.result) renderNode(resp.result);
    }});
  }}

  function refreshGraph() {{
    rpc("kb/query.graph", {{kb_id: KB_ID}}).then(function(resp) {{
      if (resp.result && resp.result.nodes) renderNodeList(resp.result.nodes);
    }});
    if (selected) {{
      rpc("kb/query.get", {{kb_id: KB_ID, node_id: selected}}).then(function(resp) {{
        if (resp.result) renderNode(resp.result);
      }});
    }}
  }}

  refreshGraph();
  setInterval(refreshGraph, POLL_MS);
}})();
</script>
</body>
</html>
"##,
        kb_id_html = kb_id_html,
        kb_id_js = kb_id_js,
        token_js = token_js,
        poll_secs = POLL_INTERVAL_SECS,
        poll_ms = POLL_INTERVAL_SECS * 1000,
    )
}

/// How often the served page's client-side JS re-polls `kb/query.graph`/
/// `.get`. Not yet a `kb_graph_wedge_*`-style `OptionRegistry` knob (this
/// view has no Scheme-facing config surface to register into — it's a
/// daemon-side HTTP route, not an editor option) — a fixed, documented
/// constant rather than a magic number scattered across the template.
const POLL_INTERVAL_SECS: u64 = 5;

/// Encode `s` as a JS string literal SAFE to embed directly inside an HTML
/// `<script>` block. `serde_json::to_string` alone is NOT sufficient here:
/// it escapes `"`/`\`/control characters but deliberately does NOT escape
/// `/` — so a `kb_id` or token containing a literal `</script>` substring
/// would reach the page unescaped and the HTML PARSER (which has no concept
/// of JS string literals) would close the script block early, letting
/// whatever follows execute as markup/script. The standard mitigation:
/// escape every `</` to `<\/` — a no-op inside a JS string literal (the
/// backslash is simply dropped) but breaks the literal `</script` substring
/// match the HTML tokenizer looks for. Caught by
/// `render_page_neutralizes_script_breakout_attempts_in_kb_id` (principle
/// #14) — the first version of this function used bare `serde_json::to_string`
/// and that test failed exactly this way.
fn js_string_literal(s: &str) -> String {
    let json = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
    json.replace("</", "<\\/")
}

/// Escape the 5 characters HTML requires escaping in text/attribute content.
/// Used only for `kb_id` in the page's visible `<title>`/`<h2>` text — the
/// script-embedded copies go through `serde_json::to_string` instead, which
/// has its own, stronger safety property (a valid JS string literal).
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_view_path_matches_the_expected_shape() {
        assert_eq!(parse_view_path("/kb/my-kb/view"), Some("my-kb"));
    }

    #[test]
    fn parse_view_path_rejects_unrelated_paths() {
        assert_eq!(parse_view_path("/"), None);
        assert_eq!(parse_view_path("/kb/my-kb"), None);
        assert_eq!(parse_view_path("/kb/my-kb/view/extra"), None);
        assert_eq!(
            parse_view_path("/.well-known/oauth-protected-resource"),
            None
        );
    }

    #[test]
    fn parse_view_path_rejects_empty_kb_id() {
        assert_eq!(parse_view_path("/kb//view"), None);
    }

    /// Adversarial: a kb_id containing an embedded `/` must not be treated
    /// as this route (it would otherwise ambiguously shadow a deeper path).
    #[test]
    fn parse_view_path_rejects_kb_id_with_embedded_slash() {
        assert_eq!(parse_view_path("/kb/a/b/view"), None);
    }

    #[test]
    fn render_page_embeds_kb_id_and_token_as_safe_js_string_literals() {
        let html = render_page("my-kb", "secret-token-abc");
        assert!(html.contains("var KB_ID = \"my-kb\";"));
        assert!(html.contains("var TOKEN = \"secret-token-abc\";"));
    }

    /// Adversarial (principle #14): a kb_id or token containing characters
    /// that would break out of a naive string interpolation (quotes,
    /// `</script>`) must be neutralized by JSON-string-escaping, never
    /// reach the page as literal breakout-capable text.
    #[test]
    fn render_page_neutralizes_script_breakout_attempts_in_kb_id() {
        let hostile = "\"; alert(1); //</script><script>alert(2)</script>";
        let html = render_page(hostile, "tok");
        assert!(
            !html.contains("<script>alert(2)</script>"),
            "a raw, unescaped </script><script> tag must never appear in the output: {html}"
        );
        // The safely-embeddable form should be present as a quoted JS string
        // literal instead: JSON-escaped by serde_json, THEN every `</`
        // additionally escaped to `<\/` so the substring `</script` can never
        // appear literally (see `js_string_literal`'s doc comment for why
        // `serde_json::to_string` alone is insufficient here).
        let expected_js_literal = js_string_literal(hostile);
        assert!(!expected_js_literal.contains("</"));
        assert!(html.contains(&format!("var KB_ID = {expected_js_literal};")));
    }

    #[test]
    fn render_page_escapes_html_special_characters_in_the_visible_title() {
        let html = render_page("<b>evil</b>", "tok");
        assert!(!html.contains("<title><b>evil</b>"));
        assert!(html.contains("&lt;b&gt;evil&lt;/b&gt;"));
    }

    #[test]
    fn render_page_states_plainly_that_v1_is_poll_based() {
        // Gate G1: no silent capability overstatement -- the page must say
        // it polls, never imply a push/live-streaming guarantee it doesn't
        // provide in v1.
        let html = render_page("kb", "tok");
        assert!(html.to_lowercase().contains("poll"));
    }

    #[test]
    fn render_page_is_deterministic() {
        let a = render_page("kb-1", "tok-1");
        let b = render_page("kb-1", "tok-1");
        assert_eq!(a, b);
    }

    #[test]
    fn html_escape_covers_all_five_reserved_characters() {
        assert_eq!(html_escape("&<>\"'"), "&amp;&lt;&gt;&quot;&#39;");
    }
}
