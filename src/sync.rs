use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use indicatif::{ProgressBar, ProgressStyle};

use crate::config::Config;
use crate::manifest::{self, DiffResult, Manifest};
use crate::paths;
use crate::sftp::SftpClient;

/// A file transfer job for the worker threads.
struct TransferJob {
    /// Manifest key (e.g. "profile/mods/mod.jar")
    key: String,
    /// Local file path
    local_path: PathBuf,
    /// Remote relative path (e.g. "profiles/profile/mods/mod.jar")
    remote_rel: String,
    /// File size in bytes
    size: u64,
}

pub fn push(force: bool, profile_filter: Option<&str>, dry_run: bool) -> anyhow::Result<()> {
    let config = Config::load()?;
    let profiles_dir = profiles_dir()?;
    let profiles = resolve_profiles(&config, profile_filter)?;

    // Single connection for manifest operations
    let client = SftpClient::connect(&config)?;

    let mut manifest = load_remote_manifest(&client)?;

    let current_host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());

    if !force && !manifest.files.is_empty() && manifest.last_sync_host != current_host {
        println!(
            "Warning: Remote was last updated from '{}' at {}.",
            manifest.last_sync_host, manifest.last_sync
        );
        println!("Use --force to overwrite, or run `pull` first.");
        anyhow::bail!("Conflict detected. Use --force to override.");
    }

    let mut total_uploaded = 0usize;
    let mut total_deleted = 0usize;
    let mut total_unchanged = 0usize;

    for profile_name in &profiles {
        let profile_dir = profiles_dir.join(profile_name);
        if !profile_dir.exists() {
            log::warn!("Profile directory not found: {}", profile_dir.display());
            continue;
        }

        println!("\n--- Profile: {} ---", profile_name);

        let local_files =
            manifest::scan_local_files(&profile_dir, profile_name, &config.exclude.extra_patterns)?;

        let diff = manifest::diff(&local_files, &manifest, profile_name);

        print_diff_summary(&diff);

        if dry_run {
            print_diff_details(&diff);
            continue;
        }

        // Upload changed files in parallel
        if !diff.changed.is_empty() {
            let jobs: Vec<TransferJob> = diff
                .changed
                .iter()
                .map(|key| {
                    let rel_in_profile =
                        key.strip_prefix(&format!("{}/", profile_name)).unwrap();
                    TransferJob {
                        key: key.clone(),
                        local_path: profile_dir.join(rel_in_profile),
                        remote_rel: format!("profiles/{}", key),
                        size: local_files.get(key).map(|e| e.size).unwrap_or(0),
                    }
                })
                .collect();

            let total_bytes: u64 = jobs.iter().map(|j| j.size).sum();
            let pb = byte_progress_bar(total_bytes, "Uploading");

            run_parallel_uploads(&config, &jobs, &pb)?;

            pb.finish_with_message(format!("{} files", diff.changed.len()));

            // Update manifest entries
            for key in &diff.changed {
                if let Some(entry) = local_files.get(key) {
                    manifest.files.insert(key.clone(), entry.clone());
                }
            }
            total_uploaded += diff.changed.len();
        }

        // Deletes are fast, no need to parallelize
        if !diff.deleted.is_empty() {
            let pb = count_progress_bar(diff.deleted.len() as u64, "Deleting");
            for key in &diff.deleted {
                let remote_rel = format!("profiles/{}", key);
                client.delete_file(&remote_rel)?;
                manifest.files.remove(key);
                pb.inc(1);
            }
            pb.finish_with_message("done");
            total_deleted += diff.deleted.len();
        }

        total_unchanged += diff.unchanged.len();
    }

    if !dry_run {
        manifest.last_sync = chrono::Utc::now();
        manifest.last_sync_host = current_host;
        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        client.upload("manifest.json", manifest_json.as_bytes())?;

        println!(
            "\nPush complete: {} uploaded, {} deleted, {} unchanged.",
            total_uploaded, total_deleted, total_unchanged
        );
    }

    Ok(())
}

