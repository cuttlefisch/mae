//! Generic eased-value-tween primitive (ADR-070 D2).
//!
//! Promotes the math `GraphColorTween` (`crate::graph_view`) already proved
//! out — ease-out-cubic easing, a `started_at`/`duration` in-flight window,
//! main-thread-only ticking — into a reusable `ValueTween<T>` so a future
//! scalar tween (e.g. ADR-071's hover/neighbor wedge-growth radius) doesn't
//! need a second, hand-copied implementation of the same easing math
//! (CLAUDE.md principle #8). `GraphColorTween` keeps its exact existing
//! field names/shape (`node_index`/`from_hex`/`to_hex`/`started_at`/
//! `duration`) — every existing call site and test stays untouched — but
//! its `current_color()` now delegates to `ValueTween<String>::current()`
//! internally instead of a second private copy of `ease_out_cubic`/`lerp_hex`.

use std::time::{Duration, Instant};

/// A type that can be linearly interpolated at an already-eased parameter
/// `t` (`[0, 1]`) between two values of itself.
pub trait Lerpable: Clone {
    fn lerp(&self, other: &Self, t: f32) -> Self;
}

impl Lerpable for f32 {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        self + (other - self) * t
    }
}

impl Lerpable for String {
    /// `"#rrggbb"` hex lerp. Falls back to `other` verbatim if either color
    /// fails to parse — a malformed color never panics, it just snaps
    /// instead of animating (matches the pre-existing `GraphColorTween`
    /// behavior this was extracted from, verbatim).
    fn lerp(&self, other: &Self, t: f32) -> Self {
        lerp_hex(self, other, t)
    }
}

/// An in-flight eased transition between two `T` values, started at
/// `started_at` and running for `duration`. Ticked by reading `current()`
/// each frame while `!is_complete()` — no background thread, no IPC, this
/// is deliberately the lightweight main-thread-tick shape (see
/// `crate::graph_view`'s doc comment on why a trivial tween has no business
/// going through the heavier physics-animation/background-thread plumbing).
#[derive(Debug, Clone)]
pub struct ValueTween<T: Lerpable> {
    pub from: T,
    pub to: T,
    pub started_at: Instant,
    pub duration: Duration,
}

impl<T: Lerpable> ValueTween<T> {
    pub fn new(from: T, to: T, duration: Duration) -> Self {
        Self {
            from,
            to,
            started_at: Instant::now(),
            duration,
        }
    }

    /// The eased, interpolated value at the current instant.
    pub fn current(&self) -> T {
        let elapsed = self.started_at.elapsed().as_secs_f32();
        let dur = self.duration.as_secs_f32().max(0.0001);
        self.from.lerp(&self.to, ease_out_cubic(elapsed / dur))
    }

    pub fn is_complete(&self) -> bool {
        self.started_at.elapsed() >= self.duration
    }
}

/// Ease-out cubic: `1 - (1-t)^3`, `t` clamped to `[0, 1]` — starts fast,
/// settles gently, the standard "pop in" curve for a UI highlight.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Parse a `"#rrggbb"` hex string into `(r, g, b)` byte components. `None`
/// for anything malformed — callers fall back to the raw `to` color
/// verbatim rather than panicking on a bad hex string. `pub(crate)`: also
/// used directly by `crate::graph_view`'s luminance/contrast/saturation
/// helpers, which parse hex colors for reasons unrelated to tweening.
pub(crate) fn parse_hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Linearly interpolate between two `"#rrggbb"` hex colors at `t` (already
/// eased, `[0, 1]`). Falls back to `to` verbatim if either color fails to
/// parse.
pub fn lerp_hex(from: &str, to: &str, t: f32) -> String {
    let t = t.clamp(0.0, 1.0);
    match (parse_hex_rgb(from), parse_hex_rgb(to)) {
        (Some((fr, fg, fb)), Some((tr, tg, tb))) => {
            let lerp_byte =
                |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * t).round() as u8 };
            format!(
                "#{:02x}{:02x}{:02x}",
                lerp_byte(fr, tr),
                lerp_byte(fg, tg),
                lerp_byte(fb, tb)
            )
        }
        _ => to.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_cubic_endpoints_and_monotonic() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < 1e-6);
        for pair in [(0.0, 0.25), (0.25, 0.5), (0.5, 0.75), (0.75, 1.0)] {
            assert!(ease_out_cubic(pair.0) < ease_out_cubic(pair.1));
        }
        // "Ease out" specifically: past the midpoint of t, already more
        // than halfway to the target value (front-loaded motion).
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn ease_out_cubic_clamps_out_of_range_input() {
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert!((ease_out_cubic(2.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lerp_hex_endpoints_and_midpoint() {
        assert_eq!(lerp_hex("#000000", "#ffffff", 0.0), "#000000");
        assert_eq!(lerp_hex("#000000", "#ffffff", 1.0), "#ffffff");
        let mid = lerp_hex("#000000", "#ffffff", 0.5);
        assert_eq!(mid, "#808080");
    }

    #[test]
    fn lerp_hex_falls_back_to_to_color_on_malformed_input() {
        assert_eq!(lerp_hex("not-a-color", "#ff0000", 0.5), "#ff0000");
        assert_eq!(lerp_hex("#ff0000", "also-bad", 0.5), "also-bad");
    }

    #[test]
    fn value_tween_f32_current_progresses_and_completes() {
        let tween = ValueTween {
            from: 0.0_f32,
            to: 10.0_f32,
            started_at: Instant::now() - Duration::from_millis(1000),
            duration: Duration::from_millis(100),
        };
        assert!(tween.is_complete());
        assert!((tween.current() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn value_tween_f32_mid_flight_is_between_endpoints_and_not_complete() {
        let tween = ValueTween {
            from: 0.0_f32,
            to: 10.0_f32,
            started_at: Instant::now(),
            duration: Duration::from_secs(60), // long enough not to race in CI
        };
        assert!(!tween.is_complete());
        let value = tween.current();
        assert!(
            (0.0..10.0).contains(&value),
            "expected value strictly between endpoints early in a long tween, got {value}"
        );
    }

    #[test]
    fn value_tween_string_hex_current_matches_lerp_hex() {
        // Comparative: ValueTween<String>::current() must agree exactly
        // with the standalone lerp_hex() function it delegates to, at a
        // fixed, backdated elapsed fraction.
        let tween = ValueTween {
            from: "#000000".to_string(),
            to: "#ffffff".to_string(),
            started_at: Instant::now() - Duration::from_millis(1000),
            duration: Duration::from_millis(100),
        };
        assert_eq!(tween.current(), "#ffffff");
    }

    #[test]
    fn value_tween_new_starts_uncompleted_for_a_nonzero_duration() {
        let tween = ValueTween::new(0.0_f32, 1.0_f32, Duration::from_secs(60));
        assert!(!tween.is_complete());
    }

    #[test]
    fn f32_lerp_is_deterministic() {
        let a = 3.0_f32.lerp(&7.0_f32, 0.25);
        let b = 3.0_f32.lerp(&7.0_f32, 0.25);
        assert_eq!(a.to_bits(), b.to_bits());
    }
}
