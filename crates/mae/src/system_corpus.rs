//! The source corpora for MAE's system KBs, compiled into the binary.
//!
//! # Why embed rather than ship files
//!
//! A system KB's store is derived from a directory of `.org` files. The sources
//! are a small fraction of the release binary, while the *stores* built from
//! them are 53–159× their source text — so embedding the sources costs far less
//! than shipping the products.
//!
//! Note the embedded bytes only reach the binary once something **uses** them:
//! with the statics referenced solely from `#[cfg(test)]` code, the linker
//! dropped them entirely and the "embedded" build measured *smaller* than the
//! one without. `bootstrap`'s fallback below is that consumer.
//!
//! The alternative, shipping the sources alongside the binary and finding them
//! by path, is the approach whose failure mode this repo has already recorded
//! twice. `pkg/embedded.rs` exists because a brew macOS install missed every
//! filesystem search path and got 0 modules — no `SPC` leader tree at all. And
//! ADR-076's own Context notes that MaePractices and the ADR KB were never
//! wired into `release.yml`/`install.sh`, so "the overwhelming majority of real
//! users got neither".
//!
//! Embedding makes the corpora present by construction. That is what lets
//! Windows, the Docker image and `cargo install` — all of which ship **zero**
//! KB corpora today — have MAE's documentation and practices at all, with no
//! per-platform packaging step to forget.
//!
//! # On-disk wins
//!
//! Exactly as with modules: an on-disk `assets/<corpus>` overrides the embedded
//! copy. That preserves the dev loop — edit a `.org` file and rebuild the KB
//! without recompiling — and it is why `include_dir!`'s compile-unit
//! invalidation is not a practical cost for contributors.
//!
//! # Not the ADR corpus
//!
//! `docs/adr/*.md` is 1.17 MB, contributor-only, and deliberately opt-in
//! (ADR-059). It ships as its own downloadable asset and stays out of every
//! user's binary.

use include_dir::{include_dir, Dir};
use mae_kb::system_kb::SystemKb;
use std::path::{Path, PathBuf};

static MANUAL: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/manual");
static PRACTICES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/practices");
static DEVPRACTICES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/devpractices");

/// The embedded corpus for `kb`, if MAE ships one.
///
/// `None` for the ADR corpus — catalogued, but not embedded (see module docs).
fn embedded(kb: &SystemKb) -> Option<&'static Dir<'static>> {
    match kb.corpus_dir {
        "assets/manual" => Some(&MANUAL),
        "assets/practices" => Some(&PRACTICES),
        "assets/devpractices" => Some(&DEVPRACTICES),
        _ => None,
    }
}

/// Where `kb`'s `.org` sources are, as a real directory a builder can walk.
///
/// Resolution, in order:
///
/// 1. **On-disk `assets/<corpus>`**, found by walking up from the running
///    executable — a dev checkout or a source build. Overrides embedded so the
///    edit-and-rebuild loop never requires a recompile.
/// 2. **Embedded**, materialised into `cache_dir` once per version.
///
/// The materialised copy lives in *cache*, not data: it is a verbatim copy of
/// bytes inside the binary, so it is rebuildable by definition and a user (or a
/// cleanup tool) deleting it loses nothing.
pub fn resolve(kb: &SystemKb, cache_dir: &Path) -> Option<PathBuf> {
    if let Some(on_disk) = on_disk_corpus(kb) {
        return Some(on_disk);
    }
    let dir = embedded(kb)?;
    let dst = cache_dir.join("kb-src").join(corpus_slug(kb));
    match materialise(dir, &dst) {
        Ok(()) => Some(dst),
        Err(e) => {
            tracing::warn!(kb = kb.name, dst = %dst.display(), error = %e,
                "failed to materialise embedded corpus");
            None
        }
    }
}

/// The last path component of `corpus_dir` — `"assets/devpractices"` ->
/// `"devpractices"`. Used as the cache subdirectory name.
fn corpus_slug(kb: &SystemKb) -> &str {
    kb.corpus_dir.rsplit('/').next().unwrap_or(kb.corpus_dir)
}

/// An on-disk `assets/<corpus>` beside (or above) the running executable.
///
/// Mirrors the exe-ancestor walk `guidance_kb_engine::well_known_paths` uses
/// for built stores, so a dev checkout resolves sources and stores the same way.
/// Requires `index.org` rather than mere directory existence: an empty or
/// half-deleted `assets/` should fall through to embedded rather than resolve
/// to a corpus that builds nothing.
fn on_disk_corpus(kb: &SystemKb) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for ancestor in exe.ancestors() {
        let candidate = ancestor.join(kb.corpus_dir);
        if candidate.join("index.org").exists() {
            return Some(candidate);
        }
    }
    None
}

