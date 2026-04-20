//! Session event recording for reproducible debugging.
//!
//! Records `InputEvent`s with timestamps into a ring buffer. Events can be
//! dumped to JSON for analysis or saved/loaded from files.

use std::collections::VecDeque;
use std::time::Instant;

use crate::input::InputEvent;
use crate::keymap::KeyPress;

const EVENT_CAP: usize = 10_000;

/// A timestamped input event.
#[derive(Debug, Clone)]
pub struct TimestampedEvent {
    /// Microseconds since recording started.
    pub offset_us: u64,
    /// The input event.
    pub event: InputEvent,
}

/// Records input events with timing information.
#[derive(Debug, Default)]
pub struct EventRecorder {
    recording: bool,
    events: VecDeque<TimestampedEvent>,
    start_time: Option<Instant>,
}

impl EventRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start recording events. Clears any existing recording.
    pub fn start_recording(&mut self) {
        self.recording = true;
        self.events.clear();
        self.start_time = Some(Instant::now());
    }

    /// Stop recording events.
    pub fn stop_recording(&mut self) {
        self.recording = false;
    }

    /// Whether recording is active.
    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// Number of recorded events.
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Duration since recording started (microseconds), or 0 if not recording.
    pub fn duration_us(&self) -> u64 {
        self.start_time
            .map(|t| t.elapsed().as_micros() as u64)
            .unwrap_or(0)
    }

    /// Record an event (only if recording is active).
    pub fn record(&mut self, event: InputEvent) {
        if !self.recording {
            return;
        }
        let offset_us = self
            .start_time
            .map(|t| t.elapsed().as_micros() as u64)
            .unwrap_or(0);

        if self.events.len() >= EVENT_CAP {
            self.events.pop_front();
        }
        self.events.push_back(TimestampedEvent { offset_us, event });
    }

    /// Get the last N events as a slice-like view.
    pub fn last_n(&self, n: usize) -> Vec<&TimestampedEvent> {
        let skip = self.events.len().saturating_sub(n);
        self.events.iter().skip(skip).collect()
    }

    /// Get all events.
    pub fn events(&self) -> &VecDeque<TimestampedEvent> {
        &self.events
    }

    /// Save recording to a JSON file.
    pub fn save(&self, path: &std::path::Path) -> Result<usize, String> {
        let entries: Vec<serde_json::Value> = self
            .events
            .iter()
            .map(|e| {
                serde_json::json!({
                    "offset_us": e.offset_us,
                    "event": format_event(&e.event),
                })
            })
            .collect();
        let json = serde_json::json!({
            "version": 1,
            "event_count": entries.len(),
            "events": entries,
        });
        let s =
            serde_json::to_string_pretty(&json).map_err(|e| format!("serialize failed: {}", e))?;
        std::fs::write(path, s).map_err(|e| format!("write failed: {}", e))?;
        Ok(self.events.len())
    }

    /// Dump the last N events as a JSON string (for AI tool consumption).
    pub fn dump_json(&self, last_n: usize) -> String {
        let events: Vec<serde_json::Value> = self
            .last_n(last_n)
            .iter()
            .map(|e| {
                serde_json::json!({
                    "offset_us": e.offset_us,
                    "event": format_event(&e.event),
                })
            })
            .collect();
        serde_json::to_string_pretty(&events).unwrap_or_else(|_| "[]".to_string())
    }
}

fn format_event(event: &InputEvent) -> serde_json::Value {
    match event {
        InputEvent::Key(kp) => {
            serde_json::json!({
                "type": "key",
                "key": format_keypress(kp),
            })
        }
        InputEvent::Resize(w, h) => {
            serde_json::json!({
                "type": "resize",
                "width": w,
                "height": h,
            })
        }
        InputEvent::MouseClick { row, col, button } => {
            serde_json::json!({
                "type": "mouse_click",
                "row": row,
                "col": col,
                "button": format!("{:?}", button),
            })
        }
        InputEvent::MouseScroll { delta } => {
            serde_json::json!({
                "type": "mouse_scroll",
                "delta": delta,
            })
        }
    }
}

