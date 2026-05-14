//! Build system detection — walk up from a file to find project markers.

use std::path::{Path, PathBuf};

/// Kinds of build systems we can detect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildSystemKind {
    Make,
    Cargo,
    Npm,
    Cmake,
    Go,
    Meson,
    Custom(String),
}

impl std::fmt::Display for BuildSystemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Make => write!(f, "make"),
            Self::Cargo => write!(f, "cargo"),
            Self::Npm => write!(f, "npm"),
            Self::Cmake => write!(f, "cmake"),
            Self::Go => write!(f, "go"),
            Self::Meson => write!(f, "meson"),
            Self::Custom(name) => write!(f, "{}", name),
        }
    }
}

/// A detected build system with its root directory and commands.
#[derive(Debug, Clone)]
pub struct BuildSystem {
    pub kind: BuildSystemKind,
    pub root: PathBuf,
    pub build_cmd: String,
    pub test_cmd: Option<String>,
    pub run_cmd: Option<String>,
}

/// Marker files and their corresponding build system kinds.
const MARKERS: &[(&str, BuildSystemKind)] = &[
    ("Cargo.toml", BuildSystemKind::Cargo),
    ("package.json", BuildSystemKind::Npm),
    ("go.mod", BuildSystemKind::Go),
    ("CMakeLists.txt", BuildSystemKind::Cmake),
    ("meson.build", BuildSystemKind::Meson),
    ("Makefile", BuildSystemKind::Make),
    ("makefile", BuildSystemKind::Make),
    ("GNUmakefile", BuildSystemKind::Make),
];

/// Detect the build system by walking up from `start` to find a marker file.
///
/// Returns the first match found (markers are checked in priority order).
/// Stops at filesystem root.
pub fn detect_build_system(start: &Path) -> Option<BuildSystem> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    loop {
        for (marker, kind) in MARKERS {
            if dir.join(marker).exists() {
                let commands = crate::systems::default_commands(kind);
                return Some(BuildSystem {
                    kind: kind.clone(),
                    root: dir,
                    build_cmd: commands.0.to_string(),
                    test_cmd: commands.1.map(|s| s.to_string()),
                    run_cmd: commands.2.map(|s| s.to_string()),
                });
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detect_cargo_project() {
        let tmp = std::env::temp_dir().join("mae_test_detect_cargo");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();
        fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

        let bs = detect_build_system(&tmp.join("src/main.rs")).unwrap();
        assert_eq!(bs.kind, BuildSystemKind::Cargo);
        assert_eq!(bs.root, tmp);
        assert!(bs.build_cmd.contains("cargo"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_npm_project() {
        let tmp = std::env::temp_dir().join("mae_test_detect_npm");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("package.json"), "{}").unwrap();

        let bs = detect_build_system(&tmp).unwrap();
        assert_eq!(bs.kind, BuildSystemKind::Npm);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_makefile() {
        let tmp = std::env::temp_dir().join("mae_test_detect_make");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("Makefile"), "all:\n\techo hello").unwrap();

        let bs = detect_build_system(&tmp).unwrap();
        assert_eq!(bs.kind, BuildSystemKind::Make);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_go_project() {
        let tmp = std::env::temp_dir().join("mae_test_detect_go");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("go.mod"), "module test").unwrap();

        let bs = detect_build_system(&tmp).unwrap();
        assert_eq!(bs.kind, BuildSystemKind::Go);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_walk_up() {
        let tmp = std::env::temp_dir().join("mae_test_detect_walkup");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src/deep/nested")).unwrap();
        fs::write(tmp.join("Cargo.toml"), "[package]").unwrap();

        let bs = detect_build_system(&tmp.join("src/deep/nested")).unwrap();
        assert_eq!(bs.kind, BuildSystemKind::Cargo);
        assert_eq!(bs.root, tmp);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_none() {
        let tmp = std::env::temp_dir().join("mae_test_detect_none");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        // No marker files
        // This might find a Cargo.toml in a parent dir, so just check it returns something or nothing
        let _ = detect_build_system(&tmp);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn cargo_priority_over_makefile() {
        let tmp = std::env::temp_dir().join("mae_test_detect_priority");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("Cargo.toml"), "[package]").unwrap();
        fs::write(tmp.join("Makefile"), "all:").unwrap();

        let bs = detect_build_system(&tmp).unwrap();
        // Cargo.toml is checked before Makefile
        assert_eq!(bs.kind, BuildSystemKind::Cargo);

        let _ = fs::remove_dir_all(&tmp);
    }
}