pub fn pull(profile_filter: Option<&str>, dry_run: bool) -> anyhow::Result<()> {
    let config = Config::load()?;
    let profiles_dir = profiles_dir()?;
    let profiles = resolve_profiles(&config, profile_filter)?;

    let client = SftpClient::connect(&config)?;

    let manifest = load_remote_manifest(&client)?;
    if manifest.files.is_empty() {
        println!("Remote is empty. Nothing to pull.");
        return Ok(());
    }

    println!(
        "Remote last synced from '{}' at {}.",
        manifest.last_sync_host, manifest.last_sync
    );

    let mut total_downloaded = 0usize;
    let mut total_deleted = 0usize;
    let mut total_unchanged = 0usize;

    for profile_name in &profiles {
        let profile_dir = profiles_dir.join(profile_name);
        println!("\n--- Profile: {} ---", profile_name);

        let local_files =
            manifest::scan_local_files(&profile_dir, profile_name, &config.exclude.extra_patterns)?;

        let diff = manifest::diff_for_pull(&local_files, &manifest, profile_name);

        print_diff_summary(&diff);

        if dry_run {
            print_diff_details(&diff);
            continue;
        }

        // Download new/changed files in parallel
        if !diff.changed.is_empty() {
            let jobs: Vec<TransferJob> = diff
                .changed
                .iter()
                .map(|key| {
                    let rel_in_profile =
                        key.strip_prefix(&format!("{}/", profile_name)).unwrap();
                    TransferJob {
                        key: key.clone(),
                        local_path: profile_dir.join(rel_in_profile),
                        remote_rel: format!("profiles/{}", key),
                        size: manifest.files.get(key).map(|e| e.size).unwrap_or(0),
                    }
                })
                .collect();

            let total_bytes: u64 = jobs.iter().map(|j| j.size).sum();
            let pb = byte_progress_bar(total_bytes, "Downloading");

            run_parallel_downloads(&config, &jobs, &pb)?;

            pb.finish_with_message(format!("{} files", diff.changed.len()));
            total_downloaded += diff.changed.len();
        }

        // Delete local files not in manifest
        if !diff.deleted.is_empty() {
            let pb = count_progress_bar(diff.deleted.len() as u64, "Cleaning up");
            for key in &diff.deleted {
                let rel_in_profile = key.strip_prefix(&format!("{}/", profile_name)).unwrap();
                let local_path = profile_dir.join(rel_in_profile);
                if local_path.exists() {
                    std::fs::remove_file(&local_path)?;
                }
                pb.inc(1);
            }
            pb.finish_with_message("done".to_string());
            total_deleted += diff.deleted.len();
        }

        total_unchanged += diff.unchanged.len();
    }

    if !dry_run {
        println!(
            "\nPull complete: {} downloaded, {} deleted, {} unchanged.",
            total_downloaded, total_deleted, total_unchanged
        );
    }

    Ok(())
}

/// Open N SSH connections upfront (before the progress bar starts).
fn connect_workers(config: &Config, count: usize) -> anyhow::Result<Vec<SftpClient>> {
    println!("  Opening {} connections...", count);
    let mut clients = Vec::with_capacity(count);
    for _ in 0..count {
        clients.push(SftpClient::connect_with_retry(config, true)?);
    }
    Ok(clients)
}

