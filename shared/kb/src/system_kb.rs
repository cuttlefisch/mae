//! The catalog of **system KBs** — MAE's own operational corpora, as distinct
//! from the user KBs MAE merely manages.
//!
//! ## Why this exists
//!
//! MAE ships several knowledge bases that exist to drive *MAE's own behaviour
//! and its AI peer's behaviour*: the manual/help corpus, MaePractices,
//! DevPractices, and the ADR corpus. Their node structure is the same as a
//! user's notes, so they were modelled the same way — but almost nothing else
//! about them is the same:
//!
//! | | system KB | user KB |
//! |---|---|---|
//! | truth | ships with the binary | the user's own content |
//! | mutability | read-only | read-write |
//! | shareable / CRDT | never | yes |
//! | on upgrade | rebuilt | migrated, never destroyed |
//! | losing one | harmless, rebuildable | data loss |
//!
//! Modelling them identically produced a specific, reproducible set of bugs —
//! `kb_share` on a bundled corpus (which strips provenance on the receiving
//! peer, since `NodeSource` is not in the CRDT wire payload), `kb_reimport`
//! emptying a dir-less guidance KB, duplicate registry rows under one name,
//! `kb_health` reporting an upstream corpus's link hygiene as the user's
//! problem, and a backup script that archives the four rebuildable stores
//! while ignoring the irreplaceable ones.
//!
//! ## Why a compile-time catalog rather than a registry
//!
//! The set of system KBs is known when the binary is built. It does not need
//! persisting, and persisting it is what caused the trouble: once a system KB
//! is a row in `kb-registry.toml` next to the user's own, every operation that
//! iterates the registry treats it as user data.
//!
//! This is deliberately **not** the "second registry file" ADR-058 rejected.
//! That rejection was about duplicating *persistence* — "two sources of truth
//! a future reader must merge". A compile-time catalog persists nothing and
//! cannot drift from the binary that contains it.
//!
//! ## What this module is and is not
//!
//! Pure data plus lookups. It deliberately performs no I/O, so `mae-core`,
//! `mae-ai`, `mae` and the daemon can all agree on *which names are system
//! names* without any of them depending on how those corpora are located or
//! built.

/// What a system KB is *for*. Distinct from `KbInstanceKind`, which describes
/// a registry row — a system KB has no registry row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemKbRole {
    /// MAE's own manual/help corpus, reached through `SPC h *` / `help_open`.
    /// Its content is partly code-generated from the live command/option/hook
    /// registries, so it describes *the running binary* and cannot meaningfully
    /// be shipped pre-baked across versions.
    Manual,
    /// A corpus of standing practices surfaced to every AI session via
    /// `ai_guidance_kb`.
    Guidance,
    /// A reference corpus that is available but not surfaced automatically.
    Reference,
}

/// One entry in the system-KB catalog.
#[derive(Debug, Clone, Copy)]
pub struct SystemKb {
    /// The name this corpus is known by — to `ai_guidance_kb`, to search-result
    /// labelling, and as a **reserved** name no user KB may claim.
    pub name: &'static str,
    /// Directory of source documents, relative to the workspace root, e.g.
    /// `"assets/devpractices"`. This is the corpus's source of truth; the
    /// built store is a derived cache of it.
    pub corpus_dir: &'static str,
    /// Filename of the pre-built store as shipped today, e.g.
    /// `"mae-devpractices.cozo"`. Retained so existing install layouts and the
    /// well-known-path probes keep resolving during the transition.
    pub asset_filename: &'static str,
    /// Env var overriding location resolution for this corpus.
    pub env_override: &'static str,
    /// What it is for.
    pub role: SystemKbRole,
    /// Whether MAE makes this corpus available without being asked.
    ///
    /// The ADR corpus is `false`: it is MAE's own decision history, useful to
    /// contributors and noise to everyone else, and ADR-059 deliberately kept
    /// it opt-in. Being in this catalog still means it is read-only and
    /// name-reserved — `auto_enable` governs availability, not class.
    pub auto_enable: bool,
}

/// The manual/help corpus's catalog name.
///
/// Named rather than spelled out at use sites because it is load-bearing in two
/// places that must agree: the query layer's primary fallback, and the
/// pseudo-instance label a search hit carries.
pub const MANUAL: &str = "manual";

/// Every system KB MAE knows about.
///
/// Adding an entry reserves its name against `kb_register` and makes it
/// read-only, so entries are added deliberately, not incidentally.
pub const SYSTEM_KBS: &[SystemKb] = &[
    SystemKb {
        name: MANUAL,
        corpus_dir: "assets/manual",
        asset_filename: "mae-manual.cozo",
        env_override: "MAE_MANUAL_PATH",
        role: SystemKbRole::Manual,
        auto_enable: true,
    },
    SystemKb {
        name: "MaePractices",
        corpus_dir: "assets/practices",
        asset_filename: "mae-practices.cozo",
        env_override: "MAE_PRACTICES_KB_PATH",
        role: SystemKbRole::Guidance,
        auto_enable: true,
    },
    SystemKb {
        name: "DevPractices",
        corpus_dir: "assets/devpractices",
        asset_filename: "mae-devpractices.cozo",
        env_override: "MAE_DEVPRACTICES_KB_PATH",
        role: SystemKbRole::Guidance,
        auto_enable: true,
    },
    SystemKb {
        name: "ADR",
        corpus_dir: "docs/adr",
        asset_filename: "mae-adr.cozo",
        env_override: "MAE_ADR_KB_PATH",
        role: SystemKbRole::Reference,
        auto_enable: false,
    },
];

