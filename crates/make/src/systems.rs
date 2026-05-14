//! Default build/test/run commands per build system.

use crate::detect::BuildSystemKind;

/// Returns (build_cmd, test_cmd, run_cmd) for a build system kind.
pub fn default_commands(
    kind: &BuildSystemKind,
) -> (&'static str, Option<&'static str>, Option<&'static str>) {
    match kind {
        BuildSystemKind::Cargo => ("cargo build", Some("cargo test"), Some("cargo run")),
        BuildSystemKind::Make => ("make", Some("make test"), None),
        BuildSystemKind::Npm => ("npm run build", Some("npm test"), Some("npm start")),
        BuildSystemKind::Cmake => ("cmake --build build", Some("ctest --test-dir build"), None),
        BuildSystemKind::Go => ("go build ./...", Some("go test ./..."), Some("go run .")),
        BuildSystemKind::Meson => ("ninja -C build", Some("meson test -C build"), None),
        BuildSystemKind::Custom(_) => ("make", None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_commands() {
        let (build, test, run) = default_commands(&BuildSystemKind::Cargo);
        assert_eq!(build, "cargo build");
        assert_eq!(test, Some("cargo test"));
        assert_eq!(run, Some("cargo run"));
    }

    #[test]
    fn make_commands() {
        let (build, test, run) = default_commands(&BuildSystemKind::Make);
        assert_eq!(build, "make");
        assert!(test.is_some());
        assert!(run.is_none());
    }

    #[test]
    fn all_systems_have_build_cmd() {
        let kinds = vec![
            BuildSystemKind::Cargo,
            BuildSystemKind::Make,
            BuildSystemKind::Npm,
            BuildSystemKind::Cmake,
            BuildSystemKind::Go,
            BuildSystemKind::Meson,
        ];
        for kind in kinds {
            let (build, _, _) = default_commands(&kind);
            assert!(!build.is_empty(), "missing build cmd for {:?}", kind);
        }
    }
}
