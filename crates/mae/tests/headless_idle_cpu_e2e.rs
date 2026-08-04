//! Real subprocess e2e for L3b of the epic's issue-closure pass (#380,
//! ADR-055's idle-CPU DoD item): a `mae --headless` instance with no active
//! MCP session must show bounded, near-zero CPU usage, not periodic
//! full-tilt polling from tick/animation logic written assuming a display
//! is attached (`Editor::on_idle_tick`, `crates/core/src/editor/idle_ops.rs`,
//! or any GUI-only animation scheduling that isn't actually inert when no
//! `Renderer` exists).
//!
//! Samples the real process's `/proc/{pid}/stat` utime+stime twice across a
//! short idle window (no MCP session ever connects) and computes CPU% from
//! the delta. Linux-only (matches `headless_e2e.rs`'s own
//! `#![cfg(target_os = "linux")]` precedent -- `/proc` is Linux-specific).

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

mod headless_test_support;
use headless_test_support::spawn_isolated_headless;

/// `(utime, stime)` in clock ticks, read from `/proc/{pid}/stat`. Parses
/// past the `comm` field (which can itself contain spaces/parens) by
/// splitting on the LAST `)` in the line, per `proc(5)`.
fn read_cpu_ticks(pid: u32) -> (u64, u64) {
    let content = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .unwrap_or_else(|e| panic!("failed to read /proc/{pid}/stat: {e}"));
    let after_comm = content
        .rsplit_once(')')
        .unwrap_or_else(|| panic!("unexpected /proc/{pid}/stat format: {content}"))
        .1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // fields[0] is `state` (proc(5) field 3); utime is field 14 (index 11
    // here), stime is field 15 (index 12).
    let utime: u64 = fields[11].parse().expect("utime not a number");
    let stime: u64 = fields[12].parse().expect("stime not a number");
    (utime, stime)
}

#[tokio::test]
async fn idle_headless_instance_uses_bounded_near_zero_cpu() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    std::fs::create_dir_all(project_root.join(".git")).unwrap();
    let xdg_config = tmp.path().join("config");
    let xdg_data = tmp.path().join("data");
    std::fs::create_dir_all(&xdg_config).unwrap();
    std::fs::create_dir_all(&xdg_data).unwrap();

    let (_socket_path, mut guard) =
        spawn_isolated_headless(&project_root, &xdg_config, &xdg_data, tmp.path(), None);
    let pid = guard.pid();

    // Let post-boot initialization settle (LSP/KB federation background
    // work, any startup-only bursts) before starting the idle-CPU sample --
    // this test measures STEADY-STATE idle, not boot cost.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    assert!(clk_tck > 0, "sysconf(_SC_CLK_TCK) returned {clk_tck}");

    let (utime_start, stime_start) = read_cpu_ticks(pid);
    let sample_start = Instant::now();

    // No MCP session ever connects during this window -- exactly the
    // "no active MCP session" scenario the DoD item describes.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let (utime_end, stime_end) = read_cpu_ticks(pid);
    let elapsed_wall = sample_start.elapsed().as_secs_f64();

    let cpu_ticks_used = (utime_end - utime_start) + (stime_end - stime_start);
    let cpu_seconds_used = cpu_ticks_used as f64 / clk_tck as f64;
    let cpu_percent = (cpu_seconds_used / elapsed_wall) * 100.0;

    guard.shutdown(Duration::from_secs(10));

    // Generous threshold (real busy-loop regressions burn 90-100% of a
    // core continuously; anything under ~5% average over a 5s idle window
    // rules that class of bug out) -- wide enough to avoid CI flakiness on
    // a loaded machine, tight enough to catch a genuine regression in
    // `Editor::on_idle_tick`/GUI-only animation scheduling firing with no
    // `Renderer` attached.
    assert!(
        cpu_percent < 5.0,
        "idle headless instance used {cpu_percent:.2}% CPU over {elapsed_wall:.1}s \
         ({cpu_ticks_used} ticks @ {clk_tck} ticks/sec) -- expected bounded, near-zero \
         usage with no active MCP session"
    );
}
