//! v3.7.9: pointer + keyboard input for the in-panel
//! click-through takeover.
//!
//! This module is the typed boundary between the
//! renderer's `PointerEvent` / `KeyboardEvent` /
//! `WheelEvent` stream and the xdotool invocation we run
//! on the per-Bot VM. The renderer hands us a
//! `InputEvent`, we turn it into a one-line shell command
//! (the `render_xdotool_script` function), and the
//! `ComputerManager::input_event` call site runs it over
//! the existing `SshPool::vm_exec` pipe.
//!
//! Why a dedicated module:
//!   - The renderer side (steps 6-8) needs a stable,
//!     serde-tagged enum. Keeping it in its own file
//!     means the Tauri command surface can `use
//!     crate::computer::input::InputEvent` without
//!     pulling in the rest of the computer module.
//!   - `render_xdotool_script` is pure (no I/O, no
//!     `async`, no SshPool), so it's trivially unit-
//!     testable. The driving-flag check that gates
//!     `input_event` lives in `mod.rs` so it can share
//!     the per-Bot `AtomicBool` map.
//!
//! Why a single shell command per event (not a batched
//! script): the renderer fires one event at a time at
//! ~60Hz when the user is moving the mouse. Each event
//! is a single `xdotool` call. We could batch them
//! server-side with `xdotool ... && xdotool ...`, but
//! the round-trip latency is already low (the SSH pool
//! to a Linux server over LAN is <5ms) and batching
//! would couple failures (a partial failure mid-batch
//! would leave the VM in an inconsistent state). One
//! event = one SSH call = one atomic outcome.

use serde::{Deserialize, Serialize};

/// v3.7.9: the per-event input shape the renderer
/// forwards. The `#[serde(tag = "type", rename_all =
/// "snake_case")]` gives us a wire format like
/// `{ "type": "pointer_move", "x": 120, "y": 340 }`,
/// which mirrors the DOM event names so the renderer's
/// handler can build a discriminated union with the
/// same string tags.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputEvent {
    /// Move the X11 cursor without clicking. Used for
    /// hover and for the pointermove of every drag.
    PointerMove { x: u32, y: u32 },
    /// Button down at the framebuffer coords. `button`
    /// is 1 (left), 2 (middle), 3 (right) per the DOM
    /// `MouseEvent.button` convention. xdotool's
    /// `mousedown` doesn't take coordinates — it's a
    /// "press the current button" verb. So we move
    /// first, then down.
    PointerDown { x: u32, y: u32, button: u8 },
    PointerUp { x: u32, y: u32, button: u8 },
    /// Wheel events. deltaY > 0 = scroll down, < 0 =
    /// up. xdotool `click 4` (up) / `click 5` (down) at
    /// (x, y). One click per wheel notch (multi-click
    /// is future work).
    Wheel { x: u32, y: u32, delta_y: i32 },
    /// Press or release a single named key. Use
    /// `xdotool keydown` / `keyup` (not `key`, which is
    /// press+release).
    KeyDown { name: String },
    KeyUp { name: String },
    /// Type a literal string. Uses `xdotool type -- "..."`
    /// so a leading `-` survives.
    Type { text: String },
}

/// Convert a renderer event into the xdotool command
/// string. Always prefixes `DISPLAY=:1` and uses the
/// `--` separator before user-supplied args.
///
/// `DISPLAY=:1` matches the existing `vm_computer_use`
/// tool's xdotool invocation — same display, same `--`
/// discipline. (The plan's pending decision is to keep
/// this; option (B) `:0` is a future slice.)
pub fn render_xdotool_script(event: &InputEvent) -> String {
    let disp = "DISPLAY=:1";
    match event {
        InputEvent::PointerMove { x, y } => {
            format!("{disp} xdotool mousemove -- {x} {y}")
        }
        InputEvent::PointerDown { x, y, button } => format!(
            "{disp} xdotool mousemove -- {x} {y} && {disp} xdotool mousedown -- {button}"
        ),
        InputEvent::PointerUp { x, y, button } => format!(
            "{disp} xdotool mousemove -- {x} {y} && {disp} xdotool mouseup -- {button}"
        ),
        InputEvent::Wheel { x, y, delta_y } => {
            // xdotool's button 4 is scroll up, 5 is scroll
            // down. delta_y < 0 is "user scrolled up";
            // delta_y > 0 is "user scrolled down". A
            // zero delta (a wheel event with no movement,
            // rare but possible) picks 5 (down) — the
            // only path that branches on `< 0` is "up".
            let btn = if *delta_y < 0 { 4 } else { 5 };
            format!(
                "{disp} xdotool mousemove -- {x} {y} && {disp} xdotool click -- {btn}"
            )
        }
        InputEvent::KeyDown { name } => {
            format!("{disp} xdotool keydown -- {}", shell_quote(name))
        }
        InputEvent::KeyUp { name } => {
            format!("{disp} xdotool keyup -- {}", shell_quote(name))
        }
        InputEvent::Type { text } => {
            format!("{disp} xdotool type -- {}", shell_quote(text))
        }
    }
}

