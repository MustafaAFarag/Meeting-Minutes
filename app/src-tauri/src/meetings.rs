use std::fs;
use std::path::{Path, PathBuf};

/// Where the pilot writes the My Meetings link when `artifact_url` is not in the config.
pub fn url_file(config_dir: &Path) -> PathBuf {
    config_dir.join("meeting-notes").join("artifact_url.txt")
}

/// The My Meetings URL: the config value wins, else the first line of `artifact_url.txt`
/// next to the config. Only http(s) links are accepted, because the result is handed to
/// the system opener.
pub fn meetings_url(
    config_value: Option<&str>,
    config_dir: Option<&Path>,
) -> Result<String, String> {
    if let Some(url) = config_value.map(str::trim).filter(|u| !u.is_empty()) {
        return check_scheme(url);
    }
    let config_dir = config_dir.ok_or("Could not find the user config folder.")?;
    let path = url_file(config_dir);
    let text = fs::read_to_string(&path).map_err(|e| {
        format!(
            "No My Meetings link yet. Put it in {} ({e})",
            path.display()
        )
    })?;
    let first = text.lines().next().map(str::trim).unwrap_or_default();
    if first.is_empty() {
        return Err(format!(
            "The My Meetings link file is empty: {}",
            path.display()
        ));
    }
    check_scheme(first)
}

fn check_scheme(url: &str) -> Result<String, String> {
    if url.starts_with("https://") || url.starts_with("http://") {
        Ok(url.to_string())
    } else {
        Err(format!("The My Meetings link is not a web address: {url}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_url_file(dir: &Path, text: &str) {
        let path = url_file(dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn config_value_wins_over_file() {
        let dir = tempfile::tempdir().unwrap();
        write_url_file(dir.path(), "https://file.example/meetings\n");
        let url = meetings_url(Some("https://config.example/m"), Some(dir.path())).unwrap();
        assert_eq!(url, "https://config.example/m");
    }

    #[test]
    fn file_is_the_fallback_and_only_the_first_line_counts() {
        let dir = tempfile::tempdir().unwrap();
        write_url_file(
            dir.path(),
            "  https://file.example/meetings  \nsecond line\n",
        );
        let url = meetings_url(None, Some(dir.path())).unwrap();
        assert_eq!(url, "https://file.example/meetings");
    }

    #[test]
    fn blank_config_value_falls_back_to_file() {
        let dir = tempfile::tempdir().unwrap();
        write_url_file(dir.path(), "https://file.example/meetings");
        let url = meetings_url(Some("   "), Some(dir.path())).unwrap();
        assert_eq!(url, "https://file.example/meetings");
    }

    #[test]
    fn missing_file_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = meetings_url(None, Some(dir.path())).unwrap_err();
        assert!(err.contains("No My Meetings link yet"), "{err}");
        assert!(err.contains("artifact_url.txt"), "{err}");
    }

    #[test]
    fn empty_file_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        write_url_file(dir.path(), "\n");
        let err = meetings_url(None, Some(dir.path())).unwrap_err();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn no_config_dir_is_an_error() {
        assert!(meetings_url(None, None).is_err());
    }

    #[test]
    fn non_web_links_are_refused() {
        assert!(meetings_url(Some("file:///etc/passwd"), None).is_err());
        assert!(meetings_url(Some("javascript:alert(1)"), None).is_err());
    }
}
