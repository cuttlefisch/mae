//! The results file: one line per verdict, appended and synced the moment it
//! is known, so a killed run loses at most the mutant it was on. The latest
//! line for an id wins. Each line carries the fingerprint of the code and
//! definitions it was reached on; a line whose fingerprint no longer matches
//! is stale and its mutant runs again.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CAUGHT: &str = "caught";
pub const SURVIVED: &str = "SURVIVED";
pub const UNVIABLE: &str = "unviable";
pub const NO_RESULT: &str = "no-result";
pub const CONTROL_OK: &str = "control-ok";

#[derive(Clone)]
pub struct Record {
    pub verdict: String,
    pub id: String,
    pub build: f64,
    pub test: f64,
    pub fingerprint: String,
}

pub struct Results {
    path: PathBuf,
    latest: BTreeMap<String, Record>,
}

impl Results {
    pub fn open(state: &Path, fresh: bool) -> Result<Results, String> {
        let path = state.join("results.tsv");
        if fresh && path.exists() {
            fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let text = fs::read_to_string(&path).unwrap_or_default();
        let mut latest = BTreeMap::new();
        for line in text.lines() {
            if let Some(r) = parse(line) {
                latest.insert(r.id.clone(), r);
            }
        }
        Ok(Results { path, latest })
    }

    /// The latest verdict for `id`, if it was reached on `fingerprint`.
    pub fn current(&self, id: &str, fingerprint: &str) -> Option<&Record> {
        self.latest.get(id).filter(|r| r.fingerprint == fingerprint)
    }

    pub fn append(&mut self, r: Record) -> Result<(), String> {
        let line = format!(
            "{}\t{}\t{:.1}\t{:.1}\t{}\n",
            r.verdict, r.id, r.build, r.test, r.fingerprint
        );
        let err = |e: std::io::Error| format!("{}: {e}", self.path.display());
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(err)?;
        f.write_all(line.as_bytes())
            .and_then(|_| f.sync_data())
            .map_err(err)?;
        self.latest.insert(r.id.clone(), r);
        Ok(())
    }
}

fn parse(line: &str) -> Option<Record> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() != 5 {
        return None;
    }
    Some(Record {
        verdict: f[0].to_string(),
        id: f[1].to_string(),
        build: f[2].parse().ok()?,
        test: f[3].parse().ok()?,
        fingerprint: f[4].to_string(),
    })
}
