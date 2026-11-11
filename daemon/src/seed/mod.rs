//! Seeding a native KB from a tagged commit of an org repository.
//!
//! Three steps, each its own module:
//! - the PLANNER ([`plan`]) turns one commit's `.org` files into a [`SeedPlan`],
//!   the exact set of nodes a seed would create, and every reason it must not. It
//!   reads git and nothing else; it never opens a `DocStore`, so a dry run cannot
//!   write.
//! - [`apply()`] writes a plan with no refusal, whose hash is the one the operator
//!   reviewed, as a native, daemon-owned KB, ordered so a crash and a re-run
//!   converge (see `apply.rs`).
//! - [`export()`] writes each node back to its source path as N(F) (below),
//!   checking first that the file would seed back to the same node.
//!
//! ## The contract (`org-literal-v1`)
//!
//! A selected file F becomes ONE node; headline and list-item `:ID:`s inside it
//! stay in the body as anchors. [`split()`] lifts exactly three things into typed
//! fields: the file's leading `:PROPERTIES:` drawer, the single `#+title:` line,
//! and the single `#+filetags:` line. The body is F with those whole lines
//! removed, **byte for byte**. Nothing else is rewritten.
//!
//! [`regen()`] writes the fields back, and `N(F) = regen(split(F))` is the
//! normalisation the exporter's output is checked against:
//!
//! ```text
//! :PROPERTIES:
//! :KEY: value        one line per property, file order, key as written,
//! :KEY:              exactly one space, and `:KEY:` alone for an empty value
//! :END:
//! #+title: TITLE     only if the file has a title (`#+title:` if empty)
//! #+filetags: :a:b:  only if the file has a filetags line (`#+filetags:` if empty)
//! BODY               verbatim
//! ```
//!
//! Guarantees, each pinned by a test in `tests/`:
//! - **G1:** `regen(split(F)) == N(F)`, and the body is F minus the lifted lines,
//!   byte for byte.
//! - **G2:** `N(N(F)) == N(F)`. The planner checks this on every file and refuses a
//!   file where it does not hold, so it is not left to a test alone.
//! - **G3:** `F == N(F)` exactly when F's header is already in the form above.
//!   Every file where it is not is listed in the dry run with the differing lines.
//!   `--require-canonical` turns each one into a refusal.
//!
//! What a file must not contain, which is checked by [`split()`] and listed as a
//! refusal: a CR (LF only), a BOM, a missing or second file drawer `:ID:`, a
//! duplicated or `KEY+` property, a drawer line that is not a property, a second
//! `#+title`/`#+filetags`, either keyword outside the header block, an Org
//! declaration (`#+TODO`, `#+MACRO`, `#+LINK`, `#+PROPERTY`, `#+SETUPFILE`, …,
//! which belong to a per-KB declarations store that does not exist yet), or one of
//! MAE's own header keywords (`#+kind`, `#+aliases`, …), whose meaning would
//! then be stored twice.
//!
//! The `:ID:` is taken exactly as Org reads it. It is never derived from the file
//! name and never validated as a uuid.
//!
//! @ai-caution: [kb-truth] Every check here exists because silently dropping or
//! rewriting a file is the failure this seeder replaces (org ingest skipped id-less
//! files, rewrote links and lowercased keys, all while reporting success). A new
//! case either fits the contract or becomes a named refusal; never a default.

mod apply;
mod export;
mod git;
mod glob;
mod history;
mod plan;
mod report;
mod split;

pub use apply::{
    apply, build_collection, build_node, ApplyError, ApplyOutcome, ApplyRefusalKind, ApplyTarget,
};
pub use export::{export, render, ExportRefusal, ExportRefusalKind, ExportReport, Rendered};
pub use git::{GitRepo, TreeEntry};
pub use glob::{Glob, GlobError};
pub use history::{History, HISTORY_PREFIX_BYTES};
pub use plan::{
    plan, plan_entries, CheckoutAttr, PlanInput, PlanOptions, PlannedNode, Refusal, SeedPlan,
    Source, TimeSource,
};
pub use report::{render_json, render_text};
pub use split::{drawer_id, normalise, regen, split, SplitFile};

