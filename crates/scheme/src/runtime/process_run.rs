//! `(process-run ARGV ENV TIMEOUT-MS [CWD])`: run one external program and
//! report exactly what happened. Built for evidence-gathering probes, so it
//! takes nothing from the editor's ambient state:
//!
//! - **No shell.** ARGV is a list of strings, and its first element is an
//!   absolute path: no PATH lookup, no word splitting, no expansion.
//! - **Nothing inherited.** The environment is exactly ENV; the working
//!   directory is CWD (default `/`); stdin is empty. On Unix the child starts
//!   a session of its own (`setsid`: no controlling terminal), and every
//!   descriptor above stderr is closed at exec, including any the editor's
//!   own libraries opened without close-on-exec.
//! - **A time limit that holds.** The child's session is its own process
//!   group, and at the deadline the whole group is killed, so a grandchild
//!   cannot keep running past it. However the program ends, the group is
//!   killed before the call returns. A process that deliberately leaves the
//!   group (`setsid`, `setpgid`) is beyond its reach: if it holds the output
//!   pipes the call stops waiting for them after a grace and reports
//!   `output-abandoned`, with whatever output was read by then.
//! - **Exact output.** stdout and stderr are kept apart, as bytevectors (a
//!   probe's evidence is hashed, so it must not pass through lossy UTF-8),
//!   each capped at [`MAX_OUTPUT_BYTES`] with the excess drained, not
//!   blocked on, and reported as truncated.
//!
//! It blocks the calling thread until the program ends or the deadline
//! passes, as `shell-command` does, which [`MAX_TIMEOUT_MS`] bounds, plus the
//! output grace (two streams, [`DRAIN_GRACE`] each). Two cases it cannot
//! bound: a program stuck in uninterruptible sleep (a hung NFS read) delays
//! the reap until the kernel lets it die; and an editor that has set SIGCHLD
//! to be ignored makes `waitid` fail, reported as `spawn-failed` ("wait").
//!
//! @ai-caution: [security] Shell tier, like `shell-command` (the owner's
//! decision, 2026-11-28): an agent granted the shell tier can call it.
//! Callers that must hold guarantees against their caller (a probe wrapper's
//! allowlist, its session folder, its budget) enforce them themselves.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The most of each stream kept: 1 MiB, as `shell-command` keeps.
pub const MAX_OUTPUT_BYTES: usize = 1 << 20;
/// The longest a call may block: ten minutes.
pub const MAX_TIMEOUT_MS: u64 = 600_000;
/// After the deadline's kill, how long to wait for the output pipes to close
/// before giving up on them (a process that left the group can hold them).
const DRAIN_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(5);

/// One run, as validated from the Scheme arguments.
#[derive(Debug, Clone)]
pub struct Spec {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
    pub cwd: PathBuf,
}

/// How the run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    Exited(i32),
    Signalled(i32),
    TimedOut,
    /// The program could not be started (not found, not executable, …).
    SpawnFailed(String),
}

/// What the run produced.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub ending: Ending,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// The output may be incomplete: a pipe stayed open past the grace after
    /// the deadline's kill (a process outside the group held it).
    pub output_abandoned: bool,
    pub elapsed: Duration,
}

/// Why the arguments are refused.
pub fn check(spec: &Spec) -> Result<(), String> {
    let Some(program) = spec.argv.first() else {
        return Err("ARGV is empty".into());
    };
    if !Path::new(program).is_absolute() {
        return Err(format!(
            "the program must be an absolute path (no PATH lookup): {program:?}"
        ));
    }
    if let Some(a) = spec.argv.iter().find(|a| a.contains('\0')) {
        return Err(format!("an argument holds NUL: {a:?}"));
    }
    for (name, value) in &spec.env {
        if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
            return Err(format!("not an environment entry: {name:?}"));
        }
    }
    let ms = spec.timeout.as_millis();
    if ms == 0 || ms > u128::from(MAX_TIMEOUT_MS) {
        return Err(format!(
            "TIMEOUT-MS must be 1..={MAX_TIMEOUT_MS} (got {ms})"
        ));
    }
    if !spec.cwd.is_absolute() || !spec.cwd.is_dir() {
        return Err(format!(
            "CWD must be an existing absolute directory: {}",
            spec.cwd.display()
        ));
    }
    Ok(())
}

/// One stream's output so far: (bytes kept, truncated), shared with its
/// reader so that what was read survives an abandoned pipe.
type Kept = std::sync::Arc<std::sync::Mutex<(Vec<u8>, bool)>>;

/// Read `r` to its end into `kept`, keeping at most [`MAX_OUTPUT_BYTES`]
/// and draining the rest.
fn drain(mut r: impl Read, kept: &Kept) {
    let mut buf = [0u8; 8192];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut k = kept.lock().unwrap_or_else(|e| e.into_inner());
                let room = MAX_OUTPUT_BYTES - k.0.len();
                k.0.extend_from_slice(&buf[..n.min(room)]);
                k.1 |= n > room;
            }
        }
    }
}