fn format_keypress(kp: &KeyPress) -> String {
    let mut s = String::new();
    if kp.ctrl {
        s.push_str("C-");
    }
    if kp.alt {
        s.push_str("M-");
    }
    s.push_str(&format!("{:?}", kp.key));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Key;

    #[test]
    fn recording_lifecycle() {
        let mut rec = EventRecorder::new();
        assert!(!rec.is_recording());
        assert_eq!(rec.event_count(), 0);

        rec.start_recording();
        assert!(rec.is_recording());

        let kp = KeyPress {
            key: Key::Char('a'),
            ctrl: false,
            alt: false,
        };
        rec.record(InputEvent::Key(kp));
        assert_eq!(rec.event_count(), 1);

        rec.stop_recording();
        assert!(!rec.is_recording());

        // Events after stop are not recorded
        let kp2 = KeyPress {
            key: Key::Char('b'),
            ctrl: false,
            alt: false,
        };
        rec.record(InputEvent::Key(kp2));
        assert_eq!(rec.event_count(), 1);
    }

    #[test]
    fn last_n_returns_tail() {
        let mut rec = EventRecorder::new();
        rec.start_recording();
        for i in 0..10 {
            let kp = KeyPress {
                key: Key::Char(char::from(b'a' + i)),
                ctrl: false,
                alt: false,
            };
            rec.record(InputEvent::Key(kp));
        }
        let last3 = rec.last_n(3);
        assert_eq!(last3.len(), 3);
    }

    #[test]
    fn cap_enforced() {
        let mut rec = EventRecorder::new();
        rec.start_recording();
        for _ in 0..EVENT_CAP + 100 {
            let kp = KeyPress {
                key: Key::Char('x'),
                ctrl: false,
                alt: false,
            };
            rec.record(InputEvent::Key(kp));
        }
        assert_eq!(rec.event_count(), EVENT_CAP);
    }

    #[test]
    fn dump_json_is_valid() {
        let mut rec = EventRecorder::new();
        rec.start_recording();
        let kp = KeyPress {
            key: Key::Char('a'),
            ctrl: false,
            alt: false,
        };
        rec.record(InputEvent::Key(kp));
        let json = rec.dump_json(10);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
    }

    #[test]
    fn save_creates_valid_json_file() {
        let mut rec = EventRecorder::new();
        rec.start_recording();
        let kp = KeyPress {
            key: Key::Char('z'),
            ctrl: true,
            alt: false,
        };
        rec.record(InputEvent::Key(kp));
        rec.stop_recording();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recording.json");
        let count = rec.save(&path).unwrap();
        assert_eq!(count, 1);

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["version"], 1);
        assert_eq!(parsed["event_count"], 1);
        assert!(parsed["events"].is_array());
    }

    #[test]
    fn start_clears_previous_recording() {
        let mut rec = EventRecorder::new();
        rec.start_recording();
        let kp = KeyPress {
            key: Key::Char('a'),
            ctrl: false,
            alt: false,
        };
        rec.record(InputEvent::Key(kp));
        assert_eq!(rec.event_count(), 1);

        rec.start_recording();
        assert_eq!(rec.event_count(), 0);
    }

    #[test]
    fn format_event_variants() {
        // Key
        let kp = KeyPress {
            key: Key::Char('x'),
            ctrl: true,
            alt: true,
        };
        let val = format_event(&InputEvent::Key(kp));
        assert_eq!(val["type"], "key");
        assert_eq!(val["key"], "C-M-Char('x')");

        // Resize
        let val = format_event(&InputEvent::Resize(120, 40));
        assert_eq!(val["type"], "resize");
        assert_eq!(val["width"], 120);

        // MouseClick
        let val = format_event(&InputEvent::MouseClick {
            row: 5,
            col: 10,
            button: crate::input::MouseButton::Left,
        });
        assert_eq!(val["type"], "mouse_click");
        assert_eq!(val["row"], 5);

        // MouseScroll
        let val = format_event(&InputEvent::MouseScroll { delta: -3 });
        assert_eq!(val["type"], "mouse_scroll");
        assert_eq!(val["delta"], -3);
    }
}
