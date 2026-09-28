mod config;
mod exclude;
mod manifest;
mod paths;
mod sftp;
mod sync;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "modrinth-sync", about = "Sync Modrinth profiles between machines via SFTP")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Upload local profile changes to remote server
    Push {
        /// Override conflict detection
        #[arg(long)]
        force: bool,
        /// Only sync a specific profile
        #[arg(long)]
        profile: Option<String>,
        /// Show what would happen without making changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Download remote profile changes to local machine
    Pull {
        /// Only sync a specific profile
        #[arg(long)]
        profile: Option<String>,
        /// Show what would happen without making changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Manage configuration
    #[command(subcommand)]
    Config(ConfigCommands),
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Interactive configuration setup
    Init,
    /// Show current configuration
    Show,
    /// Add a profile to sync
    AddProfile {
        /// Profile name (must match Modrinth profile directory name)
        name: String,
    },
    /// List available local Modrinth profiles
    ListProfiles,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .format_timestamp(None)
        .init();

    match cli.command {
        Commands::Push {
            force,
            profile,
            dry_run,
        } => sync::push(force, profile.as_deref(), dry_run)?,
        Commands::Pull { profile, dry_run } => sync::pull(profile.as_deref(), dry_run)?,
        Commands::Config(cmd) => match cmd {
            ConfigCommands::Init => config::init_interactive()?,
            ConfigCommands::Show => {
                let cfg = config::Config::load()?;
                println!("{}", toml::to_string_pretty(&cfg)?);
            }
            ConfigCommands::AddProfile { name } => {
                let mut cfg = config::Config::load()?;
                if cfg.profile.iter().any(|p| p.name == name) {
                    println!("Profile '{}' already configured.", name);
                } else {
                    cfg.profile.push(config::ProfileConfig { name: name.clone() });
                    cfg.save()?;
                    println!("Added profile '{}'.", name);
                }
            }
            ConfigCommands::ListProfiles => {
                let profiles = paths::list_local_profiles()?;
                if profiles.is_empty() {
                    println!("No profiles found.");
                } else {
                    for name in &profiles {
                        println!("  {}", name);
                    }
                }
            }
        },
    }

    Ok(())
}