/// Single-quote `s` for safe shell interpolation. Same
/// style as `shell_quote` in `tools/shell_run.rs` and
/// `tools/vm_computer_use.rs`. The `'\''` escape is the
/// standard POSIX-portable way to embed a single quote
/// inside a single-quoted string: close the string,
/// insert a literal `'` via `\'`, reopen the string.
fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every script starts with `DISPLAY=:1 xdotool` —
    /// the brief is explicit that the renderer must
    /// never have to think about the X display number.
    #[test]
    fn every_event_prefixes_display_and_xdotool() {
        let events: Vec<InputEvent> = vec![
            InputEvent::PointerMove { x: 0, y: 0 },
            InputEvent::PointerDown { x: 0, y: 0, button: 1 },
            InputEvent::PointerUp { x: 0, y: 0, button: 1 },
            InputEvent::Wheel { x: 0, y: 0, delta_y: 0 },
            InputEvent::KeyDown { name: "Return".into() },
            InputEvent::KeyUp { name: "Return".into() },
            InputEvent::Type { text: "x".into() },
        ];
        for ev in &events {
            let script = render_xdotool_script(ev);
            assert!(
                script.starts_with("DISPLAY=:1 xdotool "),
                "event {ev:?} produced non-prefixed script: {script:?}"
            );
        }
    }

    /// `PointerMove` is a single xdotool call. No `&&`
    /// chains, no `mousedown` / `mouseup`.
    #[test]
    fn pointer_move_is_single_mousemove_call() {
        let s = render_xdotool_script(&InputEvent::PointerMove { x: 120, y: 340 });
        assert_eq!(s, "DISPLAY=:1 xdotool mousemove -- 120 340");
    }

    /// `PointerDown` moves first, then presses the
    /// current button. The `mousemove -- X Y` is the
    /// first half of the chain so the press lands on
    /// the right pixel.
    #[test]
    fn pointer_down_moves_then_presses_current_button() {
        let s = render_xdotool_script(&InputEvent::PointerDown {
            x: 420,
            y: 180,
            button: 1,
        });
        assert_eq!(
            s,
            "DISPLAY=:1 xdotool mousemove -- 420 180 && DISPLAY=:1 xdotool mousedown -- 1"
        );
        // Explicit assertions for the sub-clauses so a
        // future refactor doesn't accidentally drop the
        // move or the down.
        assert!(s.contains("mousemove -- 420 180"));
        assert!(s.contains("mousedown -- 1"));
        // Symmetry: a `Down` event must NOT call
        // `mouseup`. A regression that flips the verb
        // would silently change click semantics.
        assert!(!s.contains("mouseup"));
    }

    /// `PointerUp` mirrors `PointerDown`: move first,
    /// then release. The verb is `mouseup`, not
    /// `mousedown`.
    #[test]
    fn pointer_up_moves_then_releases_current_button() {
        let s = render_xdotool_script(&InputEvent::PointerUp {
            x: 50,
            y: 60,
            button: 3,
        });
        assert_eq!(
            s,
            "DISPLAY=:1 xdotool mousemove -- 50 60 && DISPLAY=:1 xdotool mouseup -- 3"
        );
        assert!(s.contains("mousemove -- 50 60"));
        assert!(s.contains("mouseup -- 3"));
        assert!(!s.contains("mousedown"));
    }

    /// Right-click (`button: 3`) and middle-click
    /// (`button: 2`) pass through unchanged. The DOM
    /// `MouseEvent.button` convention is 1/2/3, which
    /// matches xdotool's `mousedown -- N`.
    #[test]
    fn pointer_down_passes_through_dom_button_numbers() {
        for button in [1u8, 2, 3] {
            let s = render_xdotool_script(&InputEvent::PointerDown {
                x: 10,
                y: 20,
                button,
            });
            assert!(
                s.contains(&format!("mousedown -- {button}")),
                "button {button} not preserved: {s:?}"
            );
        }
    }

    /// Wheel events with positive delta_y scroll down
    /// (xdotool button 5). The brief names the buttons
    /// explicitly: 4 = up, 5 = down.
    #[test]
    fn wheel_positive_delta_y_is_click_5_down() {
        let s = render_xdotool_script(&InputEvent::Wheel {
            x: 100,
            y: 200,
            delta_y: 5,
        });
        assert!(s.contains("mousemove -- 100 200"));
        assert!(s.contains("click -- 5"), "expected click 5, got: {s:?}");
    }

    /// Negative delta_y = scroll up = xdotool button 4.
    #[test]
    fn wheel_negative_delta_y_is_click_4_up() {
        let s = render_xdotool_script(&InputEvent::Wheel {
            x: 0,
            y: 0,
            delta_y: -3,
        });
        assert!(s.contains("click -- 4"), "expected click 4, got: {s:?}");
    }

    /// `delta_y == 0` is a degenerate case (the
    /// renderer might fire a wheel event with no
    /// movement). The brief is explicit: `delta_y < 0`
    /// is the only "up" path, so zero falls through to
    /// `else` (button 5 / down). The test pins that
    /// branch so a future refactor that switches to
    /// `<=` or `!=` doesn't silently flip the
    /// behavior.
    #[test]
    fn wheel_zero_delta_y_picks_button_5() {
        let s = render_xdotool_script(&InputEvent::Wheel {
            x: 0,
            y: 0,
            delta_y: 0,
        });
        assert!(s.contains("click -- 5"), "delta_y=0 should be button 5, got: {s:?}");
        assert!(!s.contains("click -- 4"));
    }

    /// `KeyDown` uses xdotool's `keydown` verb (not
    /// `key`, which is press+release). Holding a key
    /// down is required for things like
    /// `shift+click`, so the renderer must be able to
    /// fire Down without an Up.
    #[test]
    fn key_down_uses_keydown_verb() {
        let s = render_xdotool_script(&InputEvent::KeyDown {
            name: "ctrl+l".into(),
        });
        assert_eq!(s, "DISPLAY=:1 xdotool keydown -- 'ctrl+l'");
        // Pin the absence of `keyup` and `key ` (with a
        // trailing space — the press+release verb) so
        // a regression that re-introduces `key` would
        // fail this test loudly.
        assert!(!s.contains("keyup"));
        assert!(!s.contains("xdotool key "));
    }

    /// `KeyUp` uses `keyup`, not `keydown` and not
    /// `key`. Mirrors the `KeyDown` test.
    #[test]
    fn key_up_uses_keyup_verb() {
        let s = render_xdotool_script(&InputEvent::KeyUp {
            name: "ctrl+l".into(),
        });
        assert_eq!(s, "DISPLAY=:1 xdotool keyup -- 'ctrl+l'");
        assert!(!s.contains("keydown"));
        assert!(!s.contains("xdotool key "));
    }

    /// `Type` uses xdotool's `type` verb and the text
    /// is wrapped in single quotes. The `--` separator
    /// protects a leading `-`.
    #[test]
    fn type_wraps_text_in_single_quotes() {
        let s = render_xdotool_script(&InputEvent::Type {
            text: "hello world".into(),
        });
        assert_eq!(s, "DISPLAY=:1 xdotool type -- 'hello world'");
    }

    /// A leading `-` in the typed text survives the
    /// quoting because the `--` separator tells
    /// xdotool to stop parsing flags at the next
    /// token. The text itself is wrapped in single
    /// quotes so the shell doesn't strip the dash.
    #[test]
    fn type_with_leading_dash_survives_quoting() {
        let s = render_xdotool_script(&InputEvent::Type {
            text: "-rf /".into(),
        });
        // The dash is literal: `xdotool type -- '-rf /'`.
        // xdotool then types `-rf /` into the focused
        // widget.
        assert!(
            s.contains("type -- '-rf /'"),
            "leading dash not preserved: {s:?}"
        );
    }

    /// A single quote in the typed text is escaped via
    /// the standard `'\''` trick. The shell sees the
    /// four-character sequence `\'\''` and joins it
    /// with the surrounding single-quoted strings to
    /// produce a literal `'` in the resulting
    /// argument. This is the same trick
    /// `tools/vm_computer_use.rs::shell_quote` uses.
    #[test]
    fn type_with_single_quote_escapes_correctly() {
        let s = render_xdotool_script(&InputEvent::Type {
            text: "it's".into(),
        });
        // `it's` becomes `'it'\\''s'` in the command
        // string. The `\\` in the source is a single
        // backslash in the string; the `\\''` is
        // backslash-single-quote-single-quote.
        assert!(
            s.contains("type -- 'it'\\''s'"),
            "single-quote escape broken: {s:?}"
        );
    }

    /// A single quote in a key name is escaped the
    /// same way. The renderer's DOM `KeyboardEvent.key`
    /// doesn't contain quotes in practice, but the
    /// `code` field could in pathological cases — the
    /// shell-quoting guardrail should hold for both
    /// `Type` and `Key*` variants.
    #[test]
    fn key_down_with_single_quote_escapes_correctly() {
        let s = render_xdotool_script(&InputEvent::KeyDown {
            name: "a'b".into(),
        });
        assert!(
            s.contains("keydown -- 'a'\\''b'"),
            "single-quote escape broken: {s:?}"
        );
    }

    /// A leading `-` in a key name survives the
    /// quoting. The `--` separator protects it from
    /// being parsed as a flag by xdotool.
    #[test]
    fn key_down_with_leading_dash_survives() {
        let s = render_xdotool_script(&InputEvent::KeyDown {
            name: "-weird-name".into(),
        });
        assert!(
            s.contains("keydown -- '-weird-name'"),
            "leading dash not preserved: {s:?}"
        );
    }

    /// `shell_quote` is the lowest-level guard. Pin
    /// its behavior on the two interesting inputs: a
    /// plain string and a string with a single quote.
    #[test]
    fn shell_quote_basic_and_quote_escape() {
        assert_eq!(shell_quote("hello"), "'hello'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        // Empty string is still wrapped — the result
        // is `''` (two single quotes, which shell
        // parses as an empty argument).
        assert_eq!(shell_quote(""), "''");
    }

    /// Round-trip: every `InputEvent` is `Serialize +
    /// Deserialize`. The renderer's TS handler builds
    /// the JSON; the Rust side parses it. A regression
    /// in the serde attributes that breaks the wire
    /// format would surface here.
    #[test]
    fn input_events_round_trip_through_serde() {
        let events: Vec<InputEvent> = vec![
            InputEvent::PointerMove { x: 1, y: 2 },
            InputEvent::PointerDown { x: 3, y: 4, button: 2 },
            InputEvent::PointerUp { x: 5, y: 6, button: 3 },
            InputEvent::Wheel {
                x: 7,
                y: 8,
                delta_y: -1,
            },
            InputEvent::KeyDown {
                name: "Return".into(),
            },
            InputEvent::KeyUp {
                name: "Escape".into(),
            },
            InputEvent::Type {
                text: "hello".into(),
            },
        ];
        for ev in &events {
            let json = serde_json::to_string(ev).expect("serialize");
            let back: InputEvent = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&back, ev, "round-trip mismatch for {ev:?}: {json}");
        }
    }

    /// Wire format uses `snake_case` `type` tags so the
    /// renderer's TS handler can match on the literal
    /// string `"pointer_move"`, `"pointer_down"`, etc.
    /// Pinning the JSON shape here protects against a
    /// silent rename that would break the IPC.
    #[test]
    fn wire_format_uses_snake_case_type_tags() {
        let json = serde_json::to_string(&InputEvent::PointerMove { x: 0, y: 0 })
            .expect("serialize");
        assert!(
            json.contains("\"type\":\"pointer_move\""),
            "expected snake_case tag, got: {json}"
        );
        let json = serde_json::to_string(&InputEvent::KeyDown {
            name: "x".into(),
        })
        .expect("serialize");
        assert!(
            json.contains("\"type\":\"key_down\""),
            "expected snake_case tag, got: {json}"
        );
        let json = serde_json::to_string(&InputEvent::Type {
            text: "x".into(),
        })
        .expect("serialize");
        assert!(
            json.contains("\"type\":\"type\""),
            "expected type tag, got: {json}"
        );
    }
}