/// Every descriptor from 3 up closed at exec. Async-signal-safe (it runs
/// between fork and exec). Marked close-on-exec rather than closed, so std's
/// own exec-error pipe still reports a failed exec.
#[cfg(unix)]
fn cloexec_from_3() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: close_range(2) with CLOSE_RANGE_CLOEXEC only sets flags.
        let r = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            )
        };
        if r == 0 {
            return;
        }
    }
    // Without close_range (macOS, an old or seccomp'd kernel): mark each fd
    // up to the open-files limit, capped at 65536. A process whose limit is
    // raised past the cap could leak a descriptor numbered above it.
    // SAFETY: sysconf(3) is async-signal-safe and reads a limit.
    let max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    let max = if max <= 0 { 1024 } else { max.min(65_536) } as libc::c_int;
    for fd in 3..max {
        // SAFETY: fcntl(2) on a possibly-closed fd fails harmlessly (EBADF).
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

/// The child starts its own session (so its own process group, and no
/// controlling terminal), with every descriptor above stderr closed.
#[cfg(unix)]
fn own_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure runs in the forked child before exec and calls
    // only async-signal-safe functions (setsid, close_range/sysconf/fcntl).
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            cloexec_from_3();
            Ok(())
        });
    }
}
#[cfg(not(unix))]
fn own_group(_cmd: &mut Command) {}

/// Kill the child's whole process group (Unix), or the child (elsewhere).
fn kill_all(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // The child leads its session and group (`setsid`): its pid is the pgid.
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: kill(2) with a negative pid signals that process group;
            // no memory is involved.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
}

/// Has the child exited? Seen WITHOUT reaping it (Unix: `waitid` with
/// `WNOWAIT`), so its pid, and so its process group's id, cannot be reused
/// before [`kill_all`] signals the group.
#[cfg(unix)]
fn has_exited(child: &mut std::process::Child) -> std::io::Result<bool> {
    let pid = libc::id_t::from(child.id());
    // SAFETY: `info` is a plain C struct waitid(2) writes into; zeroed is a
    // valid initial value, and si_pid stays 0 when nothing has exited.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let flags = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
    // SAFETY: waitid(2) on our own child's pid, writing into `info`.
    if unsafe { libc::waitid(libc::P_PID, pid, &mut info, flags) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: reading the pid field of the siginfo waitid filled.
    Ok(unsafe { info.si_pid() } != 0)
}

/// Elsewhere there is no process group to protect: `try_wait` reaps, and
/// std keeps the status for the `wait` that follows.
#[cfg(not(unix))]
fn has_exited(child: &mut std::process::Child) -> std::io::Result<bool> {
    child.try_wait().map(|s| s.is_some())
}

fn ending_of(status: std::process::ExitStatus) -> Ending {
    if let Some(code) = status.code() {
        return Ending::Exited(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return Ending::Signalled(sig);
        }
    }
    Ending::Exited(-1)
}

/// Wait until the child ends or `deadline` passes, then kill its group and
/// reap it.
fn wait_for_end(child: &mut std::process::Child, deadline: Instant) -> Ending {
    // However it ends, nothing it started outlives the call: the group is
    // killed while the leader is still unreaped (so its id is still ours),
    // then the leader is reaped. Then the pipes close; one held by a process
    // that left the group is waited for only up to the grace.
    loop {
        let exited = match has_exited(child) {
            Ok(exited) => exited,
            Err(e) => {
                kill_all(child);
                let _ = child.wait();
                break Ending::SpawnFailed(format!("wait: {e}"));
            }
        };
        let timed_out = !exited && Instant::now() >= deadline;
        if !exited && !timed_out {
            std::thread::sleep(POLL);
            continue;
        }
        kill_all(child);
        let status = child.wait();
        break match (timed_out, status) {
            (true, _) => Ending::TimedOut,
            (false, Ok(s)) => ending_of(s),
            (false, Err(e)) => Ending::SpawnFailed(format!("wait: {e}")),
        };
    }
}

/// A thread reading `r` into a shared buffer, and the signal that it ended.
fn reader(r: impl Read + Send + 'static) -> (Kept, mpsc::Receiver<()>) {
    let kept: Kept = Default::default();
    let (done, ended) = mpsc::channel();
    let k = std::sync::Arc::clone(&kept);
    std::thread::spawn(move || {
        drain(r, &k);
        let _ = done.send(());
    });
    (kept, ended)
}

/// One stream's output: (bytes, truncated, abandoned). Its end is waited for
/// at most [`DRAIN_GRACE`]; past that, what was read so far is returned and
/// the pipe reported abandoned.
fn collect((kept, ended): (Kept, mpsc::Receiver<()>)) -> (Vec<u8>, bool, bool) {
    let abandoned = ended.recv_timeout(DRAIN_GRACE).is_err();
    let mut k = kept.lock().unwrap_or_else(|e| e.into_inner());
    (std::mem::take(&mut k.0), k.1, abandoned)
}

/// Run `spec` (already [`check`]ed) and report the outcome.
pub fn run(spec: &Spec) -> Outcome {
    let start = Instant::now();
    let mut cmd = Command::new(&spec.argv[0]);
    cmd.args(&spec.argv[1..])
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    own_group(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Outcome {
                ending: Ending::SpawnFailed(e.to_string()),
                stdout: Vec::new(),
                stderr: Vec::new(),
                stdout_truncated: false,
                stderr_truncated: false,
                output_abandoned: false,
                elapsed: start.elapsed(),
            }
        }
    };
    let out = reader(child.stdout.take().expect("piped"));
    let err = reader(child.stderr.take().expect("piped"));

    let ending = wait_for_end(&mut child, start + spec.timeout);
    let (stdout, stdout_truncated, a) = collect(out);
    let (stderr, stderr_truncated, b) = collect(err);
    Outcome {
        ending,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        output_abandoned: a || b,
        elapsed: start.elapsed(),
    }
}

