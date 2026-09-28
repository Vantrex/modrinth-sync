use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

use indicatif::ProgressBar;
use ssh2::{Session, Sftp};

use crate::config::Config;

const CHUNK_SIZE: usize = 65536;

pub struct SftpClient {
    sftp: Sftp,
    remote_base: String,
    #[allow(dead_code)]
    session: Session,
}

const MAX_CONNECT_RETRIES: u32 = 5;
const RETRY_BASE_MS: u64 = 200;

impl SftpClient {
    pub fn connect(config: &Config) -> anyhow::Result<Self> {
        Self::connect_with_retry(config, false)
    }

    /// Connect with retry logic for parallel worker connections.
    /// When `is_worker` is true, retries on connection/handshake failures
    /// (e.g. "Failed getting banner" from server connection limits).
    pub fn connect_with_retry(config: &Config, is_worker: bool) -> anyhow::Result<Self> {
        let addr = format!("{}:{}", config.server.host, config.server.port);
        let max_retries = if is_worker { MAX_CONNECT_RETRIES } else { 1 };

        for attempt in 1..=max_retries {
            match Self::try_connect(config, &addr) {
                Ok(client) => return Ok(client),
                Err(e) if is_worker && attempt < max_retries => {
                    let delay = RETRY_BASE_MS * 2u64.pow(attempt - 1);
                    log::debug!(
                        "Connection attempt {}/{} failed: {}. Retrying in {}ms...",
                        attempt, max_retries, e, delay
                    );
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                }
                Err(e) => return Err(e),
            }
        }

        unreachable!()
    }

    fn try_connect(config: &Config, addr: &str) -> anyhow::Result<Self> {
        log::debug!("Connecting to {}...", addr);

        let tcp = TcpStream::connect(addr)?;
        let mut session = Session::new()?;
        session.set_tcp_stream(tcp);
        session.handshake()?;

        // Try auth methods in order: agent, key file, password
        if !try_agent_auth(&session, &config.server.user)
            && !try_key_auth(&session, config)
            && !try_password_auth(&session, &config.server.user, &config.server.host)
        {
            anyhow::bail!(
                "SSH authentication failed for {}@{}. Tried: ssh-agent, key files (~/.ssh/id_*), password.\n\
                 Tip: run with -v to see which methods were attempted and why they failed.",
                config.server.user,
                config.server.host
            );
        }

        log::debug!("Connected and authenticated.");
        let sftp = session.sftp()?;

        Ok(SftpClient {
            sftp,
            remote_base: config.server.remote_base.clone(),
            session,
        })
    }

    /// Full remote path for a relative path within the sync storage.
    pub fn remote_path(&self, relative: &str) -> PathBuf {
        PathBuf::from(&self.remote_base).join(relative)
    }

