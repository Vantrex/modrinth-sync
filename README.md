# modrinth-sync

Sync Modrinth App launcher profiles (mods, configs, settings) between Windows and Linux via SFTP.

Only the latest state is stored on the remote — no versioning. Uses SHA-256 checksums for incremental transfers and deletion detection.

## What gets synced

- `mods/` — all mod .jar files
- `config/` — mod configuration files (excludes `backup/` and `logs/` subdirs)
- `options.txt`, `servers.dat`, `servers.dat_old` — game settings

Worlds, saves, screenshots, logs, and caches are **not** synced.

## Build & Install

Rust compiles native binaries — you need to build separately on each OS.

### Linux

```bash
# Install Rust if you don't have it
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Clone and build
git clone <your-repo-url> modrinth-sync
cd modrinth-sync
cargo build --release

# Install to PATH
sudo cp target/release/modrinth-sync /usr/local/bin/
```

### Windows

1. **Install Rust**: Download and run the installer from https://rustup.rs.
   Choose the default options (MSVC toolchain). This also installs the required
   Visual Studio C++ Build Tools if you don't have them.

2. **Build**: Open PowerShell or Command Prompt:

   ```powershell
   git clone <your-repo-url> modrinth-sync
   cd modrinth-sync
   cargo build --release
   ```

3. **Install**: Copy the binary to a folder in your PATH:

   ```powershell
   # Create a bin folder if you don't have one
   mkdir "$env:USERPROFILE\bin" -ErrorAction SilentlyContinue

   # Copy the binary
   copy target\release\modrinth-sync.exe "$env:USERPROFILE\bin\"

   # Add to PATH (only needed once — restart your terminal after this)
   [Environment]::SetEnvironmentVariable("Path", "$env:USERPROFILE\bin;" + [Environment]::GetEnvironmentVariable("Path", "User"), "User")
   ```

   After restarting your terminal, `modrinth-sync` will be available as a command.

The resulting binary is fully self-contained — no runtime dependencies or DLLs needed.

### 2. Configure

Run the interactive setup:

```bash
modrinth-sync config init
```

This creates a config file at:
- Linux: `~/.config/modrinth-sync/config.toml`
- Windows: `%APPDATA%\modrinth-sync\config.toml`

You'll be prompted for your SSH server details and which profiles to sync.

### 3. Test

```bash
# See what would be uploaded without doing anything
modrinth-sync push --dry-run

# Actually push
modrinth-sync push

# On the other machine, pull
modrinth-sync pull
```

## Usage

```
modrinth-sync push [--force] [--profile <name>] [--dry-run] [-v]
modrinth-sync pull [--profile <name>] [--dry-run] [-v]
modrinth-sync config init
modrinth-sync config show
modrinth-sync config add-profile <name>
modrinth-sync config list-profiles
```

## Auto-sync on login

### Linux (systemd)

This creates a systemd user service that runs `modrinth-sync pull` every time you log in.

```bash
# Create the service file
mkdir -p ~/.config/systemd/user

cat > ~/.config/systemd/user/modrinth-sync.service << 'EOF'
[Unit]
Description=Modrinth Profile Sync (pull on login)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/modrinth-sync pull
# If using ssh-agent, uncomment and adjust:
# Environment=SSH_AUTH_SOCK=%t/ssh-agent.socket

[Install]
WantedBy=default.target
EOF

# Enable it (runs on every login)
systemctl --user daemon-reload
systemctl --user enable modrinth-sync.service

# Test it now
systemctl --user start modrinth-sync.service

# Check output
systemctl --user status modrinth-sync.service
journalctl --user -u modrinth-sync.service
```

**If you use an SSH key with a passphrase**, you need the ssh-agent socket available. If you use a keyfile without a passphrase or have `key_file` set in the config, it works without ssh-agent.

To **also push on logout/shutdown**, add a second service:

```bash
cat > ~/.config/systemd/user/modrinth-sync-push.service << 'EOF'
[Unit]
Description=Modrinth Profile Sync (push on shutdown)
DefaultDependencies=no
Before=shutdown.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/modrinth-sync push --force

[Install]
WantedBy=shutdown.target
EOF

systemctl --user daemon-reload
systemctl --user enable modrinth-sync-push.service
```

### Windows (Task Scheduler)

#### Pull on login

1. Press `Win+R`, type `shell:startup`, press Enter.
2. Create a file called `modrinth-sync-pull.bat` in the folder that opens:

```bat
@echo off
modrinth-sync pull
```

Or use Task Scheduler for more control:

1. Open Task Scheduler (`taskschd.msc`)
2. Click **Create Basic Task**
3. Name: `Modrinth Sync Pull`
4. Trigger: **When I log on**
5. Action: **Start a program**
   - Program: `C:\Users\<you>\bin\modrinth-sync.exe` (adjust path)
   - Arguments: `pull`
6. Check **Open the Properties dialog** before finishing
7. In Properties, check **Run whether user is logged on or not** if you want reliability

#### Push on logout (via Group Policy)

1. Press `Win+R`, type `gpedit.msc`, press Enter.
2. Navigate to: **User Configuration > Windows Settings > Scripts (Logon/Logoff)**
3. Double-click **Logoff**
4. Click **Add**, browse to a script file `modrinth-sync-push.bat`:

```bat
@echo off
modrinth-sync push --force
```

**Alternative — push via Task Scheduler on lock/shutdown event:**

1. Open Task Scheduler
2. **Create Task** (not Basic Task)
3. Name: `Modrinth Sync Push`
4. Trigger: **On an event**
   - Log: `System`
   - Source: `User32`
   - Event ID: `1074` (shutdown/restart)
5. Action: Start program `modrinth-sync.exe` with argument `push --force`

## Config file example

```toml
[server]
host = "myserver.example.com"
port = 22
user = "marinus"
key_file = "~/.ssh/id_ed25519"
remote_base = "/home/marinus/modrinth-sync"

[[profile]]
name = "1.21.10 Skyblock"

[[profile]]
name = "litematica"

[exclude]
extra_patterns = ["config/skyhanni/backup/", "config/skyhanni/logs/"]
```

## How it works

- **Manifest**: A `manifest.json` on the remote server tracks every synced file's SHA-256 hash. This is how it detects additions, modifications, and deletions.
- **Push**: Scans local files, compares hashes against the manifest, uploads changes, deletes removed files from the remote, updates the manifest.
- **Pull**: Downloads the manifest, compares against local files, downloads changes, deletes local files that were removed on the other machine.
- **Conflicts**: If the remote was last pushed from a different machine, `push` will warn and require `--force`. The intended workflow is: always `pull` first, then `push`.
