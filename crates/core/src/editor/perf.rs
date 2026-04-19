//! Performance statistics for debug mode.
//!
//! Tracks frame timing and process-level resource usage (RSS, CPU).
//! Used by `--debug` CLI flag and `SPC t D` toggle.

use sysinfo::{Pid, System};

/// Rolling performance statistics, updated each frame.
#[derive(Debug)]
pub struct PerfStats {
    /// Resident set size in bytes (from sysinfo).
    pub rss_bytes: u64,
    /// CPU usage percent (from sysinfo, 0.0–100.0+).
    pub cpu_percent: f32,
    /// Last frame duration in microseconds.
    pub frame_time_us: u64,
    /// Rolling average frame time (over last 60 frames) in microseconds.
    pub avg_frame_time_us: u64,
    /// Ring buffer of recent frame times.
    frame_times: Vec<u64>,
    /// Index into the ring buffer.
    frame_idx: usize,
    /// Number of frames recorded so far (for averaging before ring is full).
    frame_count: u64,
    /// Frames since last process stats sample.
    sample_countdown: u32,
    /// Cached sysinfo System (reused to avoid re-allocation).
    sys: Option<System>,
}

impl Default for PerfStats {
    fn default() -> Self {
        PerfStats {
            rss_bytes: 0,
            cpu_percent: 0.0,
            frame_time_us: 0,
            avg_frame_time_us: 0,
            frame_times: vec![0u64; 60],
            frame_idx: 0,
            frame_count: 0,
            sample_countdown: 0,
            sys: None,
        }
    }
}

impl PerfStats {
    /// Record a frame's duration in microseconds.
    pub fn record_frame(&mut self, duration_us: u64) {
        self.frame_time_us = duration_us;
        self.frame_times[self.frame_idx] = duration_us;
        self.frame_idx = (self.frame_idx + 1) % self.frame_times.len();
        self.frame_count += 1;

        let count = self.frame_count.min(self.frame_times.len() as u64);
        let sum: u64 = if self.frame_count >= self.frame_times.len() as u64 {
            self.frame_times.iter().sum()
        } else {
            self.frame_times[..self.frame_count as usize].iter().sum()
        };
        self.avg_frame_time_us = sum.checked_div(count).unwrap_or(0);
    }

    /// Compute FPS from average frame time.
    pub fn fps(&self) -> f64 {
        if self.avg_frame_time_us == 0 {
            0.0
        } else {
            1_000_000.0 / self.avg_frame_time_us as f64
        }
    }

    /// Sample process-level stats (RSS, CPU). Rate-limited: only queries
    /// sysinfo every 120 calls (~6s at 20fps) to keep idle CPU low.
    pub fn sample_process_stats(&mut self) {
        if self.sample_countdown > 0 {
            self.sample_countdown -= 1;
            return;
        }
        self.sample_countdown = 120;

        let pid = Pid::from_u32(std::process::id());
        let sys = self.sys.get_or_insert_with(System::new);
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        if let Some(proc_info) = sys.process(pid) {
            self.rss_bytes = proc_info.memory();
            self.cpu_percent = proc_info.cpu_usage();
        }
    }
}
