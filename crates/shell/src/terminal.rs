//! High-level terminal emulator wrapping alacritty_terminal.
//!
//! `ShellTerminal` manages the full lifecycle: PTY spawn, I/O thread,
//! terminal state, input/output, and resize. The grid can be read for
//! rendering by the mae-renderer crate.

use std::borrow::Cow;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty;
use tracing::{debug, error, trace};

use crate::event::{ShellEvent, ShellEventListener};

/// Terminal dimensions in cells.
pub struct TermSize {
    pub columns: usize,
    pub screen_lines: usize,
}

impl TermSize {
    pub fn new(columns: usize, screen_lines: usize) -> Self {
        Self {
            columns,
            screen_lines,
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Custom text selection tracker for shell terminals.
/// alacritty_terminal v0.26.0 doesn't expose a public Selection API,
/// so we track selection state ourselves and read text from the grid.
pub struct ShellSelection {
    start: (usize, usize), // (row, col)
    end: (usize, usize),
    active: bool,
}

/// A running terminal emulator backed by a PTY + alacritty_terminal.
pub struct ShellTerminal {
    /// The terminal state, shared with the I/O thread via FairMutex.
    term: Arc<FairMutex<Term<ShellEventListener>>>,

    /// Channel for sending input to the PTY (keyboard, paste, resize).
    pty_tx: EventLoopSender,

    /// Receiver for terminal events (bell, title change, exit, etc.).
    event_rx: mpsc::Receiver<ShellEvent>,

    /// Handle to the I/O thread (joined on drop).
    _io_thread: JoinHandle<(
        EventLoop<tty::Pty, ShellEventListener>,
        alacritty_terminal::event_loop::State,
    )>,

    /// Terminal title (updated from events).
    title: String,

    /// Whether the child process has exited.
    exited: bool,

    /// PID of the child shell process.
    child_pid: u32,

    /// Generation counter — incremented each time `poll_events()` receives
    /// new data from the PTY. Renderers compare this to a cached value to
    /// avoid needless redraws when the shell is idle.
    generation: u64,

    /// Custom text selection state for mouse-based text selection.
    selection: Option<ShellSelection>,
}

/// Ensure common user binary directories are in PATH.
///
/// When MAE is launched from a desktop file (GNOME, sway, etc.), the parent
/// process has a minimal PATH that omits `~/.local/bin`, `~/.cargo/bin`, etc.
/// Terminal emulators (Alacritty, kitty, wezterm) all solve this by sourcing
/// the user's shell profile. We take a simpler approach: prepend the standard
/// directories if they exist and aren't already in PATH.
fn augment_path(env: &mut std::collections::HashMap<String, String>) {
    let home = match env
        .get("HOME")
        .cloned()
        .or_else(|| std::env::var("HOME").ok())
    {
        Some(h) => h,
        None => return,
    };
    let extra_dirs = [
        format!("{home}/.local/bin"),
        format!("{home}/.cargo/bin"),
        format!("{home}/bin"),
        format!("{home}/.npm-global/bin"),
    ];
    let current_path = env.get("PATH").cloned().unwrap_or_default();
    let path_entries: std::collections::HashSet<&str> = current_path.split(':').collect();
    let mut prepend = Vec::new();
    for dir in &extra_dirs {
        if !path_entries.contains(dir.as_str()) && std::path::Path::new(dir).is_dir() {
            prepend.push(dir.as_str());
        }
    }
    if !prepend.is_empty() {
        let new_path = format!("{}:{}", prepend.join(":"), current_path);
        env.insert("PATH".to_string(), new_path);
    }
}

impl ShellTerminal {
    /// Spawn a new terminal running the user's shell.
    ///
    /// `cols` and `rows` are the initial terminal dimensions in cells.
    /// `working_dir` is the starting directory (None = inherit).
    pub fn spawn(
        cols: u16,
        rows: u16,
        working_dir: Option<std::path::PathBuf>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::spawn_with_env(cols, rows, working_dir, std::collections::HashMap::new())
    }

    /// Spawn a terminal running a specific command (not the user's shell).
    /// When the command exits, the PTY exits — ideal for agent processes
    /// where the lifecycle should be tied to the command, not a shell.
    pub fn spawn_command(
        cols: u16,
        rows: u16,
        command: &str,
        working_dir: Option<std::path::PathBuf>,
        extra_env: std::collections::HashMap<String, String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let cols = cols.max(2);
        let rows = rows.max(1);
        let columns = cols as usize;
        let screen_lines = rows as usize;

        let (event_tx, event_rx) = mpsc::channel();
        let listener = ShellEventListener::new(event_tx);

        let config = TermConfig::default();
        let size = TermSize::new(columns, screen_lines);
        let term = Term::new(config, &size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        // Parse command into program + args (simple space-split).
        let parts: Vec<&str> = command.split_whitespace().collect();
        let (program, args) = if parts.is_empty() {
            return Err("empty command".into());
        } else {
            (
                parts[0].to_string(),
                parts[1..].iter().map(|s| s.to_string()).collect(),
            )
        };

        let mut env: std::collections::HashMap<String, String> = std::env::vars().collect();
        env.insert("MAE_TERMINAL".to_string(), "1".to_string());
        env.extend(extra_env);
        augment_path(&mut env);
        let pty_opts = tty::Options {
            shell: Some(tty::Shell::new(program, args)),
            working_directory: working_dir,
            env,
            ..Default::default()
        };

        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width: 1,
            cell_height: 1,
        };

        tty::setup_env();
        let pty = tty::new(&pty_opts, window_size, 0)?;
        let child_pid = pty.child().id();
        let event_loop = EventLoop::new(Arc::clone(&term), listener, pty, true, false)?;
        let pty_tx = event_loop.channel();
        let io_thread = event_loop.spawn();

        debug!(cols, rows, command, "agent terminal spawned");

        Ok(ShellTerminal {
            term,
            pty_tx,
            event_rx,
            _io_thread: io_thread,
            title: String::new(),
            exited: false,
            child_pid,
            generation: 0,
            selection: None,
        })
    }

    /// Spawn a new terminal with extra environment variables injected
    /// into the child process (e.g. `MAE_MCP_SOCKET`).
    pub fn spawn_with_env(
        cols: u16,
        rows: u16,
        working_dir: Option<std::path::PathBuf>,
        extra_env: std::collections::HashMap<String, String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let cols = cols.max(2);
        let rows = rows.max(1);
        let columns = cols as usize;
        let screen_lines = rows as usize;

        // Event channel: terminal → MAE.
        let (event_tx, event_rx) = mpsc::channel();
        let listener = ShellEventListener::new(event_tx);

        // Terminal config.
        let config = TermConfig::default();
        let size = TermSize::new(columns, screen_lines);

        // Create the terminal state.
        let term = Term::new(config, &size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        // PTY options.
        let mut env: std::collections::HashMap<String, String> = std::env::vars().collect();
        env.insert("MAE_TERMINAL".to_string(), "1".to_string());
        env.extend(extra_env);
        augment_path(&mut env);
        let pty_opts = tty::Options {
            working_directory: working_dir,
            env,
            ..Default::default()
        };

        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width: 1,
            cell_height: 1,
        };

        // Set up terminfo environment.
        tty::setup_env();

        // Spawn the PTY.
        let pty = tty::new(&pty_opts, window_size, 0)?;

        // Extract child PID before the PTY is moved into the event loop.
        let child_pid = pty.child().id();

        // Create and spawn the I/O event loop.
        let event_loop = EventLoop::new(
            Arc::clone(&term),
            listener,
            pty,
            /* drain_on_exit */ true,
            /* ref_test */ false,
        )?;
        let pty_tx = event_loop.channel();
        let io_thread = event_loop.spawn();

        debug!(cols, rows, program = ?pty_opts.shell, "shell terminal spawned");

        Ok(ShellTerminal {
            term,
            pty_tx,
            event_rx,
            _io_thread: io_thread,
            title: String::new(),
            exited: false,
            child_pid,
            generation: 0,
            selection: None,
        })
    }

    /// Send keyboard/paste input to the PTY.
    pub fn write_input(&self, data: &[u8]) {
        if let Err(e) = self.pty_tx.send(Msg::Input(Cow::Owned(data.to_vec()))) {
            error!("failed to send input to PTY: {}", e);
        }
    }

    /// Send a string to the PTY (convenience wrapper).
    pub fn write_str(&self, s: &str) {
        self.write_input(s.as_bytes());
    }

    /// Resize the terminal.
    pub fn resize(&self, cols: u16, rows: u16) {
        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width: 1,
            cell_height: 1,
        };
        if let Err(e) = self.pty_tx.send(Msg::Resize(window_size)) {
            error!("failed to resize PTY: {}", e);
        }
        // Also resize the terminal grid.
        let mut term = self.term.lock();
        term.resize(TermSize::new(cols as usize, rows as usize));
    }

    /// Drain pending events from the terminal. Call this in the main loop.
    /// Returns events that need handling (bell, title, exit, etc.).
    pub fn poll_events(&mut self) -> Vec<ShellEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            match &event {
                ShellEvent::Title(t) => self.title = t.clone(),
                ShellEvent::ResetTitle => self.title.clear(),
                ShellEvent::ChildExit(_) => self.exited = true,
                ShellEvent::PtyWrite(s) => {
                    // Terminal wants to write back to PTY (e.g., device status response).
                    self.write_str(s);
                }
                _ => {}
            }
            events.push(event);
        }
        if !events.is_empty() {
            self.generation += 1;
        }
        events
    }

