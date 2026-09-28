use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::paths;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    #[serde(default)]
    pub profile: Vec<ProfileConfig>,
    #[serde(default)]
    pub exclude: ExcludeConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    pub key_file: Option<String>,
    pub remote_base: String,
    #[serde(default = "default_parallel")]
    pub parallel: usize,
}

fn default_parallel() -> usize {
    4
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExcludeConfig {
    #[serde(default)]
    pub extra_patterns: Vec<String>,
}

fn default_port() -> u16 {
    22
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = paths::config_file()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;

        if !path.exists() {
            anyhow::bail!(
                "Config file not found: {}\nRun `modrinth-sync config init` to create one.",
                path.display()
            );
        }

        let content = std::fs::read_to_string(&path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = paths::config_file()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        println!("Config saved to {}", path.display());
        Ok(())
    }

    /// Resolve the key_file path, expanding ~ to home directory.
    pub fn resolved_key_file(&self) -> Option<PathBuf> {
        self.server.key_file.as_ref().map(|kf| {
            if kf.starts_with("~/") {
                if let Some(home) = dirs::home_dir() {
                    return home.join(&kf[2..]);
                }
            }
            PathBuf::from(kf)
        })
    }
}

pub fn init_interactive() -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();

    let prompt = |msg: &str, default: &str, lines: &mut io::Lines<io::StdinLock>| -> String {
        if default.is_empty() {
            print!("{}: ", msg);
        } else {
            print!("{} [{}]: ", msg, default);
        }
        io::stdout().flush().unwrap();
        let line = lines.next().unwrap_or(Ok(String::new())).unwrap_or_default();
        let line = line.trim().to_string();
        if line.is_empty() {
            default.to_string()
        } else {
            line
        }
    };

    println!("=== modrinth-sync config setup ===\n");

    let host = prompt("SSH host", "", &mut lines);
    let port = prompt("SSH port", "22", &mut lines);
    let user = prompt("SSH user", "", &mut lines);
    let key_file = prompt("SSH key file (optional, blank to skip)", "", &mut lines);
    let remote_base = prompt(
        "Remote base directory",
        &format!("/home/{}/modrinth-sync", user),
        &mut lines,
    );

    println!("\nAvailable local profiles:");
    match paths::list_local_profiles() {
        Ok(profiles) => {
            for (i, name) in profiles.iter().enumerate() {
                println!("  {}. {}", i + 1, name);
            }
            println!();
            let selection = prompt(
                "Profiles to sync (comma-separated numbers, or 'all')",
                "all",
                &mut lines,
            );

            let selected: Vec<ProfileConfig> = if selection == "all" {
                profiles.into_iter().map(|name| ProfileConfig { name }).collect()
            } else {
                selection
                    .split(',')
                    .filter_map(|s| s.trim().parse::<usize>().ok())
                    .filter_map(|i| profiles.get(i - 1).map(|name| ProfileConfig { name: name.clone() }))
                    .collect()
            };

            let config = Config {
                server: ServerConfig {
                    host,
                    port: port.parse().unwrap_or(22),
                    user,
                    key_file: if key_file.is_empty() {
                        None
                    } else {
                        Some(key_file)
                    },
                    remote_base,
                    parallel: default_parallel(),
                },
                profile: selected,
                exclude: ExcludeConfig::default(),
            };

            config.save()?;
        }
        Err(e) => {
            println!("  Warning: could not list profiles: {}", e);
            println!("  You can add profiles manually later with `config add-profile`.\n");

            let config = Config {
                server: ServerConfig {
                    host,
                    port: port.parse().unwrap_or(22),
                    user,
                    key_file: if key_file.is_empty() {
                        None
                    } else {
                        Some(key_file)
                    },
                    remote_base,
                    parallel: default_parallel(),
                },
                profile: vec![],
                exclude: ExcludeConfig::default(),
            };

            config.save()?;
        }
    }

    Ok(())
}
