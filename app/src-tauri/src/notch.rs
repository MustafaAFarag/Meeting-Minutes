//! The pure parts of the notch: window sizes, where the pill sits on the screen, and the
//! small `widget.json` file that remembers it. Nothing here touches Tauri, so it is all testable.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Logical size of the folded pill (width, height).
pub const FOLDED_SIZE: (f64, f64) = (180.0, 34.0);
/// Logical size of the unfolded pill (width, height).
pub const UNFOLDED_SIZE: (f64, f64) = (300.0, 160.0);

pub fn size_for(folded: bool) -> (f64, f64) {
    if folded {
        FOLDED_SIZE
    } else {
        UNFOLDED_SIZE
    }
}

/// A monitor's area in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    fn contains_x(&self, x: i32) -> bool {
        x >= self.x && x < self.x + self.width
    }
}

/// Left edge of a window of `width` whose horizontal centre is `center_x`.
pub fn anchored_x(center_x: i32, width: i32) -> i32 {
    center_x - width / 2
}

/// Horizontal centre of a window whose left edge is `x`.
pub fn center_from_x(x: i32, width: i32) -> i32 {
    x + width / 2
}

/// The saved centre if some monitor still contains it, else the middle of the primary monitor.
pub fn resolve_center(saved: Option<i32>, monitors: &[Rect], primary: Rect) -> i32 {
    match saved {
        Some(x) if monitors.iter().any(|m| m.contains_x(x)) => x,
        _ => primary.x + primary.width / 2,
    }
}

/// Top edge of the monitor under `center_x`, or of the primary monitor if none is.
pub fn top_y_for(center_x: i32, monitors: &[Rect], primary: Rect) -> i32 {
    monitors
        .iter()
        .find(|m| m.contains_x(center_x))
        .unwrap_or(&primary)
        .y
}

/// What the widget remembers between runs, in `<config dir>/meeting-notes/widget.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WidgetPrefs {
    /// Horizontal centre of the pill, in physical pixels.
    pub center_x: Option<i32>,
    /// Start-at-login was switched on once. Never switched on again, so the user's choice sticks.
    pub autostart_set: bool,
}

pub fn prefs_path(config_dir: &Path) -> PathBuf {
    config_dir.join("meeting-notes").join("widget.json")
}

impl WidgetPrefs {
    /// A missing or unreadable file gives the defaults.
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, self.to_json())
    }

    /// Never fails: anything that is not a JSON object, and any key that cannot be read, gives
    /// the default for that key. The file is tiny and flat, so this reads it by hand instead of
    /// pulling in a JSON crate.
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        if !(text.starts_with('{') && text.ends_with('}')) {
            return Self::default();
        }
        Self {
            center_x: raw_value(text, "center_x").and_then(|v| v.parse().ok()),
            autostart_set: raw_value(text, "autostart_set") == Some("true"),
        }
    }

    pub fn to_json(&self) -> String {
        let mut fields = Vec::new();
        if let Some(x) = self.center_x {
            fields.push(format!("\"center_x\": {x}"));
        }
        fields.push(format!("\"autostart_set\": {}", self.autostart_set));
        format!("{{{}}}\n", fields.join(", "))
    }
}

