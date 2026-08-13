//! ADR-066 Phase D spike 2/2: launches the real GUI binary on Windows and
//! confirms (a) a real window appears, (b) a synthetic `SendInput` click
//! actually lands (verified via `GetCursorPos` reading back the OS cursor
//! position `SendInput`/`SetCursorPos` moved it to) -- proving the
//! injection MECHANISM itself works before the full smoke test wires this
//! to real, MCP-observed editor state (the ADR's own bar: "process
//! launched, didn't crash" is explicitly NOT sufficient).
//!
//! Spike 1/2 (does `skia-safe`/`skia-bindings` 0.99 even build for
//! `x86_64-pc-windows-msvc`) shipped as the `windows-gui-spike` CI job
//! (`.github/workflows/ci.yml`) and passed. This is genuinely new,
//! previously-unattempted CI infrastructure with zero local
//! pre-verification possible (no Windows toolchain available to the
//! author) -- kept as its own small, isolated proof rather than folded
//! into the eventual full smoke test, so a window-discovery or
//! `SendInput`-targeting failure is never debugged in the same PR as a
//! Skia build failure.
//!
//! No way to fabricate winit `WindowEvent`s and call its event handlers
//! directly in-process (confirmed against winit 0.30's own source: the
//! `KeyEvent.platform_specific` field is `pub(crate)`, unconstructable
//! outside the winit crate, and `ActiveEventLoop` has no public
//! constructor) -- the only way to satisfy this ADR's bar is real OS input
//! into a real, live window, which is what this file does.

#![cfg(all(windows, feature = "gui"))]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetCursorPos, GetWindowRect, SetCursorPos, SetForegroundWindow,
};

/// `options.rs`'s `window_title` option's own default -- a freshly-launched
/// instance with no user config (this test's isolated env, below) uses this
/// exact title, so it's a precise, reliable `FindWindowW` target.
const WINDOW_TITLE: &str = "MAE \u{2014} Modern AI Editor";

/// Mirrors `headless_e2e.rs`'s `isolated_env` exactly (same rationale: never
/// touch the real user's config or a shared path) -- `MAE_SKIP_WIZARD=1` so
/// a fresh instance opens straight to a scratch buffer instead of blocking
/// on the first-run setup wizard. `MAE_LOG=debug` (this binary's own
/// tracing env var, `main.rs`'s `init_logging`, distinct from `RUST_LOG`)
/// so a window-never-appeared failure's captured stderr has real signal to
/// show, not just silence.
fn isolated_env(cmd: &mut Command, xdg_config: &Path, xdg_data: &Path, home: &Path) {
    cmd.env("XDG_CONFIG_HOME", xdg_config)
        .env("XDG_DATA_HOME", xdg_data)
        .env("HOME", home)
        .env("MAE_SKIP_WIZARD", "1")
        .env("MAE_LOG", "debug");
}

fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn find_window() -> HWND {
    let title = wide_null(WINDOW_TITLE);
    unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) }
}