/// The storage contract a plan is made under. It names the split and the
/// normalisation above, and enters `plan_hash`, so a change to either is a new
/// contract rather than a silent change of meaning.
pub const CONTRACT: &str = "org-literal-v1";

/// The largest body a node may have: ADR-109 D6's client cap (`kb_max_node_bytes`,
/// 1 MiB). A larger file is refused. It is never truncated.
pub const DEFAULT_MAX_BODY_BYTES: usize = 1_048_576;

/// How much larger than the body limit a selected FILE may be and still be
/// read: room for its drawer and header lines. A larger blob is refused from
/// its size alone (`cat-file --batch-check`), without reading it. A body under
/// the limit behind more than this much header is refused too, which no real
/// page approaches.
pub const HEADER_ALLOWANCE_BYTES: usize = 64 * 1024;

/// The selection when no `--include` is given: every `.org` file in the tree.
pub const DEFAULT_INCLUDE: &str = "**/*.org";

/// Why a file, or the run, cannot be seeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProblemKind {
    NotUtf8,
    NulByte,
    ByteOrderMark,
    CarriageReturn,
    NotLossless,
    NoFileDrawer,
    UnrepresentableDrawer,
    MissingId,
    MultipleIds,
    EmptyId,
    InvalidId,
    DuplicateProperty,
    AppendProperty,
    MultipleTitle,
    MultipleFiletags,
    MisplacedHeaderKeyword,
    BadFiletags,
    Declaration,
    ExtensionKeyword,
    NotRoundTrippable,
    BodyTooLarge,
    DuplicateId,
    NonCanonical,
    ReceivedTime,
    Symlink,
    Submodule,
    NotOrgFile,
    NonUtf8Path,
    CheckoutAttribute,
    ExcludeNotSelected,
    NothingSelected,
}

impl ProblemKind {
    /// The stable name in the JSON report.
    pub fn as_str(self) -> &'static str {
        use ProblemKind::*;
        match self {
            NotUtf8 => "not_utf8",
            NulByte => "nul_byte",
            ByteOrderMark => "byte_order_mark",
            CarriageReturn => "carriage_return",
            NotLossless => "not_lossless",
            NoFileDrawer => "no_file_drawer",
            UnrepresentableDrawer => "unrepresentable_drawer",
            MissingId => "missing_id",
            MultipleIds => "multiple_ids",
            EmptyId => "empty_id",
            InvalidId => "invalid_id",
            DuplicateProperty => "duplicate_property",
            AppendProperty => "append_property",
            MultipleTitle => "multiple_title",
            MultipleFiletags => "multiple_filetags",
            MisplacedHeaderKeyword => "misplaced_header_keyword",
            BadFiletags => "bad_filetags",
            Declaration => "declaration",
            ExtensionKeyword => "extension_keyword",
            NotRoundTrippable => "not_round_trippable",
            BodyTooLarge => "body_too_large",
            DuplicateId => "duplicate_id",
            NonCanonical => "non_canonical",
            ReceivedTime => "received_time",
            Symlink => "symlink",
            Submodule => "submodule",
            NotOrgFile => "not_org_file",
            NonUtf8Path => "non_utf8_path",
            CheckoutAttribute => "checkout_attribute",
            ExcludeNotSelected => "exclude_not_selected",
            NothingSelected => "nothing_selected",
        }
    }
}

/// One problem found in one file, before it is attached to a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub kind: ProblemKind,
    pub detail: String,
}

impl Problem {
    pub fn new(kind: ProblemKind, detail: impl Into<String>) -> Self {
        Problem {
            kind,
            detail: detail.into(),
        }
    }
}

/// A failure to plan at all: git could not be run, the revision does not
/// resolve, a pattern is malformed. These are distinct from refusals, which are
/// findings about the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedError(pub String);

impl std::fmt::Display for SeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SeedError {}

#[cfg(test)]
mod tests;
