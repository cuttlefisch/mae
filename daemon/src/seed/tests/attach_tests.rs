//! ADR-115 P4: a page's attachment files through plan, apply, update and
//! export. Every repository and every image is generated here.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use mae_mcp::identity::Identity;
use mae_sync::kb::AttachmentRef;

use super::apply_tests::{memory_store, target, KB, T};
use super::git_tests::Repo;
use super::plan_opts;
use crate::blob_store::process::process;
use crate::blob_store::tests::{halves, hostile_jpeg, minimal_pdf, png_chunk, png_of, CAMERA};
use crate::blob_store::BlobStore;
use crate::doc_store::DocStore;
use crate::seed::*;

/// The first page's own `:ID:`, a headline's below it, and the second page's.
const P1: &str = "0a000000-0000-4000-8000-0000000000f1";
const H1: &str = "0b000000-0000-4000-8000-0000000000f2";
const P2: &str = "0c000000-0000-4000-8000-0000000000f3";

/// Pages and files of a repository, by path.
type Pages = Vec<(String, String)>;
type Files = Vec<(String, Vec<u8>)>;

/// org-attach's directory for `id`, relative to the page.
fn dir_of(id: &str) -> String {
    format!("data/{}/{}/", &id[..2], &id[2..])
}

fn write_bytes(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A PNG with no metadata, every pixel from `seed`, so one changed pixel is
/// one other file.
fn plain_png(seed: u8) -> Vec<u8> {
    let img = RgbImage::from_fn(8, 4, |x, y| Rgb([seed, x as u8 * 20, y as u8 * 40]));
    png_of(&DynamicImage::ImageRgb8(img), false)
}

/// Two pages: a JPEG with an orientation tag and EXIF, and a PNG, on the
/// first page's own `:ID:`; a PDF below a headline with its own `:ID:`,
/// linked from a sub-headline without one (inherited); the second page links
/// a PNG of its own under the same NAME.
fn fixture(png: &[u8]) -> (Pages, Files) {
    let pages = vec![
        (
            "notes/one.org".to_string(),
            format!(
                ":PROPERTIES:\n:ID: {P1}\n:END:\n#+title: One\n\
                 A photo [[attachment:photo.jpg]] and [[attachment:diagram.png]].\n\
                 * Section\n:PROPERTIES:\n:ID: {H1}\n:END:\n\
                 ** Deeper\nThe spec: [[attachment:spec.pdf]]\n"
            ),
        ),
        (
            "notes/two.org".to_string(),
            format!(":PROPERTIES:\n:ID: {P2}\n:END:\n#+title: Two\n[[attachment:diagram.png]]\n"),
        ),
    ];
    let files = vec![
        (format!("notes/{}photo.jpg", dir_of(P1)), hostile_jpeg(6)),
        (format!("notes/{}diagram.png", dir_of(P1)), png.to_vec()),
        (format!("notes/{}spec.pdf", dir_of(H1)), minimal_pdf()),
        (
            format!("notes/{}diagram.png", dir_of(P2)),
            png_of(&DynamicImage::ImageRgb8(halves(6, 6)), false),
        ),
    ];
    (pages, files)
}

fn repo_of(pages: &Pages, files: &Files) -> Repo {
    let r = Repo::new();
    for (path, text) in pages {
        r.write(path, text);
    }
    for (path, bytes) in files {
        write_bytes(r.path(), path, bytes);
    }
    r.commit(T);
    r.git(&["tag", "v1"]);
    r
}

/// A store with an attachment store, as the CLI opens one.
fn store_with_blobs() -> (Arc<DocStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = memory_store();
    store.set_blob_store(Arc::new(
        BlobStore::open(&dir.path().join("blobs")).unwrap(),
    ));
    (store, dir)
}

fn attach_from(r: &Repo, plan: &SeedPlan) -> SeedAttachments {
    SeedAttachments {
        reader: Arc::new(GitBlobs::open(r.path(), &plan.source.commit).unwrap()),
        limits: AttachmentLimits::default(),
        quota: None,
    }
}

/// Plan `rev`, apply it with its attachment source.
async fn seed(r: &Repo, store: &DocStore, owner: &Identity) -> SeedPlan {
    let plan = r.plan("v1");
    assert!(plan.ok(), "{:?}", plan.refusals);
    let hash = plan.plan_hash().unwrap();
    let mut t = target(owner, &hash);
    t.attach = Some(attach_from(r, &plan));
    let out = apply(store, &plan, &t).await.unwrap();
    assert!(matches!(out, ApplyOutcome::Applied { .. }), "{out:?}");
    plan
}

fn copy_tree(from: &Path, to: &Path) {
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&dest).unwrap();
            copy_tree(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).unwrap();
        }
    }
}

