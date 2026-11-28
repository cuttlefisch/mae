//! `process-run`'s guarantees, one test each, each written to fail when the
//! guarantee is dropped (see `mutants/process-run.toml`).

use super::*;

const SH: &str = "/bin/sh";

fn spec(argv: &[&str], env: &[(&str, &str)], ms: u64) -> Spec {
    Spec {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        timeout: Duration::from_millis(ms),
        cwd: PathBuf::from("/"),
    }
}

fn sh(script: &str, ms: u64) -> Outcome {
    let s = spec(&[SH, "-c", script], &[("PATH", "/bin:/usr/bin")], ms);
    check(&s).unwrap();
    run(&s)
}

/// A per-test scratch directory (never a shared or address-derived path).
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mae-process-run-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Is `pid` gone (or a zombie awaiting its reaper)? Waits up to 3 s.
fn gone(pid: i32) -> bool {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        // SAFETY: signal 0 only checks that the pid exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| s.split_whitespace().nth(2) == Some("Z"))
            .unwrap_or(false);
        if !alive || zombie {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pid_in(file: &Path) -> i32 {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(p) = std::fs::read_to_string(file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return p;
        }
        assert!(
            Instant::now() < until,
            "no pid written to {}",
            file.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// No shell: metacharacters in an argument reach the program as text.
#[test]
fn arguments_are_never_interpreted_by_a_shell() {
    let arg = "$HOME; touch /tmp/never `id` *";
    let s = spec(&["/bin/echo", arg], &[], 2_000);
    let o = run(&s);
    assert_eq!(o.ending, Ending::Exited(0));
    assert_eq!(o.stdout, format!("{arg}\n").into_bytes());
}

/// The environment is exactly ENV: nothing of the editor's is inherited.
#[test]
fn the_environment_is_exactly_the_one_given() {
    let s = spec(&["/usr/bin/env"], &[("ONLY", "this")], 2_000);
    let o = run(&s);
    assert_eq!(o.ending, Ending::Exited(0));
    assert_eq!(String::from_utf8(o.stdout).unwrap(), "ONLY=this\n");
}

/// stdout and stderr are kept apart, with the exit code and the signal.
#[test]
fn streams_exit_code_and_signal_are_reported_apart() {
    let o = sh("echo out; echo err >&2; exit 7", 2_000);
    assert_eq!(o.ending, Ending::Exited(7));
    assert_eq!(o.stdout, b"out\n");
    assert_eq!(o.stderr, b"err\n");
    let o = sh("kill -TERM $$", 2_000);
    assert_eq!(o.ending, Ending::Signalled(libc::SIGTERM));
}

/// At the deadline the whole group dies, a grandchild included, well before
/// it would have ended by itself.
#[test]
fn the_deadline_kills_the_whole_process_group() {
    let d = scratch("deadline");
    let f = d.join("pid");
    // A deadline long enough that the script has always written the pid
    // before it (load cannot make 3 s expire first), and far short of the
    // 30 s the grandchild would run.
    let o = sh(
        &format!("sleep 30 & echo $! > {}; wait", f.display()),
        3_000,
    );
    assert_eq!(o.ending, Ending::TimedOut);
    assert!(o.elapsed < Duration::from_secs(10), "{:?}", o.elapsed);
    assert!(gone(pid_in(&f)), "the grandchild survived the deadline");
    let _ = std::fs::remove_dir_all(&d);
}

/// A program that ends normally leaves nothing running behind it.
#[test]
fn nothing_outlives_a_normal_exit() {
    let d = scratch("exit");
    let f = d.join("pid");
    let o = sh(&format!("sleep 30 & echo $! > {}", f.display()), 5_000);
    assert_eq!(o.ending, Ending::Exited(0));
    assert!(gone(pid_in(&f)), "a background process outlived the call");
    assert!(!o.output_abandoned);
    let _ = std::fs::remove_dir_all(&d);
}

/// Output past the cap is drained, not blocked on, and reported.
#[test]
fn output_past_the_cap_is_drained_and_reported_truncated() {
    let o = sh("head -c 3000000 /dev/zero", 10_000);
    assert_eq!(o.ending, Ending::Exited(0), "the writer was never blocked");
    assert_eq!(o.stdout.len(), MAX_OUTPUT_BYTES);
    assert!(o.stdout_truncated);
    assert!(!o.stderr_truncated);
}

/// Bytes are kept exactly, invalid UTF-8 included.
#[test]
fn output_bytes_are_exact() {
    let o = sh(r"printf '\377\000\303'", 2_000);
    assert_eq!(o.stdout, [0xff, 0x00, 0xc3]);
}

/// A program that cannot start is a result, not a crash.
#[test]
fn a_missing_program_is_spawn_failed() {
    let o = run(&spec(&["/nonexistent/probe"], &[], 1_000));
    assert!(matches!(o.ending, Ending::SpawnFailed(_)), "{:?}", o.ending);
}

/// The arguments that would let a call escape its rules are refused.
#[test]
fn arguments_outside_the_rules_are_refused() {
    let refused = |s: Spec| check(&s).unwrap_err();
    assert!(refused(spec(&[], &[], 1)).contains("empty"));
    assert!(refused(spec(&["sh", "-c", "x"], &[], 1)).contains("absolute"));
    assert!(refused(spec(&["/bin/echo", "a\0b"], &[], 1)).contains("NUL"));
    for name in ["", "A=B", "A\0"] {
        assert!(refused(spec(&["/bin/echo"], &[(name, "v")], 1)).contains("environment"));
    }
    assert!(refused(spec(&["/bin/echo"], &[], 0)).contains("TIMEOUT"));
    assert!(refused(spec(&["/bin/echo"], &[], MAX_TIMEOUT_MS + 1)).contains("TIMEOUT"));
    assert!(check(&spec(&["/bin/echo"], &[], MAX_TIMEOUT_MS)).is_ok());
    let mut s = spec(&["/bin/echo"], &[], 1);
    s.cwd = PathBuf::from("relative");
    assert!(refused(s).contains("CWD"));
}

/// Through Scheme: the alist, bytevectors and refusals as documented.
#[test]
fn the_scheme_primitive_returns_the_documented_alist() {
    let mut rt = crate::runtime::SchemeRuntime::new().unwrap();
    let out = rt
        .eval(r#"(cdr (assq 'stdout (process-run '("/bin/echo" "hi") '() 2000)))"#)
        .unwrap();
    assert_eq!(out, "#u8(104 105 10)");
    let status = rt
        .eval(r#"(cdr (assq 'status (process-run '("/bin/sh" "-c" "exit 3") '() 2000)))"#)
        .unwrap();
    assert_eq!(status, "exited");
    // An element that is not a string is refused, never silently dropped.
    assert!(rt
        .eval(r#"(process-run (list "/bin/echo" 42) '() 2000)"#)
        .is_err());
    assert!(rt
        .eval(r#"(process-run '("/bin/echo") '(("A" . 1)) 2000)"#)
        .is_err());
}

/// stdin is empty: a program that reads it ends at once.
#[test]
fn stdin_is_empty() {
    let o = run(&spec(&["/bin/cat"], &[], 5_000));
    assert_eq!(o.ending, Ending::Exited(0));
    assert!(o.stdout.is_empty());
}

/// The working directory is `/` by default, else the one given.
#[test]
fn the_working_directory_is_explicit() {
    let o = run(&spec(&["/bin/pwd"], &[], 2_000));
    assert_eq!(o.stdout, b"/\n");
    let d = scratch("cwd").canonicalize().unwrap();
    let mut s = spec(&["/bin/pwd"], &[], 2_000);
    s.cwd = d.clone();
    let o = run(&s);
    assert_eq!(o.stdout, format!("{}\n", d.display()).into_bytes());
    let _ = std::fs::remove_dir_all(&d);
}

/// A program that stops itself is still killed at the deadline.
#[test]
fn a_stopped_program_is_killed_at_the_deadline() {
    let o = sh("kill -STOP $$", 500);
    assert_eq!(o.ending, Ending::TimedOut);
    assert!(o.elapsed < Duration::from_secs(10), "{:?}", o.elapsed);
}

/// The child leads a session of its own: no controlling terminal shared
/// with the editor. Its process group is its own on every Unix (`pgid` is a
/// keyword of both procps and BSD ps); on Linux its session id is checked too,
/// from /proc (BSD ps has no session-id keyword).
#[test]
fn the_child_leads_its_own_session() {
    let o = sh(
        r#"test "$(ps -o pgid= -p $$ | tr -d ' ')" = "$$" && echo own-group"#,
        5_000,
    );
    assert_eq!(
        o.stdout,
        b"own-group\n",
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    if Path::new("/proc/self/stat").exists() {
        // Field 6 of /proc/PID/stat is the session id.
        let o = sh(
            r#"test "$(cut -d' ' -f6 /proc/$$/stat)" = "$$" && echo own-session"#,
            5_000,
        );
        assert_eq!(
            o.stdout,
            b"own-session\n",
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

/// A descriptor the editor opened without close-on-exec does not reach the
/// child.
#[test]
fn no_descriptor_of_the_editor_reaches_the_child() {
    let path = std::ffi::CString::new("/dev/null").unwrap();
    // SAFETY: open(2) without O_CLOEXEC, deliberately: the leak under test.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
    assert!(fd > 2);
    let o = sh(
        &format!("test -e /dev/fd/{fd} && echo leaked || echo closed"),
        5_000,
    );
    // SAFETY: closing the fd opened above.
    unsafe { libc::close(fd) };
    assert_eq!(o.stdout, b"closed\n");
}

/// A process that leaves the group and holds the pipes: the call returns
/// after the grace, reports the output abandoned, and keeps what was read.
#[test]
fn an_escapee_holding_the_pipes_is_abandoned_with_its_output_kept() {
    if !Path::new("/usr/bin/setsid").exists() {
        return; // no setsid(1) here (macOS): nothing to escape with
    }
    let d = scratch("escapee");
    let f = d.join("pid");
    let o = sh(
        // The parent waits until the escapee has left the group (it writes
        // its pid only after setsid), or the group kill would catch it first.
        &format!(
            "echo before; /usr/bin/setsid sh -c 'echo $$ > {f}; exec sleep 30' & \
             while [ ! -s {f} ]; do sleep 0.05; done",
            f = f.display()
        ),
        20_000,
    );
    let escapee = pid_in(&f);
    // SAFETY: killing the escapee this test started.
    unsafe { libc::kill(escapee, libc::SIGKILL) };
    assert_eq!(o.ending, Ending::Exited(0));
    assert!(o.output_abandoned);
    assert_eq!(
        o.stdout, b"before\n",
        "the output read before the grace is kept"
    );
    assert!(o.elapsed < Duration::from_secs(10), "{:?}", o.elapsed);
    let _ = std::fs::remove_dir_all(&d);
}
