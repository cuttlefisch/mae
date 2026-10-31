//! `semantics()`'s rendered regions beyond comments and blocks: footnotes
//! Org re-adds from pruned regions, select tags, archived headlines, setup
//! files, and the diagnostics every unrendered drop leaves (ADR-112 D2, D4).
//! Each rule was checked against `emacs -Q` (Org 9.7.11) exporting the same
//! text with `org-export-as`.

mod support;

use mae_org::{
    parse, semantics, semantics_with, ArchivedTrees, LinkKind, ParseOptions, Region,
    SemanticDiagnostic, Semantics, SemanticsOptions, ORG_EXPORT_SELECT_TAGS,
};

fn sem(text: &str) -> Semantics {
    semantics(&parse(text, &ParseOptions::default()))
}

fn sem_with(text: &str, opts: &SemanticsOptions) -> Semantics {
    semantics_with(&parse(text, &ParseOptions::default()), opts)
}

fn ids(s: &Semantics) -> Vec<&str> {
    s.nodes.iter().map(|n| n.id.as_str()).collect()
}

/// (from, target) of every KB link.
fn edges(s: &Semantics) -> Vec<(String, String)> {
    s.links
        .iter()
        .filter(|l| l.kind == LinkKind::Kb)
        .map(|l| (l.from.clone().unwrap_or_default(), l.target.clone()))
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// Every diagnostic as (kind, text, region name).
fn diags(s: &Semantics) -> Vec<(&'static str, String, &'static str)> {
    s.diagnostics
        .iter()
        .map(|d| match d {
            SemanticDiagnostic::IdInUnrenderedRegion { id, region, .. } => {
                (d.kind_name(), id.clone(), region.as_str())
            }
            SemanticDiagnostic::LinkInUnrenderedRegion { path, region, .. } => {
                (d.kind_name(), path.clone(), region.as_str())
            }
            SemanticDiagnostic::TransclusionInUnrenderedRegion { member, region, .. } => {
                (d.kind_name(), member.clone(), region.as_str())
            }
            SemanticDiagnostic::DuplicateId { id, .. } => (d.kind_name(), id.clone(), ""),
            SemanticDiagnostic::SetupFileNotRead { file, .. } => (d.kind_name(), file.clone(), ""),
        })
        .collect()
}

fn link_diag(path: &str, region: Region) -> (&'static str, String, &'static str) {
    (
        "link_in_unrendered_region",
        path.to_string(),
        region.as_str(),
    )
}

/// `org-export--missing-definitions`: a definition in a pruned region that
/// rendered text references is re-added, so rendered. A COMMENT subtree is
/// deleted before parsing, so a definition there is not (Emacs: "Definition
/// not found for footnote 1"); a drawer's contents are parsed, so one in a
/// LOGBOOK is (Emacs renders it).
#[test]
fn a_referenced_definition_in_a_pruned_region_is_rendered() {
    let text = ":PROPERTIES:\n:ID: f\n:END:\nSee[fn:1] and[fn:2] and[fn:3].\n\n* Hidden :noexport:\n[fn:1] In noexport [[id:one]].\n\n\nNot a note [[id:hidden-text]].\n* COMMENT Commented\n[fn:2] In a comment subtree [[id:two]].\n* Logged\n:LOGBOOK:\n[fn:3] In a drawer [[id:three]].\n:END:\n* Also hidden :noexport:\n[fn:4] Unreferenced [[id:four]].\n";
    let s = sem(text);
    assert_eq!(edges(&s), pairs(&[("f", "one"), ("f", "three")]));
    assert_eq!(
        diags(&s),
        vec![
            link_diag("id:hidden-text", Region::ExcludedSubtree),
            link_diag("id:two", Region::CommentSubtree),
            link_diag("id:four", Region::ExcludedSubtree),
        ]
    );
}

/// A rendered definition's links belong to the node of the text that
/// references it (where the note is exported), not to the node it sits
/// under; a definition referenced only from another takes that one's node;
/// a node inside a definition keeps its own links.
#[test]
fn a_footnotes_links_belong_to_the_referencing_node() {
    let text = "* A\n:PROPERTIES:\n:ID: a\n:END:\nText[fn:x].\n* B\n:PROPERTIES:\n:ID: b\n:END:\nOther[fn:y].\n* Notes\n:PROPERTIES:\n:ID: notes\n:END:\n[fn:x] For A [[id:ax]], chained[fn:z].\n\n[fn:z] Chained [[id:zx]].\n#+TRANSCLUDE: zt\n\n[fn:y] For B [[id:by]].\n- item\n  :PROPERTIES:\n  :ID: i\n  :END:\n  [[id:in-item]]\n\n\nAfter the notes [[id:notes-own]].\n";
    let s = sem(text);
    let want = [
        ("a", "ax"),
        ("a", "zx"),
        ("b", "by"),
        ("i", "in-item"),
        ("notes", "notes-own"),
    ];
    let mut got = edges(&s);
    got.sort();
    let mut want = pairs(&want);
    want.sort();
    assert_eq!(got, want);
    let t: Vec<(Option<&str>, &str)> = s
        .transclusions
        .iter()
        .map(|t| (t.from.as_deref(), t.member.as_str()))
        .collect();
    assert_eq!(t, vec![(Some("a"), "zt")]);
    // A definition before its reference is attributed all the same.
    let s = sem("* A\n:PROPERTIES:\n:ID: a\n:END:\n[fn:1] Early [[id:e]].\n* B\n:PROPERTIES:\n:ID: b\n:END:\nLate[fn:1].\n");
    assert_eq!(edges(&s), pairs(&[("b", "e")]));
}

/// Every drop of a `#+TRANSCLUDE` leaves a diagnostic naming what the member
/// would have been.
#[test]
fn transclusions_in_unrendered_regions_are_diagnosed() {
    let text = "* COMMENT c\n#+TRANSCLUDE: in-comment-subtree\n* x :noexport:\n#+TRANSCLUDE: [[id:in-noexport][N]] :level 2\n* y\n#+begin_comment\n#+TRANSCLUDE: in-comment-block\n#+end_comment\n# #+TRANSCLUDE: in-comment-line\n#+TRANSCLUDE: rendered\n";
    let s = sem(text);
    let want: Vec<(&str, String, &str)> = [
        ("in-comment-subtree", Region::CommentSubtree),
        ("in-noexport", Region::ExcludedSubtree),
        ("in-comment-block", Region::CommentBlock),
        ("in-comment-line", Region::CommentLine),
    ]
    .iter()
    .map(|(m, r)| {
        (
            "transclusion_in_unrendered_region",
            m.to_string(),
            r.as_str(),
        )
    })
    .collect();
    assert_eq!(diags(&s), want);
    let members: Vec<&str> = s.transclusions.iter().map(|t| t.member.as_str()).collect();
    assert_eq!(members, vec!["rendered"]);
}

/// `org-export--selected-trees` and `--prune-tree`: when a headline carries
/// a select tag, only the selected trees and their ancestors (headline AND
/// section) are rendered, and the zeroth section's content is not. The
/// file's property drawer still defines the file node (it is front matter,
/// read like `#+title`).
#[test]
fn select_tags_render_only_the_selected_trees() {
    let org = SemanticsOptions {
        select_tags: ORG_EXPORT_SELECT_TAGS
            .iter()
            .map(|t| t.to_string())
            .collect(),
        ..SemanticsOptions::default()
    };
    let sem = |text: &str| sem_with(text, &org);
    let text = ":PROPERTIES:\n:ID: f\n:END:\n#+title: T [[id:in-title]]\nZeroth [[id:zeroth]].\n* A\n:PROPERTIES:\n:ID: a\n:END:\nA's text [[id:a-text]].\n** B :export:\n:PROPERTIES:\n:ID: b\n:END:\n[[id:b-text]]\n*** B1\n:PROPERTIES:\n:ID: b1\n:END:\n** C\n:PROPERTIES:\n:ID: c\n:END:\n[[id:c-text]]\n* D\n[[id:d-text]]\n";
    let s = sem(text);
    assert_eq!(ids(&s), vec!["f", "a", "b", "b1"]);
    assert_eq!(s.nodes[0].title.as_deref(), Some("T id:in-title"));
    assert_eq!(edges(&s), pairs(&[("a", "a-text"), ("b", "b-text")]));
    let unselected: Vec<String> = diags(&s)
        .into_iter()
        .filter(|d| d.2 == Region::Unselected.as_str())
        .map(|d| d.1)
        .collect();
    assert_eq!(
        unselected,
        vec!["id:in-title", "id:zeroth", "c", "id:c-text", "id:d-text"]
    );
    // Without a select tag in the file nothing is pruned.
    let plain = text.replace(" :export:", "");
    assert_eq!(ids(&sem(&plain)), vec!["f", "a", "b", "b1", "c"]);
    // In-file #+SELECT_TAGS replaces the configured tags; a file tag that
    // is a select tag selects every headline (and still hides the zeroth
    // section's content).
    let declared = "#+SELECT_TAGS: pick\n* A :export:\n:PROPERTIES:\n:ID: a\n:END:\n* B :pick:\n:PROPERTIES:\n:ID: b\n:END:\n";
    assert_eq!(ids(&sem(declared)), vec!["b"]);
    let by_file = ":PROPERTIES:\n:ID: f\n:END:\n#+filetags: :export:\nZeroth [[id:z]].\n* A\n:PROPERTIES:\n:ID: a\n:END:\n";
    let s = sem(by_file);
    assert_eq!(ids(&s), vec!["f", "a"]);
    assert!(edges(&s).is_empty());
    let opts = SemanticsOptions {
        select_tags: vec!["pick".into()],
        ..SemanticsOptions::default()
    };
    let configured =
        "* A :export:\n:PROPERTIES:\n:ID: a\n:END:\n* B :pick:\n:PROPERTIES:\n:ID: b\n:END:\n";
    assert_eq!(ids(&sem_with(configured, &opts)), vec!["b"]);
    assert_eq!(ids(&sem(configured)), vec!["a"]);
}

/// The KB default selects nothing: a note tagged `export` (because it is
/// about exporting) keeps its zeroth section, its nodes and its links. Only
/// a declared selection prunes, and an in-file `#+SELECT_TAGS:` is one.
#[test]
fn the_default_declares_no_select_tags() {
    let by_file = ":PROPERTIES:\n:ID: f\n:END:\n#+filetags: :export:\nZeroth [[id:z]].\n* A\n:PROPERTIES:\n:ID: a\n:END:\n";
    let s = sem(by_file);
    assert_eq!(ids(&s), vec!["f", "a"]);
    assert_eq!(edges(&s), pairs(&[("f", "z")]));
    assert!(diags(&s).iter().all(|d| d.2 != Region::Unselected.as_str()));
    let tagged = "* A :export:\n:PROPERTIES:\n:ID: a\n:END:\n* B\n:PROPERTIES:\n:ID: b\n:END:\n";
    assert_eq!(ids(&sem(tagged)), vec!["a", "b"]);
    let declared = format!("#+SELECT_TAGS: export\n{tagged}");
    assert_eq!(ids(&sem(&declared)), vec!["a"]);
}

/// A file tag that is an exclude tag excludes every headline (Org reads
/// tags with inheritance, file tags included); the zeroth section stays.
#[test]
fn an_excluding_file_tag_excludes_every_headline() {
    let text = ":PROPERTIES:\n:ID: f\n:END:\n#+filetags: :noexport:\n[[id:zeroth]]\n* A\n:PROPERTIES:\n:ID: a\n:END:\n";
    let s = sem(text);
    assert_eq!(ids(&s), vec!["f"]);
    assert_eq!(edges(&s), pairs(&[("f", "zeroth")]));
}

/// `org-export-with-archived-trees`, default `headline`: an archived
/// headline is rendered, its contents (section, drawer, children) are not,
/// so its id is no node. The tag is matched exactly (`:archive:` is not).
#[test]
fn an_archived_headline_renders_without_its_contents() {
    let text = "* A [[id:in-title]] :ARCHIVE:\n:PROPERTIES:\n:ID: a\n:END:\n[[id:a-text]]\n** A1\n:PROPERTIES:\n:ID: a1\n:END:\n* B :archive:\n:PROPERTIES:\n:ID: b\n:END:\n";
    let s = sem(text);
    assert_eq!(ids(&s), vec!["b"]);
    let links: Vec<(Option<&str>, &str)> = s
        .links
        .iter()
        .map(|l| (l.from.as_deref(), l.target.as_str()))
        .collect();
    assert_eq!(links, vec![(None, "in-title")]);
    let archived: Vec<String> = diags(&s)
        .into_iter()
        .filter(|d| d.2 == Region::Archived.as_str())
        .map(|d| d.1)
        .collect();
    assert_eq!(archived, vec!["a", "id:a-text", "a1"]);
    let with = |mode| SemanticsOptions {
        archived_trees: mode,
        ..SemanticsOptions::default()
    };
    assert_eq!(
        ids(&sem_with(text, &with(ArchivedTrees::All))),
        vec!["a", "a1", "b"]
    );
    let none = sem_with(text, &with(ArchivedTrees::None));
    assert_eq!(ids(&none), vec!["b"]);
    assert!(none.links.is_empty(), "{:?}", none.links);
    let tag = SemanticsOptions {
        archive_tag: "archive".into(),
        ..SemanticsOptions::default()
    };
    assert_eq!(ids(&sem_with(text, &tag)), vec!["a", "a1"]);
}

/// A `#+SETUPFILE` can change TODO keywords and tags; mae-org reads no
/// files, so it says so, wherever the keyword is.
#[test]
fn a_setup_file_is_diagnosed_not_read() {
    let text = "#+SETUPFILE: ~/org/setup.org\n* x :noexport:\n#+setupfile: \"other.org\"\n";
    let s = sem(text);
    let got: Vec<(&str, String)> = diags(&s)
        .into_iter()
        .filter(|d| d.0 == "setup_file_not_read")
        .map(|d| (d.0, d.1))
        .collect();
    assert_eq!(
        got,
        vec![
            ("setup_file_not_read", "~/org/setup.org".to_string()),
            ("setup_file_not_read", "\"other.org\"".to_string()),
        ]
    );
    let span = s.diagnostics[0].span();
    assert_eq!(&text[span.range()], "#+SETUPFILE: ~/org/setup.org");
}

/// Exclude and select tags, the archive check and the file tags are each a
/// set lookup: a hostile tag list is linear (see `tests/scaling.rs`), and
/// the answer is the same as for a short one.
#[test]
fn many_tags_resolve_like_few() {
    let words: Vec<String> = (0..300).map(|i| format!("x{i}")).collect();
    let text = format!(
        "#+EXCLUDE_TAGS: {}\n* A :y:x299:\n:PROPERTIES:\n:ID: a\n:END:\n* B :y:\n:PROPERTIES:\n:ID: b\n:END:\n",
        words.join(" ")
    );
    assert_eq!(ids(&sem(&text)), vec!["b"]);
}