fn maps(plan: &SeedPlan) -> BTreeMap<String, BTreeMap<String, AttachmentRef>> {
    plan.nodes
        .iter()
        .map(|n| (n.file.id.clone(), n.attachment_map()))
        .collect()
}

/// seed, export, seed the export again: every page and file comes back, the
/// PDF and the metadata-free PNGs byte for byte, the JPEG as processing made
/// it (rotated by its tag, EXIF gone), and the second seed binds the same
/// hashes. Kills: binding no attachments at apply, writing a file at another
/// directory than the text's, or writing other bytes than the store's.
#[tokio::test]
async fn seed_export_seed_round_trips_every_attachment() {
    let png = plain_png(7);
    let (pages, files) = fixture(&png);
    let r = repo_of(&pages, &files);
    let (store, _blobs) = store_with_blobs();
    let owner = Identity::generate("daemon");
    let plan = seed(&r, &store, &owner).await;
    assert_eq!(plan.unique_attachments().len(), 4);
    let one = plan.nodes.iter().find(|n| n.file.id == P1).unwrap();
    let names: Vec<&str> = one.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["diagram.png", "photo.jpg", "spec.pdf"]);
    // The dry run lists every file with its processed hash and size (D7).
    let text = render_text(&plan);
    for a in &one.attachments {
        let line = format!("{} ({} bytes, {})", a.sha256, a.size, a.media_type);
        assert!(text.contains(&line), "{line} in\n{text}");
    }
    let json = render_json(&plan, &[]);
    assert_eq!(json["counts"]["attachments"], 4);

    let out = tempfile::tempdir().unwrap();
    let report = export(&store, KB, out.path(), false).await.unwrap();
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    let mut written = report.attachments.clone();
    written.sort();
    let mut want: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
    want.sort();
    assert_eq!(written, want);
    for (path, text) in &pages {
        let got = std::fs::read_to_string(out.path().join(path)).unwrap();
        assert_eq!(got, normalise(text).unwrap(), "{path}");
    }
    for (path, bytes) in &files {
        let got = std::fs::read(out.path().join(path)).unwrap();
        if path.ends_with(".jpg") {
            // The one documented difference: orientation applied, metadata gone.
            assert_ne!(&got, bytes);
            assert_eq!(
                got,
                process(bytes.clone(), &AttachmentLimits::default().process())
                    .unwrap()
                    .bytes
            );
            let (w, h) = image::load_from_memory(&got).unwrap().dimensions();
            assert_eq!((w, h), (16, 32), "orientation 6 turns 32x16 upright");
            assert!(!got.windows(6).any(|w| w == b"Exif\0\0"));
            assert!(!got.windows(CAMERA.len()).any(|w| w == CAMERA));
        } else {
            assert_eq!(&got, bytes, "{path} comes back byte for byte");
        }
    }

    // The export seeds back to the same attachments.
    let again = Repo::new();
    copy_tree(out.path(), again.path());
    again.commit(T);
    again.git(&["tag", "v1"]);
    let replan = again.plan("v1");
    assert!(replan.ok(), "{:?}", replan.refusals);
    assert_eq!(maps(&replan), maps(&plan));
}

/// Every link the seed cannot carry is a named refusal at its page, the dry
/// run lists them all, and an apply writes nothing: no node, no blob.
/// Kills: dropping any one refusal, following a symlink, or reading past the
/// size cap.
/// Each hostile link, the page number it is on, and the refusal it must get.
fn hostile_cases() -> Vec<(u8, &'static str, ProblemKind)> {
    vec![
        (
            1,
            "[[attachment:../../../etc/passwd]]",
            ProblemKind::AttachmentName,
        ),
        (2, "[[attachment:/etc/passwd]]", ProblemKind::AttachmentName),
        (3, "[[attachment:CON.png]]", ProblemKind::AttachmentName),
        (4, "[[attachment:a%2e%2e.png]]", ProblemKind::AttachmentName),
        (5, "[[attachment:evil.png]]", ProblemKind::AttachmentSymlink),
        (
            6,
            "[[attachment:inside.png]]",
            ProblemKind::AttachmentSymlink,
        ),
        (
            7,
            "[[attachment:bomb.png]]",
            ProblemKind::AttachmentTooManyPixels,
        ),
        (
            8,
            "[[attachment:photo.avif]]",
            ProblemKind::AttachmentUnsupported,
        ),
        (
            9,
            "[[attachment:huge.bin]]",
            ProblemKind::AttachmentTooLarge,
        ),
        (
            10,
            "[[attachment:absent.png]]",
            ProblemKind::AttachmentMissing,
        ),
    ]
}

