use super::*;

/// A root with a sibling directory outside it, both real.
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    Fixture {
        _tmp: tmp,
        root,
        outside,
    }
}

/// A directory name that exists nowhere, so a traversal through it can only be
/// judged lexically: the case #967's check got wrong.
fn absent() -> String {
    format!("absent-{}", uuid::Uuid::new_v4().simple())
}

#[test]
fn a_traversal_through_a_missing_directory_is_refused_relative_and_absolute() {
    let f = fixture();
    let a = absent();
    let rel = PathBuf::from(format!("../{a}/file"));
    assert_eq!(contained(&f.root, &rel), Err(Escape::ParentDir));
    // Absolute, still starting with the root: what a lexical starts_with let through.
    let abs = f.root.join("..").join(&a).join("file");
    assert!(abs.starts_with(&f.root), "the attack's premise");
    assert_eq!(contained(&f.root, &abs), Err(Escape::ParentDir));
    // A `..` that climbs back inside is refused too: no `..` at all.
    assert_eq!(
        contained(&f.root, Path::new("x/../y")),
        Err(Escape::ParentDir)
    );
    assert_eq!(contained(&f.root, Path::new("")), Err(Escape::ParentDir));
}

#[test]
fn a_new_path_below_the_root_lands_there() {
    let f = fixture();
    let a = absent();
    let out = contained(&f.root, Path::new(&format!("{a}/deeper/file.txt"))).unwrap();
    assert_eq!(out, f.root.join(&a).join("deeper/file.txt"));
    let abs = f.root.join("plain.txt");
    assert_eq!(contained(&f.root, &abs).unwrap(), abs);
    assert_eq!(
        contained(&f.root, Path::new("./dot.txt")).unwrap(),
        f.root.join("./dot.txt")
    );
}

#[test]
fn the_root_itself_and_a_sibling_sharing_its_name_prefix_are_outside() {
    let f = fixture();
    assert!(matches!(
        contained(&f.root, &f.root),
        Err(Escape::Outside(_))
    ));
    // `root-evil` starts with the string "root", not with the path `root`.
    let evil = f.root.with_file_name("root-evil");
    std::fs::create_dir_all(&evil).unwrap();
    assert!(matches!(
        contained(&f.root, &evil.join("f")),
        Err(Escape::Outside(_))
    ));
    assert!(matches!(
        contained(&f.root, &f.outside.join("f")),
        Err(Escape::Outside(_))
    ));
}

#[cfg(unix)]
#[test]
fn a_link_inside_the_root_that_points_out_is_followed_before_judging() {
    let f = fixture();
    std::os::unix::fs::symlink(&f.outside, f.root.join("door")).unwrap();
    // Below the link nothing exists: the write would create `outside/<a>/file`.
    let target = format!("door/{}/file", absent());
    match contained(&f.root, Path::new(&target)) {
        Err(Escape::Outside(p)) => assert_eq!(p, f.outside.canonicalize().unwrap()),
        other => panic!("a link out of the root must be refused: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_dangling_link_is_refused_and_a_final_link_is_reported() {
    let f = fixture();
    let nowhere = f.outside.join(absent());
    std::os::unix::fs::symlink(&nowhere, f.root.join("dangling")).unwrap();
    assert!(matches!(
        contained(&f.root, Path::new("dangling")),
        Err(Escape::Unresolvable(_))
    ));
    // A link to a file INSIDE the root is contained, but is still a link.
    std::fs::write(f.root.join("real.txt"), "x").unwrap();
    std::os::unix::fs::symlink(f.root.join("real.txt"), f.root.join("alias.txt")).unwrap();
    let out = contained(&f.root, Path::new("alias.txt")).unwrap();
    assert!(is_symlink(&out));
    assert!(!is_symlink(&f.root.join("real.txt")));
}

#[test]
fn a_root_that_does_not_exist_confines_nothing() {
    let f = fixture();
    let gone = f.root.join(absent());
    assert!(matches!(
        contained(&gone, Path::new("file")),
        Err(Escape::Unresolvable(_))
    ));
}
