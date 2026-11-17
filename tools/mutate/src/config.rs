//! The declared lists: `mutants/runners.toml` and one `mutants/<area>.toml` per
//! area, plus the structural and anchor checks that `--check` runs.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const LIST_DIR: &str = "mutants";
pub const RUNNERS_FILE: &str = "runners.toml";

/// How one kind of test suite is built, run and judged. Defined once, named by
/// every mutant that uses it.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Runner {
    /// Working directory, relative to the repository root. A workspace whose
    /// `.cargo/config.toml` matters must be run from inside it.
    pub workspace: String,
    /// Builds the test binaries without running them. `{targets}` expands to
    /// the mutant's targets; `{filter}` to its filter.
    pub build: Vec<String>,
    /// Runs the tests that must fail. Must contain `{filter}`.
    pub test: Vec<String>,
    /// Default targets (e.g. `--lib`, `-p crate`), overridable per mutant.
    #[serde(default)]
    pub targets: Vec<String>,
    /// Output markers that mean "a test failed": the mutant is caught.
    pub caught: Vec<String>,
    /// Patterns with one `{n}` that count the tests run, summed over the
    /// output. The control run requires more than zero.
    pub count: Vec<String>,
    /// Output markers that mean "did not compile".
    #[serde(default = "default_unviable")]
    pub unviable: Vec<String>,
    /// Output marker proving the build recompiled something. A mutant build
    /// without it ran on a stale binary (an mtime that did not advance), so
    /// the tool bumps the mtimes and rebuilds once, and never judges it as is.
    /// Empty disables the check.
    #[serde(default = "default_rebuilt")]
    pub rebuilt: String,
    /// How many times a killed build or test (no marker, non-zero exit) is
    /// retried before it is reported as no result.
    #[serde(default = "default_retry")]
    pub retry_on_kill: u32,
    /// Queue class for the default command prefix (see `exec::prefix`).
    #[serde(default = "default_class")]
    pub queue_class: String,
}

fn default_unviable() -> Vec<String> {
    vec!["error[".into(), "could not compile".into()]
}
fn default_rebuilt() -> String {
    "Compiling ".into()
}
fn default_retry() -> u32 {
    1
}
fn default_class() -> String {
    "rust".into()
}

/// One exact text replacement in one file.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub file: String,
    pub find: String,
    pub replace: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Mutant {
    /// Unique and stable: the `--only` key and the results key.
    pub id: String,
    pub runner: String,
    /// The tests that must fail when this mutant is applied.
    pub filter: String,
    /// Overrides the runner's default targets.
    #[serde(default)]
    pub targets: Option<Vec<String>>,
    /// The decision or issue the guarded mechanism comes from.
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    /// Non-empty: why this mutant cannot be caught. It is listed, not run.
    #[serde(default)]
    pub equivalent: Option<String>,
    /// Applied in order; several files are allowed.
    #[serde(rename = "edit")]
    pub edits: Vec<Edit>,
    /// The `<area>` of the file it was read from.
    #[serde(skip)]
    pub area: String,
}

impl Mutant {
    pub fn is_equivalent(&self) -> bool {
        self.equivalent
            .as_deref()
            .is_some_and(|r| !r.trim().is_empty())
    }

    pub fn targets<'a>(&'a self, runner: &'a Runner) -> &'a [String] {
        self.targets.as_deref().unwrap_or(&runner.targets)
    }

    pub fn files(&self) -> BTreeSet<String> {
        self.edits.iter().map(|e| e.file.clone()).collect()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnersFile {
    runner: BTreeMap<String, Runner>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AreaFile {
    #[serde(default)]
    mutant: Vec<Mutant>,
}

pub struct Lists {
    pub runners: BTreeMap<String, Runner>,
    /// Areas in file-name order, mutants in declaration order within each.
    pub mutants: Vec<Mutant>,
}

pub fn load(root: &Path) -> Result<Lists, String> {
    let dir = root.join(LIST_DIR);
    let runners_path = dir.join(RUNNERS_FILE);
    let text = read(&runners_path)?;
    let runners: RunnersFile =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", runners_path.display()))?;
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".toml") && n != RUNNERS_FILE)
        .collect();
    names.sort();
    let mut mutants = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let file: AreaFile =
            toml::from_str(&read(&path)?).map_err(|e| format!("{}: {e}", path.display()))?;
        let area = name.trim_end_matches(".toml").to_string();
        mutants.extend(file.mutant.into_iter().map(|mut m| {
            m.area = area.clone();
            m
        }));
    }
    Ok(Lists {
        runners: runners.runner,
        mutants,
    })
}

pub fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Structural problems anywhere in the lists. Any one refuses every command.
pub fn check_structure(root: &Path, lists: &Lists) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, r) in &lists.runners {
        if !root.join(&r.workspace).is_dir() {
            problems.push(format!(
                "runner {name}: workspace {} not found",
                r.workspace
            ));
        }
        if !r.test.iter().any(|a| a.contains("{filter}")) {
            problems.push(format!("runner {name}: `test` never uses {{filter}}"));
        }
        if r.build.is_empty() || r.caught.is_empty() || r.count.is_empty() {
            problems.push(format!(
                "runner {name}: build, caught and count must be non-empty"
            ));
        }
        if r.count.iter().any(|p| !p.contains("{n}")) {
            problems.push(format!("runner {name}: every count pattern needs {{n}}"));
        }
    }
    let mut seen = BTreeSet::new();
    for m in &lists.mutants {
        if m.id.trim().is_empty() || m.id.contains(char::is_whitespace) {
            problems.push(format!(
                "{}: id {:?} is empty or has whitespace",
                m.area, m.id
            ));
        }
        if !seen.insert(m.id.as_str()) {
            problems.push(format!("{}: duplicate id {}", m.area, m.id));
        }
        if !lists.runners.contains_key(&m.runner) {
            problems.push(format!("{}: unknown runner {:?}", m.id, m.runner));
        }
        if m.filter.trim().is_empty() {
            problems.push(format!("{}: empty filter", m.id));
        }
        if m.edits.is_empty() {
            problems.push(format!("{}: no [[mutant.edit]]", m.id));
        }
        for e in &m.edits {
            if e.find.is_empty() || e.find == e.replace {
                problems.push(format!("{}: {}: empty or no-op edit", m.id, e.file));
            }
        }
    }
    problems
}

/// The files a mutant changes, as (path, original, mutated). Every `find` must
/// occur exactly once in the file as the earlier edits of the same mutant left
/// it; anything else is refused, never guessed at.
pub fn apply(root: &Path, m: &Mutant) -> Result<Vec<(String, String, String)>, String> {
    let mut files: Vec<(String, String, String)> = Vec::new();
    for e in &m.edits {
        let idx = match files.iter().position(|f| f.0 == e.file) {
            Some(i) => i,
            None => {
                let text = read(&root.join(&e.file)).map_err(|err| format!("{}: {err}", m.id))?;
                files.push((e.file.clone(), text.clone(), text));
                files.len() - 1
            }
        };
        let current = &files[idx].2;
        let n = current.matches(e.find.as_str()).count();
        if n != 1 {
            return Err(format!("{}: anchor found {n} times in {}", m.id, e.file));
        }
        files[idx].2 = current.replacen(e.find.as_str(), &e.replace, 1);
    }
    Ok(files)
}

/// Anchor problems for the given mutants (equivalent ones included: their
/// anchors still document real code).
pub fn check_anchors(root: &Path, mutants: &[&Mutant]) -> Vec<String> {
    mutants
        .iter()
        .filter_map(|m| apply(root, m).err())
        .collect()
}