/// The cap the hostile repository's oversized file is one byte over.
const HOSTILE_CAP: u64 = 32 * 1024;

/// A repository of hostile attachment links, one per case (page `p<i>`
/// refused with the kind given), plus an ID that climbs out of its
/// directory (`p11`) and one good page; committed and tagged `v1`.
fn hostile_repo() -> (Repo, Vec<(u8, &'static str, ProblemKind)>) {
    let id = |n: u8| format!("0d000000-0000-4000-8000-0000000000{n:02x}");
    let page = |i: u8, link: &str| {
        (
            format!("hostile/p{i}.org"),
            format!(
                ":PROPERTIES:\n:ID: {}\n:END:\n#+title: P{i}\n{link}\n",
                id(i)
            ),
        )
    };
    let at = |i: u8, name: &str| format!("hostile/{}{name}", dir_of(&id(i)));
    let mut bomb = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = 50_000u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&50_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    bomb.extend(png_chunk(b"IHDR", &ihdr));
    bomb.extend(png_chunk(b"IDAT", b"not zlib"));
    bomb.extend(png_chunk(b"IEND", b""));
    let avif = b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf".to_vec();
    let cases = hostile_cases();
    let mut pages: Vec<(String, String)> = cases.iter().map(|c| page(c.0, c.1)).collect();
    // An ID that would climb out of the page's directory.
    pages.push((
        "hostile/p11.org".into(),
        format!(
            ":PROPERTIES:\n:ID: {}\n:END:\n* H\n:PROPERTIES:\n:ID: ../../../../tmp/x\n:END:\n\
             [[attachment:a.png]]\n",
            id(11)
        ),
    ));
    // A good page beside them: its blob must not be stored either.
    pages.push((
        "hostile/good.org".into(),
        format!(":PROPERTIES:\n:ID: {P1}\n:END:\n[[attachment:ok.png]]\n"),
    ));
    let files = vec![
        (at(7, "bomb.png"), bomb),
        (at(8, "photo.avif"), avif),
        (at(9, "huge.bin"), vec![0x5a; HOSTILE_CAP as usize + 1]),
        (format!("hostile/{}ok.png", dir_of(P1)), plain_png(1)),
    ];
    let r = Repo::new();
    for (p, t) in &pages {
        r.write(p, t);
    }
    for (p, b) in &files {
        write_bytes(r.path(), p, b);
    }
    let link = |from: &str, to: &str| {
        let p = r.path().join(from);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(to, p).unwrap();
    };
    link(&at(5, "evil.png"), "/etc/passwd");
    // The directory itself is a symlink out of the repository.
    link(
        dir_of(&id(6))
            .trim_end_matches('/')
            .replace("data/", "hostile/data/")
            .as_str(),
        "/tmp",
    );
    r.commit(T);
    r.git(&["tag", "v1"]);
    (r, cases)
}

#[tokio::test]
async fn a_hostile_repository_is_refused_by_name_and_nothing_is_written() {
    let (r, cases) = hostile_repo();
    let cap = HOSTILE_CAP;

    let mut opts = plan_opts(r.path(), "v1");
    opts.attachment_limits.max_bytes = cap;
    let plan = crate::seed::plan(&opts).unwrap();
    let report = render_text(&plan);
    for (i, link, kind) in &cases {
        let path = format!("hostile/p{i}.org");
        assert!(
            plan.refusals
                .iter()
                .any(|f| f.path == path && f.kind == *kind),
            "{link}: want {kind:?} at {path}, got {:#?}",
            plan.refusals
        );
        assert!(report.contains(&path), "the dry run lists {path}");
    }
    assert!(plan
        .refusals
        .iter()
        .any(|f| f.path == "hostile/p11.org" && f.kind == ProblemKind::AttachmentPath));
    assert!(!plan.refusals.iter().any(|f| f.path == "hostile/good.org"));

    let (store, _blobs) = store_with_blobs();
    let owner = Identity::generate("daemon");
    let wrong = "0".repeat(64);
    let mut t = target(&owner, &wrong);
    t.attach = Some(attach_from(&r, &plan));
    let err = apply(&store, &plan, &t).await.unwrap_err();
    assert!(format!("{err:?}").contains("PlanRefused"), "{err:?}");
    assert!(store.blob_store().unwrap().list_all().unwrap().is_empty());
    assert_eq!(store.wal_high_water(&format!("kbc:{KB}")).await.unwrap(), 0);
}

/// Planning twice gives one hash; one changed pixel of one image gives
/// another, though no page changed. Kills: leaving attachments out of
/// `plan_hash`.
#[test]
fn plan_hash_covers_the_processed_bytes_of_every_attachment() {
    let (pages, files) = fixture(&plain_png(7));
    let a = repo_of(&pages, &files);
    let h1 = a.plan("v1").plan_hash().unwrap();
    assert_eq!(a.plan("v1").plan_hash().unwrap(), h1);
    let (pages, files) = fixture(&plain_png(8));
    let b = repo_of(&pages, &files);
    let h2 = b.plan("v1").plan_hash().unwrap();
    assert_ne!(h1, h2, "one image changed, the pages did not");
    // A PDF is stored as it is: one byte of it is another plan too.
    let (pages, mut files) = fixture(&plain_png(7));
    let pdf = files.iter_mut().find(|f| f.0.ends_with(".pdf")).unwrap();
    let last = pdf.1.len() - 3;
    pdf.1[last] ^= 1;
    let c = repo_of(&pages, &files);
    assert_ne!(c.plan("v1").plan_hash().unwrap(), h1);
}

/// An update replaces, removes and adds attachment files; the node's map is
/// the new plan's, and the replaced and removed blobs are no longer
/// referenced by any node. Kills: an update that leaves the map as seeded,
/// or binds a blob it did not store.
#[tokio::test]
async fn an_update_replaces_removes_and_adds_attachments() {
    let (pages, files) = fixture(&plain_png(7));
    let r = repo_of(&pages, &files);
    let (store, _blobs) = store_with_blobs();
    let owner = Identity::generate("daemon");
    let first = seed(&r, &store, &owner).await;
    store.set_signer(Arc::new(Identity::generate("daemon")));
    let old = maps(&first)[P1].clone();

    // diagram.png replaced, spec.pdf removed (link and file), extra.png added.
    let d1 = dir_of(P1);
    write_bytes(r.path(), &format!("notes/{d1}diagram.png"), &plain_png(9));
    std::fs::remove_file(r.path().join(format!("notes/{}spec.pdf", dir_of(H1)))).unwrap();
    // Below the headline with its own :ID:, so in its directory.
    write_bytes(
        r.path(),
        &format!("notes/{}extra.png", dir_of(H1)),
        &plain_png(11),
    );
    r.write(
        "notes/one.org",
        &format!(
            ":PROPERTIES:\n:ID: {P1}\n:END:\n#+title: One\n\
             A photo [[attachment:photo.jpg]] and [[attachment:diagram.png]].\n\
             * Section\n:PROPERTIES:\n:ID: {H1}\n:END:\n** Deeper\n[[attachment:extra.png]]\n"
        ),
    );
    // The second page's text is untouched; only its image changes.
    let p2_png = format!("notes/{}diagram.png", dir_of(P2));
    write_bytes(r.path(), &p2_png, &plain_png(13));
    r.commit(T + 100);
    r.git(&["tag", "v2"]);
    let plan = r.plan("v2");
    assert!(plan.ok(), "{:?}", plan.refusals);
    let hash = plan.plan_hash().unwrap();
    let mut t = target(&owner, &hash);
    t.attach = Some(attach_from(&r, &plan));
    let out = update(&store, &plan, &t, None).await.unwrap();
    assert!(
        matches!(out, UpdateOutcome::Updated { written: 2, .. }),
        "{out:?}"
    );
    let p2 = super::update_tests::node(&store, P2).await.attachments;
    assert_eq!(p2, maps(&plan)[P2], "an image-only change reaches its node");

    let now = super::update_tests::node(&store, P1).await.attachments;
    assert_eq!(now, maps(&plan)[P1]);
    assert_eq!(
        now.keys().collect::<Vec<_>>(),
        ["diagram.png", "extra.png", "photo.jpg"]
    );
    assert_eq!(now["photo.jpg"], old["photo.jpg"]);
    assert_ne!(now["diagram.png"], old["diagram.png"]);
    let mut referenced: Vec<String> = Vec::new();
    for id in [P1, P2] {
        let n = super::update_tests::node(&store, id).await;
        referenced.extend(n.attachments.into_values().map(|a| a.sha256));
    }
    let old_p2 = &maps(&first)[P2]["diagram.png"].sha256;
    for gone in [&old["diagram.png"].sha256, &old["spec.pdf"].sha256, old_p2] {
        assert!(!referenced.contains(gone), "{gone} is still referenced");
    }
    // Every blob a node binds is held.
    let held = store.blob_store().unwrap();
    for sha in &referenced {
        assert!(
            held.get(KB, sha).unwrap().is_some(),
            "{sha} bound but not stored"
        );
    }
}

/// `kb/query.get` lists each attachment, in NAME order, with the URL its blob
/// is read at; a reader who is no member gets the same refusal as before,
/// naming no hash. Kills: a wrong or missing `url`, the map shape, or
/// another KB's id in the URL.
#[tokio::test]
async fn kb_query_get_lists_each_attachment_with_its_blob_url() {
    let (pages, files) = fixture(&plain_png(7));
    let r = repo_of(&pages, &files);
    let (store, _blobs) = store_with_blobs();
    let owner = Identity::generate("daemon");
    let plan = seed(&r, &store, &owner).await;
    store.set_signer(Arc::new(Identity::generate("daemon")));
    let got = super::apply_tests::get(&store, &owner, P1).await;
    let one = plan.nodes.iter().find(|n| n.file.id == P1).unwrap();
    let want: Vec<serde_json::Value> = one
        .attachments
        .iter()
        .map(|a| {
            serde_json::json!({
                "name": a.name,
                "sha256": a.sha256,
                "size": a.size,
                "media_type": a.media_type,
                "url": format!("/kb/seeded/blobs/{}", a.sha256),
            })
        })
        .collect();
    assert_eq!(want.len(), 3);
    assert_eq!(got["attachments"], serde_json::Value::Array(want));
    check_fixture(&got);
    let p2 = super::apply_tests::get(&store, &owner, P2).await;
    assert_eq!(p2["attachments"].as_array().unwrap().len(), 1);

    let stranger = Identity::generate("stranger").fingerprint();
    let refused = crate::kb_query::dispatch(
        "kb/query.get",
        &serde_json::json!({"kb_id": KB, "node_id": P1}),
        &store,
        Some(&stranger),
        crate::kb_query::KbQueryLimits::default(),
    )
    .await
    .unwrap_err();
    let text = format!("{refused:?}");
    assert!(!text.contains(&one.attachments[0].sha256), "{text}");
    assert!(!text.contains("blobs"), "{text}");
}

/// `docs/verification/fixtures/kb-attach/query_get_attachments.json` is a
/// `kb/query.get` answer this daemon gave (written with `MAE_BLESS_FIXTURES`
/// set): the same fields, and every attachment's `url` its blob route.
fn check_fixture(got: &serde_json::Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../docs/verification/fixtures/kb-attach/query_get_attachments.json");
    if std::env::var_os("MAE_BLESS_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(got).unwrap() + "\n").unwrap();
    }
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let keys =
        |v: &serde_json::Value| -> Vec<String> { v.as_object().unwrap().keys().cloned().collect() };
    assert_eq!(keys(&fixture), keys(got), "regenerate {}", path.display());
    for v in [&fixture, got] {
        let kb = v["kb_id"].as_str().unwrap();
        for a in v["attachments"].as_array().unwrap() {
            assert_eq!(
                keys(a),
                ["media_type", "name", "sha256", "size", "url"],
                "{a}"
            );
            let url = format!("/kb/{kb}/blobs/{}", a["sha256"].as_str().unwrap());
            assert_eq!(a["url"], url);
        }
    }
}