/// The bare text of `"key": value` in a flat JSON object (up to the next `,`, `}` or space).
fn raw_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\"");
    let rest = text[text.find(&needle)? + needle.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let end = rest
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: Rect = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    const RIGHT: Rect = Rect {
        x: 1920,
        y: 0,
        width: 1920,
        height: 1080,
    };

    #[test]
    fn anchor_keeps_the_centre_when_the_width_changes() {
        let center = 960;
        let folded = anchored_x(center, 180);
        let unfolded = anchored_x(center, 300);
        assert_eq!(folded, 870);
        assert_eq!(unfolded, 810);
        assert_eq!(center_from_x(folded, 180), center);
        assert_eq!(center_from_x(unfolded, 300), center);
    }

    #[test]
    fn anchor_math_handles_odd_widths_and_negative_monitors() {
        assert_eq!(anchored_x(100, 301), 100 - 150);
        assert_eq!(anchored_x(-500, 180), -590);
        assert_eq!(center_from_x(-590, 180), -500);
    }

    #[test]
    fn sizes_are_folded_and_unfolded() {
        assert_eq!(size_for(true), FOLDED_SIZE);
        assert_eq!(size_for(false), UNFOLDED_SIZE);
    }

    #[test]
    fn default_centre_is_the_middle_of_the_primary_monitor() {
        assert_eq!(resolve_center(None, &[LEFT, RIGHT], LEFT), 960);
        assert_eq!(resolve_center(None, &[LEFT, RIGHT], RIGHT), 2880);
    }

    #[test]
    fn saved_centre_on_any_monitor_is_kept() {
        assert_eq!(resolve_center(Some(1200), &[LEFT, RIGHT], LEFT), 1200);
        assert_eq!(resolve_center(Some(3000), &[LEFT, RIGHT], LEFT), 3000);
        assert_eq!(resolve_center(Some(0), &[LEFT, RIGHT], RIGHT), 0);
    }

    #[test]
    fn saved_centre_off_every_monitor_falls_back_to_the_default() {
        assert_eq!(resolve_center(Some(5000), &[LEFT, RIGHT], LEFT), 960);
        assert_eq!(resolve_center(Some(-1), &[LEFT, RIGHT], LEFT), 960);
        // The right edge is exclusive: 3840 is the first pixel past the second monitor.
        assert_eq!(resolve_center(Some(3840), &[LEFT, RIGHT], LEFT), 960);
        // A monitor that was unplugged since the last run.
        assert_eq!(resolve_center(Some(3000), &[LEFT], LEFT), 960);
    }

    #[test]
    fn top_edge_follows_the_monitor_under_the_centre() {
        let raised = Rect { y: -200, ..RIGHT };
        assert_eq!(top_y_for(500, &[LEFT, raised], LEFT), 0);
        assert_eq!(top_y_for(2500, &[LEFT, raised], LEFT), -200);
        assert_eq!(top_y_for(9000, &[LEFT, raised], LEFT), 0);
    }

    #[test]
    fn prefs_round_trip_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = prefs_path(dir.path());
        let prefs = WidgetPrefs {
            center_x: Some(-1234),
            autostart_set: true,
        };
        prefs.save(&path).unwrap();
        assert_eq!(WidgetPrefs::load(&path), prefs);
    }

    #[test]
    fn saved_file_is_plain_json_with_the_documented_keys() {
        let prefs = WidgetPrefs {
            center_x: Some(960),
            autostart_set: false,
        };
        assert_eq!(
            prefs.to_json(),
            "{\"center_x\": 960, \"autostart_set\": false}\n"
        );
        let unset = WidgetPrefs::default().to_json();
        assert_eq!(unset, "{\"autostart_set\": false}\n");
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            WidgetPrefs::load(&prefs_path(dir.path())),
            WidgetPrefs::default()
        );
    }

    #[test]
    fn corrupt_file_gives_defaults() {
        for text in ["", "not json", "[1, 2]", "{\"center_x\": 12", "\u{0}\u{1}"] {
            assert_eq!(WidgetPrefs::parse(text), WidgetPrefs::default(), "{text:?}");
        }
    }

    #[test]
    fn unreadable_values_fall_back_one_key_at_a_time() {
        let p = WidgetPrefs::parse("{\"center_x\": \"left\", \"autostart_set\": true}");
        assert_eq!(p.center_x, None);
        assert!(p.autostart_set);
        let p = WidgetPrefs::parse("{\"center_x\": 700, \"autostart_set\": 1}");
        assert_eq!(p.center_x, Some(700));
        assert!(!p.autostart_set);
        let p = WidgetPrefs::parse("{\"center_x\": 99999999999}");
        assert_eq!(p.center_x, None);
    }

    #[test]
    fn parse_accepts_other_spacing_key_order_and_unknown_keys() {
        let p = WidgetPrefs::parse(
            "{\n  \"autostart_set\":true,\n  \"extra\": 5,\n  \"center_x\" : -40\n}\n",
        );
        assert_eq!(p.center_x, Some(-40));
        assert!(p.autostart_set);
    }

    #[test]
    fn save_creates_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = prefs_path(dir.path());
        assert!(!path.parent().unwrap().exists());
        WidgetPrefs::default().save(&path).unwrap();
        assert!(path.exists());
    }
}
