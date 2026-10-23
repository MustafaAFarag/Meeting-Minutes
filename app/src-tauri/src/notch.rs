//! The pure parts of the notch: window sizes, where the tab sits on the screen, and the small
//! `widget.json` file that remembers it. Nothing here touches Tauri, so it is all testable.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Logical size of the folded tab (width, height).
pub const FOLDED_SIZE: (f64, f64) = (12.0, 56.0);
/// Logical size of the unfolded column (width, height).
pub const UNFOLDED_SIZE: (f64, f64) = (44.0, 170.0);

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
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    /// Squared distance from a point to the rectangle (0 when inside).
    fn distance2(&self, x: i32, y: i32) -> i64 {
        let dx = (self.x - x).max(0).max(x - (self.x + self.width - 1));
        let dy = (self.y - y).max(0).max(y - (self.y + self.height - 1));
        i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)
    }

    fn clamp_point(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x.clamp(self.x, self.x + self.width - 1),
            y.clamp(self.y, self.y + self.height - 1),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
}

impl Edge {
    pub fn as_str(self) -> &'static str {
        match self {
            Edge::Left => "left",
            Edge::Right => "right",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "left" => Some(Edge::Left),
            "right" => Some(Edge::Right),
            _ => None,
        }
    }
}

/// The monitor that contains the point, if any.
pub fn monitor_at(x: i32, y: i32, monitors: &[Rect]) -> Option<Rect> {
    monitors.iter().find(|m| m.contains(x, y)).copied()
}

/// The side edge of `monitor` that `x` is nearest to. A tie goes to the right edge.
pub fn nearest_edge(x: i32, monitor: Rect) -> Edge {
    if x - monitor.x < monitor.x + monitor.width - x {
        Edge::Left
    } else {
        Edge::Right
    }
}

/// Left coordinate of a window of `width` hanging from `edge` of `monitor`.
pub fn edge_x(edge: Edge, monitor: Rect, width: i32) -> i32 {
    match edge {
        Edge::Left => monitor.x,
        Edge::Right => monitor.x + monitor.width - width,
    }
}

/// Top coordinate of a window of `height` centred on `center_y`, kept fully inside the monitor.
pub fn clamp_top(center_y: i32, height: i32, monitor: Rect) -> i32 {
    let lowest = (monitor.y + monitor.height - height).max(monitor.y);
    (center_y - height / 2).clamp(monitor.y, lowest)
}

/// Where the tab hangs: which edge of which monitor, and the vertical centre it keeps while
/// resizing (physical pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub edge: Edge,
    pub monitor: Rect,
    pub center_y: i32,
}

impl Anchor {
    /// Top-left corner of a window of this size when it sits at the anchor.
    pub fn window_pos(&self, width: i32, height: i32) -> (i32, i32) {
        (
            edge_x(self.edge, self.monitor, width),
            clamp_top(self.center_y, height, self.monitor),
        )
    }

    /// The point that is remembered: the edge, and the drop centre (kept on the monitor).
    pub fn saved(&self, drop_x: i32) -> SavedPos {
        SavedPos {
            edge: self.edge,
            x: drop_x.clamp(self.monitor.x, self.monitor.x + self.monitor.width - 1),
            y: self.center_y,
        }
    }
}

/// The anchor for a tab dropped with its centre at (`x`, `y`): the monitor containing the point
/// (else the nearest one), snapped to that monitor's nearest side edge.
pub fn drop_anchor(x: i32, y: i32, monitors: &[Rect], primary: Rect) -> Anchor {
    let monitor = monitor_at(x, y, monitors)
        .or_else(|| monitors.iter().min_by_key(|m| m.distance2(x, y)).copied())
        .unwrap_or(primary);
    let (x, y) = monitor.clamp_point(x, y);
    Anchor {
        edge: nearest_edge(x, monitor),
        monitor,
        center_y: y,
    }
}