/// Run jobs in parallel across pre-connected workers.
fn run_parallel<F>(
    clients: Vec<SftpClient>,
    jobs: &[TransferJob],
    pb: &ProgressBar,
    work_fn: F,
) -> anyhow::Result<()>
where
    F: Fn(&SftpClient, &TransferJob, &ProgressBar) -> anyhow::Result<()> + Sync + Send,
{
    let job_index = Arc::new(Mutex::new(0usize));
    let had_error = Arc::new(AtomicBool::new(false));
    let first_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let work_fn = &work_fn;
    std::thread::scope(|s| {
        let handles: Vec<_> = clients
            .iter()
            .map(|client| {
                let job_index = Arc::clone(&job_index);
                let had_error = Arc::clone(&had_error);
                let first_error = Arc::clone(&first_error);

                s.spawn(move || {
                    loop {
                        if had_error.load(Ordering::Relaxed) {
                            break;
                        }

                        let idx = {
                            let mut i = job_index.lock().unwrap();
                            let current = *i;
                            if current >= jobs.len() {
                                break;
                            }
                            *i += 1;
                            current
                        };

                        let job = &jobs[idx];
                        pb.set_message(short_filename(&job.key));

                        if let Err(e) = work_fn(client, job, pb) {
                            log::error!("Transfer failed for {}: {}", job.key, e);
                            had_error.store(true, Ordering::Relaxed);
                            let mut err = first_error.lock().unwrap();
                            if err.is_none() {
                                *err = Some(format!("Transfer failed for {}: {}", job.key, e));
                            }
                            break;
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("Worker thread panicked");
        }
    });

    if had_error.load(Ordering::Relaxed) {
        let err = first_error.lock().unwrap().clone().unwrap_or_else(|| "Unknown error".to_string());
        anyhow::bail!(err);
    }

    Ok(())
}

/// Upload files in parallel using pre-connected SSH connections.
fn run_parallel_uploads(
    config: &Config,
    jobs: &[TransferJob],
    pb: &ProgressBar,
) -> anyhow::Result<()> {
    let n_workers = config.server.parallel.min(jobs.len()).max(1);
    let clients = connect_workers(config, n_workers)?;

    run_parallel(clients, jobs, pb, |client, job, pb| {
        client.upload_file(&job.local_path, &job.remote_rel, pb)
    })
}

/// Download files in parallel using pre-connected SSH connections.
fn run_parallel_downloads(
    config: &Config,
    jobs: &[TransferJob],
    pb: &ProgressBar,
) -> anyhow::Result<()> {
    let n_workers = config.server.parallel.min(jobs.len()).max(1);

    // Pre-create all parent directories to avoid races
    for job in jobs {
        if let Some(parent) = job.local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let clients = connect_workers(config, n_workers)?;

    run_parallel(clients, jobs, pb, |client, job, pb| {
        client.download_file(&job.remote_rel, &job.local_path, pb)
    })
}

fn profiles_dir() -> anyhow::Result<PathBuf> {
    paths::modrinth_profiles_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine Modrinth profiles directory"))
}

fn resolve_profiles(config: &Config, filter: Option<&str>) -> anyhow::Result<Vec<String>> {
    if let Some(name) = filter {
        if config.profile.iter().any(|p| p.name == name) {
            Ok(vec![name.to_string()])
        } else {
            anyhow::bail!(
                "Profile '{}' not found in config. Configured profiles: {}",
                name,
                config
                    .profile
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    } else {
        Ok(config.profile.iter().map(|p| p.name.clone()).collect())
    }
}

fn load_remote_manifest(client: &SftpClient) -> anyhow::Result<Manifest> {
    match client.download("manifest.json")? {
        Some(data) => {
            let manifest: Manifest = serde_json::from_slice(&data)?;
            Ok(manifest)
        }
        None => {
            log::info!("No remote manifest found. Starting fresh.");
            Ok(Manifest::default())
        }
    }
}

fn print_diff_summary(diff: &DiffResult) {
    println!(
        "  {} changed, {} deleted, {} unchanged",
        diff.changed.len(),
        diff.deleted.len(),
        diff.unchanged.len()
    );
}

fn print_diff_details(diff: &DiffResult) {
    for key in &diff.changed {
        println!("  + {}", key);
    }
    for key in &diff.deleted {
        println!("  - {}", key);
    }
}

fn byte_progress_bar(total_bytes: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(&format!(
                "  {} {{bar:30}} {{percent:>3}}% {{bytes}}/{{total_bytes}} [{{elapsed_precise}}] {{bytes_per_sec}} {{msg}}",
                prefix
            ))
            .unwrap()
            .progress_chars("=> "),
    );
    pb
}

fn count_progress_bar(len: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(&format!(
                "  {} {{bar:30}} {{pos}}/{{len}} {{msg}}",
                prefix
            ))
            .unwrap()
            .progress_chars("=> "),
    );
    pb
}

fn short_filename(path: &str) -> String {
    path.rsplit('/')
        .next()
        .or_else(|| path.rsplit('\\').next())
        .unwrap_or(path)
        .to_string()
}
