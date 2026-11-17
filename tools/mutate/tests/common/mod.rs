//! A fixture repository per test: the crate under `tests/fixture`, copied into
//! its own directory under cargo's per-target temp dir, made a git repository,
//! and driven through the real `mae-mutate` binary. Git and the tool run with
//! no global or system git config, and never through a job queue.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const FIXTURE_LIB: &str = include_str!("../fixture/src/lib.rs");

pub struct Fx {
    pub dir: PathBuf,
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}

impl Fx {
    /// A fresh, uncommitted copy of the fixture with the given area list.
    pub fn new(name: &str, area: &str) -> Fx {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = fs::remove_dir_all(&dir);
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture"),
            &dir,
        );
        fs::rename(dir.join("gitignore"), dir.join(".gitignore")).unwrap();
        let fx = Fx { dir };
        fx.write("mutants/fx.toml", area);
        fx.git(&["init", "-q", "-b", "main"]);
        fx
    }

    /// `new` plus a first commit.
    pub fn committed(name: &str, area: &str) -> Fx {
        let fx = Fx::new(name, area);
        fx.commit("fixture");
        fx
    }

    pub fn write(&self, rel: &str, text: &str) {
        let p = self.dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.dir.join(rel)).unwrap_or_default()
    }

    pub fn commit(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mae-mutate"));
        c.arg("--root")
            .arg(&self.dir)
            .args(args)
            .current_dir(&self.dir)
            .env("MUTATE_QUEUE", "off")
            .env_remove("MUTATE_PREFIX")
            .env("CARGO_TARGET_DIR", self.dir.join("target"))
            // Pinned, not inherited: CI sets CARGO_TERM_COLOR=always, and a
            // test's outcome must not depend on who runs it. The coloured
            // case has its own test.
            .env("CARGO_TERM_COLOR", "never")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        c
    }

    pub fn mutate(&self, args: &[&str]) -> Out {
        Out(self.command(args).output().unwrap())
    }

    /// Lines of `calls.log`: "<MUTATE_ID> <build|test>" per runner call.
    pub fn calls(&self) -> Vec<String> {
        self.read("calls.log").lines().map(str::to_string).collect()
    }

    pub fn state(&self, rel: &str) -> PathBuf {
        self.dir.join(".git/mae-mutate").join(rel)
    }
}

pub struct Out(pub Output);

impl Out {
    pub fn code(&self) -> Option<i32> {
        self.0.status.code()
    }
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.0.stdout).into_owned()
    }
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.0.stderr).into_owned()
    }
    pub fn all(&self) -> String {
        format!("{}\n{}", self.stdout(), self.stderr())
    }
}

/// One `[[mutant]]` with one edit to `src/lib.rs`.
pub fn mutant(id: &str, find: &str, replace: &str, filter: &str) -> String {
    mutant_in(id, "src/lib.rs", find, replace, filter)
}

pub fn mutant_in(id: &str, file: &str, find: &str, replace: &str, filter: &str) -> String {
    format!(
        "[[mutant]]\nid = \"{id}\"\nrunner = \"fx\"\nfilter = \"{filter}\"\n\
         [[mutant.edit]]\nfile = \"{file}\"\nfind = '''{find}'''\nreplace = '''{replace}'''\n\n"
    )
}

/// Caught: `one()` returns 2 and `one_is_one` fails.
pub fn caught_a() -> String {
    mutant("a", "{ 1 }", "{ 2 }", "one_is_one")
}

/// Same length as `a`, same file, and no test looks at `five()`: survives.
pub fn survives_b() -> String {
    mutant("b", "{ 5 }", "{ 6 }", "one_is_one")
}
