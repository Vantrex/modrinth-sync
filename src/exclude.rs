use std::path::Path;

/// Directories to exclude at the profile root level.
const EXCLUDED_DIRS: &[&str] = &[
    "saves",
    "screenshots",
    "crash-reports",
    "logs",
    ".fabric",
    ".mixin.out",
    ".firmament",
    ".autoupdates",
    "cache",
    "data",
    "debug",
    "downloads",
    "essential",
    "moddata",
    "datapacks",
];

/// Files to exclude at the profile root level.
const EXCLUDED_FILES: &[&str] = &[
    "usercache.json",
    "realms_persistence.json",
    "command_history.txt",
    "debug-profile.json",
];

/// Directories included for sync (relative to profile root).
const INCLUDED_DIRS: &[&str] = &["mods", "config"];

/// Files included for sync (relative to profile root).
const INCLUDED_FILES: &[&str] = &["options.txt", "servers.dat", "servers.dat_old"];

/// Subdirectory names that are always excluded inside config/.
const EXCLUDED_CONFIG_SUBDIRS: &[&str] = &["backup", "backups", "logs"];

/// Check if a relative path (from profile root) should be included in sync.
pub fn should_include(relative_path: &Path, extra_excludes: &[String]) -> bool {
    let path_str = relative_path.to_string_lossy();

    // Check extra user-defined exclusions first
    for pattern in extra_excludes {
        let pattern = pattern.trim_end_matches('/');
        if path_str.starts_with(pattern) || path_str.contains(&format!("/{}/", pattern)) {
            return false;
        }
        // Glob-like suffix match for patterns like "*-repo-cache"
        if pattern.starts_with('*') {
            let suffix = &pattern[1..];
            for component in relative_path.components() {
                let comp = component.as_os_str().to_string_lossy();
                if comp.ends_with(suffix) {
                    return false;
                }
            }
        }
    }

    let components: Vec<&str> = relative_path
        .components()
        .map(|c| c.as_os_str().to_str().unwrap_or(""))
        .collect();

    if components.is_empty() {
        return false;
    }

    let top_level = components[0];

    // Check if top-level is an included directory
    if INCLUDED_DIRS.contains(&top_level) {
        // For config/, check excluded subdirectories
        if top_level == "config" && components.len() > 1 {
            for component in &components[1..] {
                if EXCLUDED_CONFIG_SUBDIRS.contains(component) {
                    return false;
                }
            }
        }
        return true;
    }

    // Check if it's an included root-level file
    if components.len() == 1 && INCLUDED_FILES.contains(&top_level) {
        return true;
    }

    // Check hardcoded exclusions (for anything not in include list)
    if EXCLUDED_DIRS.contains(&top_level) || EXCLUDED_FILES.contains(&top_level) {
        return false;
    }

    // Exclude repo-cache directories
    for component in &components {
        if component.ends_with("-repo-cache") {
            return false;
        }
    }

    // Exclude log files at root
    if components.len() == 1 && top_level.ends_with(".log") {
        return false;
    }

    // Exclude console output files
    if components.len() == 1 && top_level.starts_with("console_output_") {
        return false;
    }

    // By default, only include what's in the include lists
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_includes_mods() {
        assert!(should_include(Path::new("mods/fabric-api.jar"), &[]));
    }

    #[test]
    fn test_includes_config() {
        assert!(should_include(Path::new("config/mod.toml"), &[]));
    }

    #[test]
    fn test_excludes_config_backup() {
        assert!(!should_include(Path::new("config/skyhanni/backup/old.json"), &[]));
    }

    #[test]
    fn test_includes_root_files() {
        assert!(should_include(Path::new("options.txt"), &[]));
        assert!(should_include(Path::new("servers.dat"), &[]));
    }

    #[test]
    fn test_excludes_saves() {
        assert!(!should_include(Path::new("saves/world1/level.dat"), &[]));
    }

    #[test]
    fn test_excludes_screenshots() {
        assert!(!should_include(Path::new("screenshots/img.png"), &[]));
    }

    #[test]
    fn test_extra_excludes() {
        let extras = vec!["config/skyhanni/logs".to_string()];
        assert!(!should_include(Path::new("config/skyhanni/logs/log.txt"), &extras));
    }
}
