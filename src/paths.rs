use std::path::PathBuf;

/// Returns the Modrinth App profiles directory for the current platform.
pub fn modrinth_profiles_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        dirs::data_dir().map(|d| d.join("ModrinthApp").join("profiles"))
    }
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir().map(|d| d.join("ModrinthApp").join("profiles"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        dirs::data_dir().map(|d| d.join("ModrinthApp").join("profiles"))
    }
}

/// Returns the config directory for modrinth-sync.
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("modrinth-sync"))
}

/// Returns the path to the config file.
pub fn config_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.toml"))
}

/// Lists available profile names from the Modrinth profiles directory.
pub fn list_local_profiles() -> anyhow::Result<Vec<String>> {
    let profiles_dir = modrinth_profiles_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine Modrinth profiles directory"))?;

    if !profiles_dir.exists() {
        anyhow::bail!(
            "Modrinth profiles directory not found: {}",
            profiles_dir.display()
        );
    }

    let mut profiles = Vec::new();
    for entry in std::fs::read_dir(&profiles_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                profiles.push(name.to_string());
            }
        }
    }
    profiles.sort();
    Ok(profiles)
}