/// Where the tab goes when nothing is saved. Linux cannot hide the widget from screen sharing, so
/// there it starts on a monitor other than the primary one (the one usually shared), if any.
pub fn home_monitor(monitors: &[Rect], primary: Rect, avoid_primary: bool) -> Rect {
    if avoid_primary {
        if let Some(other) = monitors.iter().find(|m| **m != primary) {
            return *other;
        }
    }
    primary
}

/// The saved spot if some monitor still contains it, else the right edge of `home`, vertically
/// centred.
pub fn resolve(saved: Option<SavedPos>, monitors: &[Rect], home: Rect) -> Anchor {
    if let Some(s) = saved {
        if let Some(monitor) = monitor_at(s.x, s.y, monitors) {
            return Anchor {
                edge: s.edge,
                monitor,
                center_y: s.y,
            };
        }
    }
    Anchor {
        edge: Edge::Right,
        monitor: home,
        center_y: home.y + home.height / 2,
    }
}

/// The remembered drop point: the edge it snapped to and the tab's centre (physical pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedPos {
    pub edge: Edge,
    pub x: i32,
    pub y: i32,
}

/// What the widget remembers between runs, in `<config dir>/meeting-notes/widget.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WidgetPrefs {
    pub pos: Option<SavedPos>,
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
    /// the default for that key. The position counts only when `edge`, `x` and `y` are all
    /// readable, so the old top-edge file (`center_x`) simply gives no position. The file is
    /// tiny and flat, so this reads it by hand instead of pulling in a JSON crate.
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        if !(text.starts_with('{') && text.ends_with('}')) {
            return Self::default();
        }
        let pos = (|| {
            Some(SavedPos {
                edge: Edge::parse(
                    raw_value(text, "edge")?
                        .strip_prefix('"')?
                        .strip_suffix('"')?,
                )?,
                x: raw_value(text, "x")?.parse().ok()?,
                y: raw_value(text, "y")?.parse().ok()?,
            })
        })();
        Self {
            pos,
            autostart_set: raw_value(text, "autostart_set") == Some("true"),
        }
    }

    pub fn to_json(&self) -> String {
        let mut fields = Vec::new();
        if let Some(p) = self.pos {
            fields.push(format!(
                "\"edge\": \"{}\", \"x\": {}, \"y\": {}",
                p.edge.as_str(),
                p.x,
                p.y
            ));
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
    fn sizes_are_folded_and_unfolded() {
        assert_eq!(size_for(true), FOLDED_SIZE);
        assert_eq!(size_for(false), UNFOLDED_SIZE);
    }

    #[test]
    fn nearest_edge_is_left_or_right_of_the_monitor() {
        assert_eq!(nearest_edge(10, LEFT), Edge::Left);
        assert_eq!(nearest_edge(959, LEFT), Edge::Left);
        assert_eq!(nearest_edge(961, LEFT), Edge::Right);
        assert_eq!(nearest_edge(1900, LEFT), Edge::Right);
        // Offsets come from the monitor, not from 0.
        assert_eq!(nearest_edge(1930, RIGHT), Edge::Left);
        assert_eq!(nearest_edge(3800, RIGHT), Edge::Right);
        // The exact middle goes right.
        assert_eq!(nearest_edge(960, LEFT), Edge::Right);
    }

    #[test]
    fn drop_picks_the_monitor_under_the_point() {
        let a = drop_anchor(1925, 300, &[LEFT, RIGHT], LEFT);
        assert_eq!((a.edge, a.monitor, a.center_y), (Edge::Left, RIGHT, 300));
        let a = drop_anchor(1915, 300, &[LEFT, RIGHT], LEFT);
        assert_eq!((a.edge, a.monitor), (Edge::Right, LEFT));
    }

    #[test]
    fn home_avoids_the_primary_monitor_only_when_asked_and_possible() {
        assert_eq!(home_monitor(&[LEFT, RIGHT], LEFT, true), RIGHT);
        assert_eq!(home_monitor(&[LEFT, RIGHT], RIGHT, true), LEFT);
        assert_eq!(home_monitor(&[LEFT, RIGHT], LEFT, false), LEFT);
        assert_eq!(home_monitor(&[LEFT], LEFT, true), LEFT);
    }

    #[test]
    fn drop_off_every_monitor_uses_the_nearest_one() {
        let a = drop_anchor(5000, 2000, &[LEFT, RIGHT], LEFT);
        assert_eq!((a.edge, a.monitor, a.center_y), (Edge::Right, RIGHT, 1079));
        let a = drop_anchor(-300, -50, &[LEFT, RIGHT], RIGHT);
        assert_eq!((a.edge, a.monitor, a.center_y), (Edge::Left, LEFT, 0));
        let a = drop_anchor(10, 10, &[], RIGHT);
        assert_eq!(a.monitor, RIGHT);
    }

    #[test]
    fn x_hangs_from_each_edge() {
        assert_eq!(edge_x(Edge::Left, LEFT, 12), 0);
        assert_eq!(edge_x(Edge::Right, LEFT, 12), 1908);
        assert_eq!(edge_x(Edge::Left, RIGHT, 44), 1920);
        assert_eq!(edge_x(Edge::Right, RIGHT, 44), 3796);
    }

    #[test]
    fn x_follows_the_width_so_the_edge_stays_put() {
        let anchor = Anchor {
            edge: Edge::Right,
            monitor: LEFT,
            center_y: 540,
        };
        let (folded_x, _) = anchor.window_pos(12, 56);
        let (open_x, _) = anchor.window_pos(44, 170);
        assert_eq!(folded_x + 12, 1920);
        assert_eq!(open_x + 44, 1920);
    }

    #[test]
    fn y_keeps_the_centre_and_stays_on_screen() {
        assert_eq!(clamp_top(540, 56, LEFT), 512);
        assert_eq!(clamp_top(540, 170, LEFT), 455);
        // Too close to the top or the bottom: pushed back inside.
        assert_eq!(clamp_top(10, 170, LEFT), 0);
        assert_eq!(clamp_top(1075, 170, LEFT), 910);
        assert_eq!(clamp_top(-500, 56, LEFT), 0);
        assert_eq!(clamp_top(9000, 56, LEFT), 1024);
        // A monitor offset on y.
        let raised = Rect { y: -200, ..RIGHT };
        assert_eq!(clamp_top(-190, 56, raised), -200);
        // A window taller than the monitor sticks to the top.
        let tiny = Rect {
            height: 100,
            ..LEFT
        };
        assert_eq!(clamp_top(50, 170, tiny), 0);
    }

    #[test]
    fn default_is_the_right_edge_of_the_primary_monitor_centred() {
        let a = resolve(None, &[LEFT, RIGHT], RIGHT);
        assert_eq!((a.edge, a.monitor, a.center_y), (Edge::Right, RIGHT, 540));
        assert_eq!(a.window_pos(12, 56), (3828, 512));
    }

    #[test]
    fn saved_spot_on_any_monitor_is_kept() {
        let saved = SavedPos {
            edge: Edge::Left,
            x: 1930,
            y: 200,
        };
        let a = resolve(Some(saved), &[LEFT, RIGHT], LEFT);
        assert_eq!((a.edge, a.monitor, a.center_y), (Edge::Left, RIGHT, 200));
    }

    #[test]
    fn saved_spot_off_every_monitor_falls_back_to_the_default() {
        for (x, y) in [(5000, 100), (-1, 100), (100, -1), (100, 1080), (3840, 10)] {
            let saved = SavedPos {
                edge: Edge::Left,
                x,
                y,
            };
            let a = resolve(Some(saved), &[LEFT, RIGHT], LEFT);
            assert_eq!((a.edge, a.monitor, a.center_y), (Edge::Right, LEFT, 540));
        }
        // A monitor that was unplugged since the last run.
        let saved = SavedPos {
            edge: Edge::Right,
            x: 3000,
            y: 100,
        };
        let a = resolve(Some(saved), &[LEFT], LEFT);
        assert_eq!((a.edge, a.monitor, a.center_y), (Edge::Right, LEFT, 540));
    }

    #[test]
    fn what_is_saved_is_restored_to_the_same_spot() {
        for (x, y) in [(3700, 300), (1925, 300)] {
            let dropped = drop_anchor(x, y, &[LEFT, RIGHT], LEFT);
            let saved = dropped.saved(x);
            assert_eq!((saved.x, saved.y), (x, y));
            assert_eq!(resolve(Some(saved), &[LEFT, RIGHT], LEFT), dropped);
        }
    }

    #[test]
    fn prefs_round_trip_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = prefs_path(dir.path());
        let prefs = WidgetPrefs {
            pos: Some(SavedPos {
                edge: Edge::Left,
                x: -1234,
                y: 56,
            }),
            autostart_set: true,
        };
        prefs.save(&path).unwrap();
        assert_eq!(WidgetPrefs::load(&path), prefs);
    }

    #[test]
    fn saved_file_is_plain_json_with_the_documented_keys() {
        let prefs = WidgetPrefs {
            pos: Some(SavedPos {
                edge: Edge::Right,
                x: 3834,
                y: 540,
            }),
            autostart_set: false,
        };
        assert_eq!(
            prefs.to_json(),
            "{\"edge\": \"right\", \"x\": 3834, \"y\": 540, \"autostart_set\": false}\n"
        );
        let unset = WidgetPrefs::default().to_json();
        assert_eq!(unset, "{\"autostart_set\": false}\n");
    }

    #[test]
    fn old_center_x_file_is_read_without_failing_and_ignored() {
        let p = WidgetPrefs::parse("{\"center_x\": 960, \"autostart_set\": true}\n");
        assert_eq!(p.pos, None);
        assert!(p.autostart_set);
        // Saving it again drops the old key.
        assert_eq!(p.to_json(), "{\"autostart_set\": true}\n");
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
        for text in ["", "not json", "[1, 2]", "{\"edge\": \"left", "\u{0}\u{1}"] {
            assert_eq!(WidgetPrefs::parse(text), WidgetPrefs::default(), "{text:?}");
        }
    }

    #[test]
    fn unreadable_values_fall_back() {
        let whole = |edge: &str, x: &str, y: &str| {
            WidgetPrefs::parse(&format!(
                "{{\"edge\": {edge}, \"x\": {x}, \"y\": {y}, \"autostart_set\": true}}"
            ))
        };
        assert!(whole("\"left\"", "10", "20").pos.is_some());
        // Any one bad part drops the position but not the other keys.
        for p in [
            whole("\"top\"", "10", "20"),
            whole("left", "10", "20"),
            whole("\"left\"", "\"ten\"", "20"),
            whole("\"left\"", "10", "99999999999"),
        ] {
            assert_eq!(p.pos, None);
            assert!(p.autostart_set);
        }
        let p = WidgetPrefs::parse("{\"edge\": \"left\", \"x\": 5}");
        assert_eq!(p.pos, None);
        let p = WidgetPrefs::parse("{\"autostart_set\": 1}");
        assert!(!p.autostart_set);
    }

    #[test]
    fn parse_accepts_other_spacing_key_order_and_unknown_keys() {
        let p = WidgetPrefs::parse(
            "{\n  \"autostart_set\":true,\n  \"extra\": 5,\n  \"y\" : -40,\n  \"x\":7,\n  \"edge\":\"right\"\n}\n",
        );
        assert_eq!(
            p.pos,
            Some(SavedPos {
                edge: Edge::Right,
                x: 7,
                y: -40
            })
        );
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
