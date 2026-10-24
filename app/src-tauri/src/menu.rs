//! The tray menu, as plain data, so its order can be tested without a window system.

pub struct Entry {
    pub id: &'static str,
    pub label: &'static str,
    /// Drawn with a check mark (Start at login).
    pub checkable: bool,
}

const fn entry(id: &'static str, label: &'static str) -> Entry {
    Entry {
        id,
        label,
        checkable: false,
    }
}

/// Top to bottom. "Update to vX" is added later, in front of "Open log folder".
pub const TRAY: [Entry; 6] = [
    entry("show", "Show"),
    entry("hide", "Hide"),
    Entry {
        id: "autostart",
        label: "Start at login",
        checkable: true,
    },
    entry("open_logs", "Open log folder"),
    entry("report", "Report a problem"),
    entry("quit", "Quit"),
];

/// Where the "Update to vX" item goes: after Start at login, before the help items and Quit.
pub fn update_insert_index() -> usize {
    TRAY.iter()
        .position(|item| item.id == "open_logs")
        .unwrap_or(TRAY.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Vec<&'static str> {
        TRAY.iter().map(|item| item.id).collect()
    }

    #[test]
    fn the_tray_keeps_its_old_items_and_adds_the_help_items() {
        assert_eq!(
            ids(),
            ["show", "hide", "autostart", "open_logs", "report", "quit"]
        );
    }

    #[test]
    fn report_a_problem_is_right_above_quit() {
        let ids = ids();
        assert_eq!(ids[ids.len() - 1], "quit");
        assert_eq!(ids[ids.len() - 2], "report");
        let label = |id: &str| TRAY.iter().find(|i| i.id == id).map(|i| i.label);
        assert_eq!(label("report"), Some("Report a problem"));
        assert_eq!(label("open_logs"), Some("Open log folder"));
    }

    #[test]
    fn the_update_item_goes_between_start_at_login_and_the_help_items() {
        let at = update_insert_index();
        assert_eq!(TRAY[at - 1].id, "autostart");
        assert_eq!(TRAY[at].id, "open_logs");
    }

    #[test]
    fn only_start_at_login_has_a_check_mark() {
        let checked: Vec<_> = TRAY.iter().filter(|i| i.checkable).map(|i| i.id).collect();
        assert_eq!(checked, ["autostart"]);
    }
}