/// Look up a system KB by name. Case-insensitive, because `ai_guidance_kb` is
/// user-typed and `"devpractices"` should not silently resolve to nothing.
pub fn find(name: &str) -> Option<&'static SystemKb> {
    SYSTEM_KBS
        .iter()
        .find(|kb| kb.name.eq_ignore_ascii_case(name))
}

/// Whether `name` is reserved by the system catalog and therefore unavailable
/// to `kb_register`.
///
/// Reserving the name is what makes "which corpus is DevPractices?" have one
/// answer. Before this, `KbRegistry::register` deduplicated on `org_dir` and
/// never on `name`, so registering a second `DevPractices` silently produced a
/// duplicate row that `find()` would never return — meaning ADR-076 D4's
/// documented "your own registration always wins" held only if you happened to
/// register *before* startup auto-registration ran.
pub fn is_reserved_name(name: &str) -> bool {
    find(name).is_some()
}

/// The system KBs MAE makes available without being asked.
pub fn auto_enabled() -> impl Iterator<Item = &'static SystemKb> {
    SYSTEM_KBS.iter().filter(|kb| kb.auto_enable)
}

/// User cache path: `$XDG_CACHE_HOME/mae`, else `~/.cache/mae`.
///
/// XDG-first on **every** platform, deliberately not `dirs::cache_dir()` —
/// that follows Apple conventions on macOS and ignores `XDG_CACHE_HOME`, which
/// both contradicts the documented `~/.cache/mae` contract and breaks env-var
/// test isolation (CLAUDE.md principle #13).
///
/// This lives here, in the crate that owns the catalog, because both the
/// builder (`crates/mae`) and the reader (`crates/ai`) must agree on it and
/// neither can depend on the other. `crates/mae/src/pkg/paths.rs` delegates
/// here rather than keeping a second copy.
pub fn default_cache_dir() -> Option<std::path::PathBuf> {
    std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".cache"))
        })
        .map(|base| base.join("mae"))
}

/// Where `kb`'s built store lives: `<cache>/kb/<name>-<version>.cozo`.
///
/// **The single authority for this path, and it has to stay that way.** It was
/// briefly computed in two places — the builder wrote
/// `<cache>/kb/<name>-<version>.cozo` while the guidance reader derived
/// `<data dir>/<asset_filename>` — and because nothing tested the join, a
/// clean install built the store correctly and then delivered no guidance at
/// all. Two functions answering "where does this KB live" is exactly the
/// duplicated-mechanism failure principle #8 exists to prevent; if you need
/// this path, call this function.
///
/// Version-keyed so an upgrade rebuilds rather than serving whatever content
/// first landed. The version is this crate's own `CARGO_PKG_VERSION`: every
/// crate in the workspace shares one version, so taking it here yields one
/// answer for every caller, where a passed-in version could differ per caller
/// and silently split the cache.
pub fn built_store_path(cache_dir: &std::path::Path, kb: &SystemKb) -> std::path::PathBuf {
    cache_dir.join("kb").join(format!(
        "{}-{}.cozo",
        kb.asset_filename.trim_end_matches(".cozo"),
        env!("CARGO_PKG_VERSION")
    ))
}

/// What a reserved-name registry row turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservedRow {
    /// MAE put it there. Safe to remove: the corpus is now served from the
    /// catalog, and the row's store is a derived copy of a shipped asset.
    MaeProvisioned,
    /// A reserved name, but not the shape MAE writes — so someone pointed it at
    /// content of their own. Never removed automatically: the row may be the
    /// only record of where that content lives.
    UserOwned,
}