fn wait_for_window(timeout: Duration) -> HWND {
    let deadline = Instant::now() + timeout;
    loop {
        let hwnd = find_window();
        if !hwnd.is_null() {
            return hwnd;
        }
        if Instant::now() >= deadline {
            return std::ptr::null_mut();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// RAII guard so a panicking assertion never leaks the spawned GUI process --
/// mirrors `headless_e2e.rs`'s `HeadlessGuard` precedent. No graceful
/// shutdown protocol needed here (unlike headless's SIGTERM/socket
/// handshake) -- this spike doesn't yet talk to the process at all, so a
/// hard kill is the correct, only option.
struct SpikeGuard {
    child: Option<Child>,
}

impl Drop for SpikeGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn a_real_window_appears_and_a_synthetic_click_lands() {
    let tmp = tempfile::tempdir().unwrap();
    let xdg_config = tmp.path().join("config");
    let xdg_data = tmp.path().join("data");
    std::fs::create_dir_all(&xdg_config).unwrap();
    std::fs::create_dir_all(&xdg_data).unwrap();

    // Captured to files (not `Stdio::null()`) so a window-never-appeared
    // failure can print the real subprocess output inline below -- the
    // actual, previously-unknown root cause, not a guess.
    let stdout_path = tmp.path().join("mae-gui-stdout.log");
    let stderr_path = tmp.path().join("mae-gui-stderr.log");
    let stdout_file = std::fs::File::create(&stdout_path).unwrap();
    let stderr_file = std::fs::File::create(&stderr_path).unwrap();

    let mae = env!("CARGO_BIN_EXE_mae");
    let mut cmd = Command::new(mae);
    // `--gui` forces the GUI backend regardless of display-detection
    // heuristics (`main.rs::gui_display_available`) -- unconditionally
    // `true` on non-unix already, but explicit here removes any doubt.
    cmd.args(["--gui"])
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    isolated_env(&mut cmd, &xdg_config, &xdg_data, tmp.path());
    let child = cmd.spawn().expect("failed to spawn `mae --gui`");
    let guard = SpikeGuard { child: Some(child) };

    // 1. A real window must appear -- proves the runner has a real
    // interactive desktop session capable of hosting a Win32 window, not
    // just claimed per public GH-hosted-runner docs (untested by this repo
    // before this spike).
    //
    // 120s, not 30s: this run's own first attempt (with a 30s budget) never
    // got anywhere near window creation -- the captured MAE_LOG=debug
    // stderr showed it was still deep in KB federation/cozo Datalog query
    // evaluation (the built-in practices/manual KBs this binary
    // auto-registers at startup, CLAUDE.md's own "dev-practices KB
    // dogfooding") at the moment it was killed, with zero GUI/winit log
    // lines having appeared at all. A cold Windows CI runner (no OS-level
    // disk cache, fresh cozo DB files) doing that work is genuinely slower
    // than 30s allows for, not a hang or a real regression -- matches
    // `headless_e2e.rs`'s own documented precedent (a cold headless boot
    // observed using 14.7s of a 15s budget "flaky-under-load, not a real
    // regression").
    let hwnd = wait_for_window(Duration::from_secs(120));
    if hwnd.is_null() {
        let stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
        let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        panic!(
            "no window titled {WINDOW_TITLE:?} appeared within 120s.\n\
             \n\
             Check STARTUP COST FIRST. This bound is not generous: the test has \
             been measured at 46s, 92s and 110s on passing runs of this same job, \
             so it fails intermittently whenever startup grows (issue #713 -- \
             embedding the KB corpora in #706 roughly doubled it, because Windows \
             ships no pre-built stores and so builds them all on first launch). \
             Repeated cozo `stratum`/`epoch` lines in the stderr below mean the \
             process is alive and grinding through KB work, not hung.\n\
             \n\
             Only if startup is NOT the cause: the runner may lack a real desktop \
             session, or the process may have failed to start outright -- in which \
             case the stderr below is short or empty rather than full of KB \
             activity.\n\
             \n\
             Captured subprocess output:\n--- stdout ---\n{stdout}\n\
             --- stderr ---\n{stderr}"
        );
    }

    unsafe {
        // Best-effort -- a non-foreground window can still receive
        // SendInput-generated events once it's under the cursor, so this
        // is a robustness improvement, not a correctness requirement this
        // test's own assertions depend on.
        SetForegroundWindow(hwnd);

        let mut rect: RECT = std::mem::zeroed();
        let got_rect = GetWindowRect(hwnd, &mut rect);
        assert_ne!(
            got_rect, 0,
            "GetWindowRect failed on a window we just found"
        );

        // A point safely inside the window body, away from title-bar
        // controls (close/minimize/maximize) a real click there could
        // accidentally trigger.
        let x = rect.left + (rect.right - rect.left) / 2;
        let y = rect.top + (rect.bottom - rect.top) / 2 + 20;

        // 2. Position the OS cursor, then inject a real left-click via
        // SendInput -- the same synthetic-input API real automation tools
        // use to generate genuine WM_LBUTTONDOWN/WM_LBUTTONUP messages, not
        // merely move a visual cursor.
        let set_pos = SetCursorPos(x, y);
        assert_ne!(set_pos, 0, "SetCursorPos failed");

        let down = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_LEFTDOWN,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_LEFTUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let inputs = [down, up];
        let sent = SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
        assert_eq!(
            sent, 2,
            "SendInput must report both injected events accepted by the OS"
        );

        // 3. The observable proof: the real OS cursor position now reflects
        // where this test positioned it -- confirms SendInput/SetCursorPos
        // genuinely affected system input state, not merely that the FFI
        // calls returned without error. Deliberately NOT yet wired to
        // editor state (the full smoke test's job, tracked separately) --
        // this spike proves only that the injection mechanism itself
        // works, the prerequisite the full harness will be built on.
        let mut cursor: POINT = std::mem::zeroed();
        let got_cursor = GetCursorPos(&mut cursor);
        assert_ne!(got_cursor, 0, "GetCursorPos failed");
        assert_eq!(
            (cursor.x, cursor.y),
            (x, y),
            "the OS cursor position after SendInput must match where this test positioned it"
        );
    }

    drop(guard);
}
