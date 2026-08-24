//! External URLs in a node body are links in the TEXT, not edges in the GRAPH.
//!
//! `parse_typed_links` reports every link it finds, which is correct for a
//! parser. Turning each into a row in the `links` relation is not: the target is
//! a URL, no node can ever have that id, and `kb_health`'s broken-link query
//! (`not exists[dst]`) therefore reports it — permanently, and once per external
//! link in the corpus. A health report that always shows dozens of broken links
//! stops being read, which costs more than the noise itself.

use mae_kb::org;

/// The filter must be an EXCLUDE list, and this is the test that pins why.
///
/// The obvious implementation — keep only targets matching `KB_NAMESPACES` —
/// silently drops links to org-roam-style bare-UUID ids, which carry no
/// namespace at all. A false negative when rendering costs a plain-text link; a
/// false negative here costs a real graph edge.
#[test]
fn bare_and_namespaced_node_ids_are_never_treated_as_external() {
    for id in [
        "concept:buffer",
        "index",
        "user:20260824-my-note",
        // The case an include-list filter would break:
        "1F0A-BEEF-4C21-9A33",
        "20260824T101500",
        "my-note-without-a-namespace",
        // Contains a colon but is not a URI scheme.
        "project:some/path",
    ] {
        assert!(
            !org::is_external_link_target(id),
            "'{id}' is a node id and must remain a graph edge"
        );
    }
}

/// The complement: things that cannot possibly be node ids.
#[test]
fn urls_and_org_link_schemes_are_external() {
    for target in [
        "https://example.com",
        "http://example.com/path?q=1",
        "file:notes.txt",
        "mailto:someone@example.com",
        "attachment:diagram.png",
        "docview:paper.pdf::12",
        "doi:10.1000/182",
        // Case-insensitive, because org content is hand-written.
        "HTTPS://EXAMPLE.COM",
        "  https://example.com  ",
    ] {
        assert!(
            org::is_external_link_target(target),
            "'{target}' cannot be a node id and must not become a graph edge"
        );
    }
}

/// End to end through the real ingest: a body mixing internal and external links
/// must yield edges for the internal ones ONLY.
///
/// Asserted on the parsed node's links rather than on the filter in isolation,
/// because the defect was that three separate edge-creation sites each forgot
/// it — testing the predicate alone would have passed while the sites stayed
/// open.
#[test]
fn ingest_creates_edges_only_for_internal_targets() {
    let content = "\
:PROPERTIES:
:ID: concept:host
:END:
#+title: Host

Prose with [[https://example.com][a site]], [[file:local.txt][a file]],
[[mailto:a@b.c][mail]], [[concept:buffer][Buffer]] and [[1F0A-BEEF][a uuid node]].
";
    let parsed = org::parse_org_multi_result(content);
    let targets: Vec<String> = parsed
        .typed_links
        .iter()
        .map(|(_, l)| l.target.clone())
        .collect();

    assert!(
        targets.contains(&"concept:buffer".to_string()),
        "an internal namespaced link must become an edge; got {targets:?}"
    );
    assert!(
        targets.contains(&"1F0A-BEEF".to_string()),
        "a bare-uuid node id must become an edge; got {targets:?}"
    );
    for external in ["https://example.com", "file:local.txt", "mailto:a@b.c"] {
        assert!(
            !targets.iter().any(|t| t == external),
            "'{external}' became a graph edge; it will be reported broken forever"
        );
    }
}
