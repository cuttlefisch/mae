//! mae-site: MAE's web core (ADR-119), the one derivation of a KB into pages
//! shared by the live app (in the daemon) and the static export.
//! @stability: experimental
//!
//! Callers hand it page sources (one Org text per page, from the store or a
//! bundle) and an [`Addressing`]; it returns sanitised pages. No I/O, no
//! clock, no configuration from content (D3).
//!
//! - [`address`]: where a page lives (D4): IDs only, never titles or paths.
//! - [`derive`]: the resolver built once per KB (D5) and each page rendered
//!   against it with mae-org.
//! - [`assets`]: the browser runtime's committed build, embedded and
//!   served under content-addressed names (D2, D17).
//! - [`strings`]: every string the engine writes, by locale (P-164).
//! - [`explorer`]: every published page, grouped by tag (P-39 to P-45).
//! - [`order`]: A to Z as a reader reads it, for every list of titles.
//! - [`sanitize`]: the final allow-list every content-derived fragment
//!   passes (D18, D19).

pub mod address;
pub mod assets;
pub mod derive;
pub mod explorer;
pub mod order;
pub mod sanitize;
pub mod strings;

pub use address::{AddressError, Addressing};
pub use derive::{
    derive, derive_with, scrub, Backlink, DerivedPage, IndexEntry, PageRef, Site, SiteOptions,
    SourcePage, TocEntry, Withheld,
};