/// Write `dir`'s files into `dst`, skipping the work when a previous run of the
/// same MAE version already did it.
///
/// Version-stamped rather than content-hashed because embedded bytes cannot
/// change without the binary changing — the version *is* the content key here.
/// A stale or partial extraction is corrected by re-extracting rather than
/// detected, which keeps this cheap enough to run at every startup.
fn materialise(dir: &Dir<'static>, dst: &Path) -> std::io::Result<()> {
    let stamp = dst.join(".mae-version");
    if std::fs::read_to_string(&stamp).is_ok_and(|v| v.trim() == env!("CARGO_PKG_VERSION")) {
        return Ok(());
    }

    // Extract into a sibling temp dir and rename into place, so a process killed
    // mid-extraction never leaves a half-written corpus that the stamp would
    // later certify as complete.
    let tmp = dst.with_extension(format!("extracting-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    for file in dir.files() {
        let name = file.path().file_name().unwrap_or_default();
        std::fs::write(tmp.join(name), file.contents())?;
    }
    std::fs::write(tmp.join(".mae-version"), env!("CARGO_PKG_VERSION"))?;

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_dir_all(dst);
    std::fs::rename(&tmp, dst)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb(name: &str) -> &'static SystemKb {
        mae_kb::system_kb::find(name).expect("catalog entry")
    }

    /// Every auto-enabled corpus must actually be embedded. A catalog entry
    /// whose corpus MAE forgot to compile in would resolve to nothing on the
    /// platforms this exists to serve — and would do so silently.
    #[test]
    fn every_auto_enabled_corpus_is_embedded() {
        for entry in mae_kb::system_kb::auto_enabled() {
            assert!(
                embedded(entry).is_some(),
                "{} is auto-enabled but has no embedded corpus",
                entry.name
            );
        }
    }

    /// ...and the ADR corpus deliberately is not: 1.17 MB of contributor-only
    /// decision history has no business in every user's binary.
    #[test]
    fn the_adr_corpus_is_not_embedded() {
        assert!(embedded(kb("ADR")).is_none());
    }

    /// The embedded content is the real corpus, not an empty directory that
    /// happens to exist. Each is asserted on a distinctive string from its own
    /// `index.org`, so this cannot pass against a stub or against the wrong
    /// corpus having been embedded under the wrong name.
    #[test]
    fn the_embedded_corpora_carry_their_own_real_content() {
        // Markers are distinctive strings from each corpus's OWN index.org
        // title, so embedding the wrong corpus under a name fails here rather
        // than passing on a shared boilerplate line like `:ID: index`.
        for (entry, marker) in [
            (kb("DevPractices"), "DevPractices — Development Conventions"),
            (kb("MaePractices"), "MAE Development Practices"),
            (kb("manual"), "MAE"),
        ] {
            let dir = embedded(entry).expect("embedded");
            let index = dir
                .files()
                .find(|f| f.path().ends_with("index.org"))
                .unwrap_or_else(|| panic!("{} must embed an index.org", entry.name));
            let text = index.contents_utf8().expect("utf8");
            assert!(
                text.contains(marker),
                "{}'s embedded index.org does not contain {marker:?} -- wrong corpus embedded?",
                entry.name
            );
        }
    }

    /// The whole point: the embedded corpus alone yields a directory a builder
    /// can walk — which is what Windows, the Docker image and `cargo install`
    /// get, since none of them ships `assets/`.
    ///
    /// Exercises `materialise` rather than `resolve`, deliberately and with the
    /// name saying so: `on_disk_corpus` walks the *test binary's* ancestors,
    /// which in this checkout genuinely do contain `assets/devpractices`, and
    /// `current_exe()` is not something a test can redirect. Asserting through
    /// `resolve` here would silently exercise the on-disk branch and prove
    /// nothing about the embedded one.
    #[test]
    fn the_embedded_corpus_materialises_into_a_walkable_directory() {
        let cache = tempfile::tempdir().unwrap();
        let entry = kb("DevPractices");
        let dst = cache.path().join("kb-src").join("devpractices");
        materialise(embedded(entry).unwrap(), &dst).expect("materialise");

        assert!(
            dst.join("index.org").exists(),
            "the materialised corpus must contain index.org"
        );
        let count = std::fs::read_dir(&dst)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "org"))
            .count();
        assert!(
            count > 50,
            "expected the real devpractices corpus, got {count} files"
        );
    }

    /// Re-extraction is skipped when the stamp matches, and the stamp is what
    /// decides — not mere directory existence, which would leave a corpus from
    /// an older MAE in place forever.
    #[test]
    fn materialise_is_idempotent_but_refreshes_on_a_version_change() {
        let cache = tempfile::tempdir().unwrap();
        let dst = cache.path().join("kb-src").join("practices");
        let dir = embedded(kb("MaePractices")).unwrap();

        materialise(dir, &dst).expect("first");
        let marker = dst.join("__user_scribble__");
        std::fs::write(&marker, b"x").unwrap();

        // Same version: skipped, so the scribble survives.
        materialise(dir, &dst).expect("second");
        assert!(marker.exists(), "same-version extraction must be skipped");

        // A stamp from another version forces a clean re-extraction.
        std::fs::write(dst.join(".mae-version"), "0.0.0-other").unwrap();
        materialise(dir, &dst).expect("third");
        assert!(
            !marker.exists(),
            "a version change must re-extract, not leave the old corpus in place"
        );
        assert!(dst.join("index.org").exists());
    }
}