    /// Download a file from the remote. Returns None if file doesn't exist.
    pub fn download(&self, remote_relative: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let path = self.remote_path(remote_relative);
        match self.sftp.open(&path) {
            Ok(mut file) => {
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)?;
                Ok(Some(buf))
            }
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Upload data to a remote file, creating parent directories as needed.
    pub fn upload(&self, remote_relative: &str, data: &[u8]) -> anyhow::Result<()> {
        let path = self.remote_path(remote_relative);
        self.mkdir_recursive(path.parent().unwrap())?;

        let mut file = self.sftp.create(&path)?;
        file.write_all(data)?;
        Ok(())
    }

    /// Upload a local file to the remote, reporting per-chunk progress.
    pub fn upload_file(
        &self,
        local_path: &Path,
        remote_relative: &str,
        pb: &ProgressBar,
    ) -> anyhow::Result<()> {
        let path = self.remote_path(remote_relative);
        self.mkdir_recursive(path.parent().unwrap())?;

        let mut local_file = std::fs::File::open(local_path)?;
        let mut remote_file = self.sftp.create(&path)?;
        let mut buf = [0u8; CHUNK_SIZE];
        loop {
            let n = local_file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            remote_file.write_all(&buf[..n])?;
            pb.inc(n as u64);
        }
        Ok(())
    }

    /// Download a remote file to a local path, reporting per-chunk progress.
    pub fn download_file(
        &self,
        remote_relative: &str,
        local_path: &Path,
        pb: &ProgressBar,
    ) -> anyhow::Result<()> {
        let path = self.remote_path(remote_relative);
        let mut remote_file = self.sftp.open(&path)?;

        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut local_file = std::fs::File::create(local_path)?;
        let mut buf = [0u8; CHUNK_SIZE];
        loop {
            let n = remote_file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            local_file.write_all(&buf[..n])?;
            pb.inc(n as u64);
        }
        Ok(())
    }

    /// Delete a remote file.
    pub fn delete_file(&self, remote_relative: &str) -> anyhow::Result<()> {
        let path = self.remote_path(remote_relative);
        match self.sftp.unlink(&path) {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Recursively create remote directories.
    pub fn mkdir_recursive(&self, path: &Path) -> anyhow::Result<()> {
        let mut current = PathBuf::new();
        for component in path.components() {
            current.push(component);
            // Try to create, ignore "already exists" errors
            match self.sftp.mkdir(&current, 0o755) {
                Ok(()) => {}
                Err(e) if is_already_exists(&e) => {}
                // Also ignore "not found"-like errors for intermediate paths
                // that may already exist as part of the base path
                Err(e) if is_failure(&e) => {
                    // Check if it already exists by trying to stat
                    if self.sftp.stat(&current).is_ok() {
                        continue;
                    }
                    return Err(e.into());
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// List files recursively under a remote directory, returning relative paths.
    #[allow(dead_code)]
    pub fn list_files_recursive(&self, remote_dir_relative: &str) -> anyhow::Result<Vec<String>> {
        let base = self.remote_path(remote_dir_relative);
        let mut result = Vec::new();
        self.list_recursive_inner(&base, &base, &mut result)?;
        Ok(result)
    }

    fn list_recursive_inner(
        &self,
        current: &Path,
        base: &Path,
        result: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        let entries = match self.sftp.readdir(current) {
            Ok(entries) => entries,
            Err(e) if is_not_found(&e) => return Ok(()),
            Err(e) => return Err(e.into()),
        };

        for (path, stat) in entries {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if name == "." || name == ".." {
                continue;
            }
            if stat.is_dir() {
                self.list_recursive_inner(&path, base, result)?;
            } else {
                if let Ok(rel) = path.strip_prefix(base) {
                    result.push(rel.to_string_lossy().to_string());
                }
            }
        }
        Ok(())
    }
}

fn try_agent_auth(session: &Session, user: &str) -> bool {
    log::debug!("Trying SSH agent auth...");
    match session.userauth_agent(user) {
        Ok(()) if session.authenticated() => {
            log::debug!("SSH agent auth succeeded.");
            true
        }
        _ => false,
    }
}

fn try_key_auth(session: &Session, config: &Config) -> bool {
    // Build list of key paths to try: explicit config first, then defaults
    let mut key_paths: Vec<std::path::PathBuf> = Vec::new();

    if let Some(key_path) = config.resolved_key_file() {
        key_paths.push(key_path);
    }

    // Add default key locations
    if let Some(home) = dirs::home_dir() {
        let ssh_dir = home.join(".ssh");
        for name in &["id_ed25519", "id_rsa", "id_ecdsa"] {
            let p = ssh_dir.join(name);
            if !key_paths.contains(&p) {
                key_paths.push(p);
            }
        }
    }

    for key_path in &key_paths {
        if !key_path.exists() {
            continue;
        }
        log::debug!("Trying key file auth: {}", key_path.display());
        // First try without passphrase
        match session.userauth_pubkey_file(&config.server.user, None, key_path, None) {
            Ok(()) if session.authenticated() => {
                log::debug!("Key file auth succeeded: {}", key_path.display());
                return true;
            }
            Err(e) => {
                log::debug!("Key file {} without passphrase failed: {}", key_path.display(), e);
                // If it failed, try with a passphrase prompt
                if let Ok(passphrase) = rpassword::prompt_password(
                    format!("Passphrase for {}: ", key_path.display())
                ) {
                    if !passphrase.is_empty() {
                        match session.userauth_pubkey_file(
                            &config.server.user, None, key_path, Some(&passphrase)
                        ) {
                            Ok(()) if session.authenticated() => {
                                log::debug!("Key file auth with passphrase succeeded: {}", key_path.display());
                                return true;
                            }
                            Err(e) => {
                                log::debug!("Key file {} with passphrase failed: {}", key_path.display(), e);
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn try_password_auth(session: &Session, user: &str, host: &str) -> bool {
    log::debug!("Trying password auth...");
    match rpassword::prompt_password(format!("SSH password for {}@{}: ", user, host)) {
        Ok(password) => match session.userauth_password(user, &password) {
            Ok(()) if session.authenticated() => {
                log::debug!("Password auth succeeded.");
                true
            }
            _ => false,
        },
        Err(_) => false,
    }
}

fn is_not_found(e: &ssh2::Error) -> bool {
    e.code() == ssh2::ErrorCode::SFTP(2) // SSH_FX_NO_SUCH_FILE
}

fn is_already_exists(e: &ssh2::Error) -> bool {
    e.code() == ssh2::ErrorCode::SFTP(11) // SSH_FX_FILE_ALREADY_EXISTS
}

fn is_failure(e: &ssh2::Error) -> bool {
    e.code() == ssh2::ErrorCode::SFTP(4) // SSH_FX_FAILURE (generic)
}