// ---------------------------------------------------------------- Scheme

use crate::lisp_error::{Arity, LispError};
use crate::permission::tier;
use crate::value::Value;
use crate::vm::Vm;

/// A list of strings, each element checked: anything else is refused, never
/// skipped (a dropped argument would change what runs).
fn strings(v: &Value, what: &str) -> Result<Vec<String>, LispError> {
    v.to_vec()?
        .iter()
        .map(|e| e.as_str().map(str::to_string))
        .collect::<Result<_, _>>()
        .map_err(|_| LispError::internal(format!("process-run: {what} must be a list of strings")))
}

/// ENV: a list of (NAME . VALUE) string pairs.
fn env_pairs(v: &Value) -> Result<Vec<(String, String)>, LispError> {
    let refuse = || {
        LispError::internal("process-run: ENV must be a list of (NAME . VALUE) strings".to_string())
    };
    v.to_vec()?
        .iter()
        .map(|e| match e {
            Value::Pair(p) => Ok((
                p.0.as_str().map_err(|_| refuse())?.to_string(),
                p.1.as_str().map_err(|_| refuse())?.to_string(),
            )),
            _ => Err(refuse()),
        })
        .collect()
}

fn outcome_value(o: Outcome) -> Value {
    let entry = |k: &str, v: Value| Value::cons(Value::symbol(k), v);
    let int_or_false = |n: Option<i32>| n.map_or(Value::Bool(false), |n| Value::Int(i64::from(n)));
    let (status, exit, signal, error) = match o.ending {
        Ending::Exited(c) => ("exited", Some(c), None, None),
        Ending::Signalled(s) => ("signalled", None, Some(s), None),
        Ending::TimedOut => ("timed-out", None, None, None),
        Ending::SpawnFailed(e) => ("spawn-failed", None, None, Some(e)),
    };
    Value::list(vec![
        entry("status", Value::symbol(status)),
        entry("exit", int_or_false(exit)),
        entry("signal", int_or_false(signal)),
        entry("error", error.map_or(Value::Bool(false), Value::string)),
        entry("stdout", Value::bytevector(o.stdout)),
        entry("stderr", Value::bytevector(o.stderr)),
        entry("stdout-truncated", Value::Bool(o.stdout_truncated)),
        entry("stderr-truncated", Value::Bool(o.stderr_truncated)),
        entry("output-abandoned", Value::Bool(o.output_abandoned)),
        entry(
            "elapsed-ms",
            Value::Int(i64::try_from(o.elapsed.as_millis()).unwrap_or(i64::MAX)),
        ),
    ])
}

/// `process-run` itself: its arguments checked, then the run.
fn primitive(args: &[Value]) -> Result<Value, LispError> {
    let argv = strings(&args[0], "ARGV")?;
    let env = env_pairs(&args[1])?;
    let ms = u64::try_from(args[2].as_int()?)
        .map_err(|_| LispError::internal("process-run: TIMEOUT-MS must be positive".to_string()))?;
    let cwd = match args.get(3) {
        None => PathBuf::from("/"),
        Some(v) => PathBuf::from(v.as_str()?),
    };
    let spec = Spec {
        argv,
        env,
        timeout: Duration::from_millis(ms),
        cwd,
    };
    check(&spec).map_err(|e| LispError::internal(format!("process-run: {e}")))?;
    Ok(outcome_value(run(&spec)))
}

/// Register `process-run`.
pub(super) fn register_process_run_fn(vm: &mut Vm) {
    vm.register_fn(
        "process-run",
        "Run one program without a shell: (process-run ARGV ENV TIMEOUT-MS [CWD]). ARGV is a \
         list of strings whose first is an absolute path (no PATH lookup); ENV is the complete \
         environment, a list of (NAME . VALUE) strings (nothing is inherited); TIMEOUT-MS is \
         1..=600000; CWD defaults to /; stdin is empty. At the deadline the program's whole \
         process group is killed, and so is anything it left running when it ends. Returns an \
         alist: status (exited | signalled | timed-out | spawn-failed), exit, signal, error, \
         stdout and stderr as bytevectors (exact bytes, each capped at 1 MiB), \
         stdout-truncated, stderr-truncated, output-abandoned, elapsed-ms. Blocks until done.",
        Arity::Variadic(3),
        tier::SHELL,
        primitive,
    );
}

#[cfg(all(test, unix))]
#[path = "process_run_tests.rs"]
mod tests;
