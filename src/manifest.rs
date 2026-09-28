use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::exclude;

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub last_sync: DateTime<Utc>,
    pub last_sync_host: String,
    pub files: HashMap<String, FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub sha256: String,
    pub size: u64,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: 1,
            last_sync: Utc::now(),
            last_sync_host: current_hostname(),
            files: HashMap::new(),
        }
    }
}

/// The result of diffing local files against the manifest.
pub struct DiffResult {
    /// Files that are new or modified locally (need upload on push, or download on pull).
    pub changed: Vec<String>,
    /// Files that exist in manifest but not locally (deleted locally).
    pub deleted: Vec<String>,
    /// Files that are unchanged.
    pub unchanged: Vec<String>,
}

/// Scan local profile directory and compute SHA-256 hashes for all included files.
/// Returns a map of relative paths (using forward slashes) to FileEntry.
pub fn scan_local_files(
    profile_dir: &Path,
    profile_name: &str,
    extra_excludes: &[String],
) -> anyhow::Result<HashMap<String, FileEntry>> {
    let mut files = HashMap::new();

    if !profile_dir.exists() {
        return Ok(files);
    }

    for entry in WalkDir::new(profile_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let full_path = entry.path();
        let relative_to_profile = full_path.strip_prefix(profile_dir)?;

        if !exclude::should_include(relative_to_profile, extra_excludes) {
            continue;
        }

        // Build manifest key: "profile_name/relative/path" with forward slashes
        let rel_str = relative_to_profile
            .to_string_lossy()
            .replace('\\', "/");
        let manifest_key = format!("{}/{}", profile_name, rel_str);

        let metadata = std::fs::metadata(full_path)?;
        let hash = sha256_file(full_path)?;

        files.insert(
            manifest_key,
            FileEntry {
                sha256: hash,
                size: metadata.len(),
            },
        );
    }

    Ok(files)
}

/// Compute diff between local files and manifest for a given profile.
pub fn diff(
    local_files: &HashMap<String, FileEntry>,
    manifest: &Manifest,
    profile_name: &str,
) -> DiffResult {
    let mut changed = Vec::new();
    let mut unchanged = Vec::new();
    let mut deleted = Vec::new();

    // Check local files against manifest
    for (key, local_entry) in local_files {
        match manifest.files.get(key) {
            Some(remote_entry) if remote_entry.sha256 == local_entry.sha256 => {
                unchanged.push(key.clone());
            }
            _ => {
                changed.push(key.clone());
            }
        }
    }

    // Check manifest files that don't exist locally (deletions)
    let prefix = format!("{}/", profile_name);
    for key in manifest.files.keys() {
        if key.starts_with(&prefix) && !local_files.contains_key(key) {
            deleted.push(key.clone());
        }
    }

    DiffResult {
        changed,
        deleted,
        unchanged,
    }
}

/// Compute diff for pull: find what needs to be downloaded/deleted locally.
pub fn diff_for_pull(
    local_files: &HashMap<String, FileEntry>,
    manifest: &Manifest,
    profile_name: &str,
) -> DiffResult {
    let mut changed = Vec::new();
    let mut unchanged = Vec::new();
    let mut deleted = Vec::new();

    let prefix = format!("{}/", profile_name);

    // Files in manifest that are new or changed compared to local
    for (key, remote_entry) in &manifest.files {
        if !key.starts_with(&prefix) {
            continue;
        }
        match local_files.get(key) {
            Some(local_entry) if local_entry.sha256 == remote_entry.sha256 => {
                unchanged.push(key.clone());
            }
            _ => {
                changed.push(key.clone());
            }
        }
    }

    // Local files not in manifest (were deleted on the other machine)
    for key in local_files.keys() {
        if !manifest.files.contains_key(key) {
            deleted.push(key.clone());
        }
    }

    DiffResult {
        changed,
        deleted,
        unchanged,
    }
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let data = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let result = hasher.finalize();
    Ok(hex::encode(result))
}

fn current_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string())
}