    /// Generation counter — incremented each time new events arrive from the PTY.
    /// Compare across frames to detect whether the shell produced new output.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Access the terminal state for rendering (locks the mutex).
    pub fn term(&self) -> impl std::ops::Deref<Target = Term<ShellEventListener>> + '_ {
        let lock_start = std::time::Instant::now();
        let guard = self.term.lock();
        trace!(
            wait_us = lock_start.elapsed().as_micros() as u64,
            "term lock acquired"
        );
        guard
    }

    /// Whether the child process has exited.
    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// Current terminal title (set by shell escape sequences).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// PID of the child shell process.
    pub fn child_pid(&self) -> u32 {
        self.child_pid
    }

    /// Current working directory of the foreground process in the terminal.
    ///
    /// On Linux, reads `/proc/{pid}/cwd` to determine the cwd. Returns `None`
    /// if the symlink cannot be read (e.g., process has exited, or on non-Linux).
    pub fn cwd(&self) -> Option<String> {
        let link = format!("/proc/{}/cwd", self.child_pid);
        std::fs::read_link(&link)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// Reset the terminal: send a full reset escape sequence to clear screen
    /// and restore default state. Fixes residual characters from full-screen
    /// programs like cmatrix, htop, etc. that don't clean up on kill.
    pub fn reset(&self) {
        // RIS (Reset to Initial State) + clear screen + home cursor
        self.write_input(b"\x1bc\x1b[2J\x1b[H");
    }

    /// Shutdown the terminal. Sends shutdown message to the I/O thread.
    pub fn shutdown(&self) {
        let _ = self.pty_tx.send(Msg::Shutdown);
    }

    /// Scroll the terminal display by a given amount.
    /// Used for scrollback navigation (Shift-PageUp/Down, Ctrl-Shift-j/k).
    pub fn scroll_display(&self, scroll: alacritty_terminal::grid::Scroll) {
        self.term.lock().scroll_display(scroll);
    }

    /// Scroll to the bottom of the terminal (live output).
    pub fn scroll_to_bottom(&self) {
        self.term
            .lock()
            .scroll_display(alacritty_terminal::grid::Scroll::Bottom);
    }

    /// Get the current display offset (0 = at bottom/live, >0 = scrolled up).
    pub fn display_offset(&self) -> usize {
        let lock_start = std::time::Instant::now();
        let term = self.term.lock();
        trace!(
            wait_us = lock_start.elapsed().as_micros() as u64,
            "term lock acquired (display_offset)"
        );
        term.grid().display_offset()
    }

    /// Start a new text selection at the given grid position.
    pub fn start_selection(&mut self, row: usize, col: usize) {
        self.selection = Some(ShellSelection {
            start: (row, col),
            end: (row, col),
            active: true,
        });
    }

    /// Update the selection endpoint (called during mouse drag).
    pub fn update_selection(&mut self, row: usize, col: usize) {
        if let Some(ref mut sel) = self.selection {
            if sel.active {
                sel.end = (row, col);
            }
        }
    }

    /// Finish the selection and extract the selected text from the grid.
    pub fn finish_selection(&mut self) -> Option<String> {
        let sel = self.selection.as_mut()?;
        sel.active = false;

        let (start, end) = if sel.start <= sel.end {
            (sel.start, sel.end)
        } else {
            (sel.end, sel.start)
        };

        let term = self.term.lock();
        let content = term.renderable_content();

        // Collect cells from renderable_content (same approach as read_viewport).
        let mut text = String::new();
        let mut last_line: Option<usize> = None;
        let mut line_buf = String::new();

        for cell in content.display_iter {
            let row_idx = cell.point.line.0 as usize;
            let col_idx = cell.point.column.0;

            if row_idx < start.0 || row_idx > end.0 {
                continue;
            }

            let col_start = if row_idx == start.0 { start.1 } else { 0 };
            let col_end = if row_idx == end.0 { end.1 } else { usize::MAX };

            if col_idx < col_start || col_idx > col_end {
                continue;
            }

            if last_line.is_some() && last_line != Some(row_idx) {
                // Flush previous line.
                text.push_str(line_buf.trim_end());
                text.push('\n');
                line_buf.clear();
            }
            last_line = Some(row_idx);
            line_buf.push(cell.c);
        }
        // Flush final line.
        text.push_str(line_buf.trim_end());

        Some(text.trim_end().to_string())
    }

    /// Clear any active selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Get the current selection range for rendering highlights.
    /// Returns `((start_row, start_col), (end_row, end_col))` in normalized order.
    pub fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let sel = self.selection.as_ref()?;
        if sel.start <= sel.end {
            Some((sel.start, sel.end))
        } else {
            Some((sel.end, sel.start))
        }
    }

    /// Pre-populate the terminal's color palette from the editor theme.
    ///
    /// This makes OSC 10/11 color queries return the correct theme colors,
    /// and ensures programs that inspect terminal colors see theme-aware
    /// values. Call after spawn and on theme change.
    ///
    /// Indices: 0-15 = ANSI base colors, 256 = foreground, 257 = background.
    pub fn set_theme_colors(&self, colors: &[(usize, (u8, u8, u8))]) {
        use alacritty_terminal::vte::ansi::{Handler, Rgb};
        let mut term = self.term.lock();
        for &(idx, (r, g, b)) in colors {
            term.set_color(idx, Rgb { r, g, b });
        }
    }

    /// Read a line of text from the terminal grid (0-indexed from top of viewport).
    /// Uses renderable_content to extract the line.
    pub fn read_line(&self, line: usize) -> String {
        let term = self.term.lock();
        let content = term.renderable_content();
        let mut result = String::new();
        for cell in content.display_iter {
            if cell.point.line.0 as usize == line {
                result.push(cell.c);
            }
        }
        result.trim_end().to_string()
    }

    /// Read recent terminal output as a string (last N lines of the viewport).
    pub fn read_viewport(&self, max_lines: usize) -> Vec<String> {
        let lock_start = std::time::Instant::now();
        let term = self.term.lock();
        trace!(
            wait_us = lock_start.elapsed().as_micros() as u64,
            "term lock acquired (read_viewport)"
        );
        let content = term.renderable_content();
        let mut lines: Vec<String> = Vec::new();
        let mut current_line = String::new();
        let mut current_line_idx: Option<usize> = None;

        for cell in content.display_iter {
            let line_idx = cell.point.line.0 as usize;
            if current_line_idx != Some(line_idx) {
                if current_line_idx.is_some() {
                    lines.push(std::mem::take(&mut current_line));
                }
                current_line_idx = Some(line_idx);
            }
            current_line.push(cell.c);
        }
        if !current_line.is_empty() {
            lines.push(current_line);
        }

        // Return last max_lines.
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn spawn_and_read_grid() {
        // Spawn a terminal with a simple echo command.
        let mut term = ShellTerminal::spawn(80, 24, None).expect("failed to spawn terminal");

        // Give the shell time to start and produce output.
        thread::sleep(Duration::from_millis(1000));
        term.poll_events();

        // Send a command.
        term.write_str("echo MAE_SHELL_TEST_OK\n");

        // Wait for output to appear in the grid.
        let mut found = false;
        for _ in 0..20 {
            thread::sleep(Duration::from_millis(100));
            term.poll_events();
            let viewport = term.read_viewport(24);
            let joined = viewport.join("\n");
            if joined.contains("MAE_SHELL_TEST_OK") {
                found = true;
                break;
            }
        }
        assert!(found, "viewport should contain echo output");

        term.shutdown();
    }

    #[test]
    fn resize_terminal() {
        let term = ShellTerminal::spawn(80, 24, None).expect("failed to spawn terminal");
        thread::sleep(Duration::from_millis(200));

        // Resize should not panic.
        term.resize(120, 40);

        // Verify grid dimensions updated.
        let t = term.term();
        assert_eq!(t.grid().columns(), 120);
        assert_eq!(t.grid().screen_lines(), 40);
        drop(t);

        term.shutdown();
    }

    #[test]
    fn child_exit_detected() {
        let mut term = ShellTerminal::spawn(80, 24, None).expect("failed to spawn terminal");
        // Wait for shell startup — generous timeout for CI under load.
        thread::sleep(Duration::from_millis(1000));
        term.poll_events();

        // Tell the shell to exit.
        term.write_str("exit\n");

        // Wait for exit event (up to 4s under load).
        let mut exited = false;
        for _ in 0..40 {
            thread::sleep(Duration::from_millis(100));
            term.poll_events();
            if term.has_exited() {
                exited = true;
                break;
            }
        }
        assert!(exited, "terminal should detect child exit");
    }
}