/// Classify a registry row that carries a reserved system-KB name.
///
/// **Keyed on shape, deliberately not on `kind`.** `KbInstanceKind::Guidance`
/// looks like the natural discriminator and is not trustworthy: on a real
/// machine that has run MAE for months, the two auto-registered rows were found
/// carrying *different* kinds — `DevPractices` as `Guidance` and `MaePractices`
/// as `UserRegistered` — because the variant post-dates the code that writes
/// some of these rows. A field that is wrong on half the population cannot
/// decide whether to delete something.
///
/// The shape MAE writes, on the other hand, is exact and was never produced by
/// `kb_register`: a **dir-less** instance whose `db_path` is the data dir's own
/// copy of the shipped asset (see `guidance_kb_engine::ensure_registered`).
///
/// `data_dir` is the caller's resolved MAE data directory.
pub fn classify_reserved_row(
    name: &str,
    org_dir: &std::path::Path,
    db_path: &std::path::Path,
    data_dir: &std::path::Path,
) -> Option<ReservedRow> {
    let entry = find(name)?;
    let mae_shape =
        org_dir.as_os_str().is_empty() && db_path == data_dir.join(entry.asset_filename).as_path();
    Some(if mae_shape {
        ReservedRow::MaeProvisioned
    } else {
        ReservedRow::UserOwned
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names are the reservation mechanism, so a duplicate would make
    /// `find()` order-dependent — the same class of bug as the duplicate
    /// registry rows this catalog exists to prevent.
    #[test]
    fn catalog_names_are_unique_case_insensitively() {
        for (i, a) in SYSTEM_KBS.iter().enumerate() {
            for b in SYSTEM_KBS.iter().skip(i + 1) {
                assert!(
                    !a.name.eq_ignore_ascii_case(b.name),
                    "duplicate system KB name: {} / {}",
                    a.name,
                    b.name
                );
            }
        }
    }

    #[test]
    fn every_entry_is_findable_by_its_own_name() {
        for kb in SYSTEM_KBS {
            let found = find(kb.name).expect("entry must be findable by its own name");
            assert_eq!(found.name, kb.name);
        }
    }

    /// `ai_guidance_kb` is typed by a human into `init.scm`. Rejecting
    /// `"devpractices"` while accepting `"DevPractices"` is the shape of bug
    /// that makes a setting look broken.
    #[test]
    fn lookup_is_case_insensitive() {
        assert!(find("devpractices").is_some());
        assert!(find("DEVPRACTICES").is_some());
        assert!(find("DevPractices").is_some());
    }

    /// The negative case matters more than the positive one: reserving a name
    /// blocks `kb_register`, so over-reserving would lock users out of names
    /// MAE has no claim to.
    #[test]
    fn ordinary_names_are_not_reserved() {
        for name in ["FieldJournal", "my-notes", "practices", "adr-notes", ""] {
            assert!(
                !is_reserved_name(name),
                "{name:?} must not be reserved -- reserving blocks kb_register"
            );
        }
    }

    #[test]
    fn reserved_names_cover_every_catalog_entry_including_opt_in_ones() {
        assert!(
            is_reserved_name("ADR"),
            "opt-in is about availability, not class"
        );
        for kb in SYSTEM_KBS {
            assert!(is_reserved_name(kb.name));
        }
    }

    /// The migration predicate decides whether a row gets DELETED, so its
    /// negative half matters more than its positive one.
    ///
    /// Both fixtures are the real shapes seen on a machine that had run MAE for
    /// months — including the one that motivated keying on shape rather than
    /// `kind`: `MaePractices` was stamped `UserRegistered` and `DevPractices`
    /// `Guidance`, though MAE wrote both.
    #[test]
    fn a_mae_provisioned_row_is_recognised_by_shape_whatever_its_kind_says() {
        let data = std::path::Path::new("/home/u/.local/share/mae");
        for name in ["MaePractices", "DevPractices", "manual"] {
            let entry = find(name).unwrap();
            assert_eq!(
                classify_reserved_row(
                    name,
                    std::path::Path::new(""),
                    &data.join(entry.asset_filename),
                    data
                ),
                Some(ReservedRow::MaeProvisioned),
                "{name}"
            );
        }
    }

    /// A reserved name pointing at the user's own org directory is theirs, and
    /// removing it would discard the only record of where that content lives.
    #[test]
    fn a_reserved_name_over_a_users_own_content_is_never_mae_provisioned() {
        let data = std::path::Path::new("/home/u/.local/share/mae");
        // Their own org dir under the reserved name (ADR-076 D4's override).
        assert_eq!(
            classify_reserved_row(
                "DevPractices",
                std::path::Path::new("/home/u/notes/practices"),
                &data.join("mae-devpractices.cozo"),
                data
            ),
            Some(ReservedRow::UserOwned)
        );
        // Dir-less, but pointing somewhere MAE never writes.
        assert_eq!(
            classify_reserved_row(
                "DevPractices",
                std::path::Path::new(""),
                std::path::Path::new("/home/u/elsewhere/mine.cozo"),
                data
            ),
            Some(ReservedRow::UserOwned)
        );
    }

    /// An ordinary KB is not classified at all — the migration must never even
    /// consider deleting a row it has no claim to.
    #[test]
    fn an_unreserved_name_is_not_classified() {
        let data = std::path::Path::new("/home/u/.local/share/mae");
        assert_eq!(
            classify_reserved_row(
                "MyNotes",
                std::path::Path::new(""),
                &data.join("mae-devpractices.cozo"),
                data
            ),
            None
        );
    }

    /// `auto_enable` governs availability only. If this ever equals the full
    /// catalog, the ADR corpus has silently become a default — dozens of
    /// decision records injected into every session, which ADR-059 rejected.
    #[test]
    fn the_adr_corpus_is_available_but_not_automatic() {
        let auto: Vec<_> = auto_enabled().map(|kb| kb.name).collect();
        assert!(!auto.contains(&"ADR"));
        assert!(auto.contains(&"DevPractices"));
        assert!(auto.contains(&"MaePractices"));
        assert!(auto.contains(&"manual"));
    }
}
