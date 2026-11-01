//! Bounded Emacs: `conformance/leak-export.el` over a directory of
//! documents, in a `systemd-run --user --scope` with `MemoryMax=3G` and no
//! swap where systemd is there, else under `ulimit -v`, and always under
//! `timeout` (≤ 120 s) with a process-group kill as the backstop. HOME is a
//! scratch directory under `target/`, so `~/.emacs.d` is never read or
//! written. A document that makes Emacs hang or run out of memory is
//! found (the first with no result), recorded, and skipped; the rest of its
//! chunk is exported again.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Emacs's answer for one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmacsOut {
    Html(String),
    Error(String),
    /// Emacs did not finish it: `timeout`, `killed`, `oom` or `crash`.
    Hang(&'static str),
    /// Emacs exited cleanly without a result (should not happen).
    Missing,
}

/// The memory cap where there is no systemd (`ulimit -v`, KiB).
const VMEM_KB: u64 = 3 * 1024 * 1024;

const SCRIPT: &str = r#"home=$1; t=$2; kb=$3; scoped=$4; shift 4
if [ "$scoped" = 1 ]; then
  exec systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 --setenv=HOME="$home" timeout "$t" emacs "$@"
fi
ulimit -v "$kb" 2>/dev/null || echo 'leak oracle: no ulimit -v here; Emacs has no memory cap' >&2
if command -v timeout >/dev/null 2>&1; then exec timeout "$t" emacs "$@"; fi
if command -v gtimeout >/dev/null 2>&1; then exec gtimeout "$t" emacs "$@"; fi
exec emacs "$@""#;

static RUNS: AtomicUsize = AtomicUsize::new(0);

/// A configured runner.
#[derive(Debug, Clone)]
pub struct Emacs {
    root: PathBuf,
    home: PathBuf,
    el: PathBuf,
    timeout: u64,
    scoped: bool,
}

/// How one Emacs process ended.
enum Exit {
    Ok,
    Bad(&'static str),
}

impl Emacs {
    /// A runner whose scratch files go under `root` (a `target/` dir).
    pub fn new(root: &Path, el: &Path, timeout: u64) -> Emacs {
        let home = root.join("emacs-home");
        std::fs::create_dir_all(&home).expect("scratch HOME");
        let scoped = std::env::var_os("LEAK_ORACLE_NO_SCOPE").is_none() && systemd_scope_works();
        Emacs {
            root: root.to_path_buf(),
            home,
            el: el.to_path_buf(),
            timeout: timeout.clamp(5, 120),
            scoped,
        }
    }

    pub fn scoped(&self) -> bool {
        self.scoped
    }

    /// Emacs's export of every `(name, text)`, in order.
    pub fn export(&self, docs: &[&str]) -> Vec<EmacsOut> {
        let mut out = vec![EmacsOut::Missing; docs.len()];
        let mut pending: Vec<usize> = (0..docs.len()).collect();
        while !pending.is_empty() {
            let run = self.root.join(format!(
                "run-{}-{}",
                std::process::id(),
                RUNS.fetch_add(1, Ordering::Relaxed)
            ));
            let (inp, res) = (run.join("in"), run.join("out"));
            std::fs::create_dir_all(&inp).expect("in dir");
            std::fs::create_dir_all(&res).expect("out dir");
            for (k, &i) in pending.iter().enumerate() {
                std::fs::write(inp.join(format!("d{k:06}.org")), docs[i]).expect("write doc");
            }
            let exit = self.run(&inp, &res, &run.join("emacs.log"));
            pending = self.collect(&res, &pending, exit, &mut out);
            let _ = std::fs::remove_dir_all(&run);
        }
        out
    }

    /// Reads the results; returns the documents to export again.
    fn collect(
        &self,
        res: &Path,
        pending: &[usize],
        exit: Exit,
        out: &mut [EmacsOut],
    ) -> Vec<usize> {
        let mut again = Vec::new();
        let mut culprit_found = false;
        for (k, &i) in pending.iter().enumerate() {
            let base = res.join(format!("d{k:06}"));
            if let Ok(h) = std::fs::read_to_string(base.with_extension("html")) {
                out[i] = EmacsOut::Html(h);
            } else if let Ok(e) = std::fs::read_to_string(base.with_extension("error")) {
                out[i] = EmacsOut::Error(e);
            } else if let Exit::Bad(why) = exit {
                if culprit_found {
                    again.push(i);
                } else {
                    out[i] = EmacsOut::Hang(why);
                    culprit_found = true;
                }
            }
        }
        again
    }

    fn run(&self, inp: &Path, res: &Path, log: &Path) -> Exit {
        let log_file = std::fs::File::create(log).expect("emacs log");
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(SCRIPT)
            .arg("sh")
            .arg(&self.home)
            .arg(self.timeout.to_string())
            .arg(VMEM_KB.to_string())
            .arg(if self.scoped { "1" } else { "0" })
            .args(["-Q", "--batch", "-l"])
            .arg(&self.el)
            .arg(inp)
            .arg(res)
            .env("HOME", &self.home)
            .stdin(Stdio::null())
            .stdout(log_file.try_clone().expect("log fd"))
            .stderr(log_file);
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        let mut child = cmd.spawn().expect("sh runs");
        let deadline = Instant::now() + Duration::from_secs(self.timeout + 15);
        loop {
            match child.try_wait().expect("wait for emacs") {
                Some(s) if s.success() => return Exit::Ok,
                Some(s) => return Exit::Bad(classify(s.code(), log)),
                None if Instant::now() >= deadline => {
                    kill_group(child.id());
                    let _ = child.wait();
                    return Exit::Bad("timeout");
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }
}

fn classify(code: Option<i32>, log: &Path) -> &'static str {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    match code {
        _ if text.contains("Memory exhausted") => "oom",
        Some(124) => "timeout",
        Some(137) | None => "killed",
        _ => "crash",
    }
}

fn kill_group(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stderr(Stdio::null())
        .status();
}

fn systemd_scope_works() -> bool {
    Command::new("systemd-run")
        .args(["--user", "--scope", "-q", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
