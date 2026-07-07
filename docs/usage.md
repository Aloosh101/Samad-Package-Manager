# Usage

**Note**: `spm` is a thin client — every command requires the `spmd` daemon to be running. If `spmd` is unreachable, all commands fail immediately.

## Getting Started

```bash
# Start the daemon (must be root)
sudo spmd &

# Or install as a systemd service
sudo spm init --install-daemon
sudo systemctl start spmd

# Verify daemon is running
spm ps
```

## Package Operations

```bash
# Install from repositories
spm install nginx
spm install figlet cowsay --yes           # Non-interactive

# Install from a local file (.deb / .rpm / .sam)
spm install ./package.deb
spm install ./package.rpm --replace       # Force reinstall

# Install with sandbox isolation
spm install --sandbox nginx               # Standard sandbox
spm install --sandbox strict nginx        # Strict isolation
spm install --smart nginx                 # RPATH isolation for cross-distro

# Version strategy flags
spm install nginx --prefer-newest         # Prefer newest version
spm install nginx --stable-debian         # Prefer stable from Debian repos
spm install nginx --newest-redhat         # Prefer newest from RedHat repos

# Package conversion (install without extracting)
spm install ./package.deb --convert-only  # Converts to .sam without installing

# Remove packages
spm remove nginx
spm remove nginx --yes                    # Non-interactive

# Purge (remove + config files)
spm purge nginx

# Autoremove orphaned dependencies
spm autoremove --yes

# Upgrade packages
spm upgrade                               # Upgrade all packages
spm upgrade nginx                         # Upgrade specific package

# Distribution upgrade
spm dist-upgrade --yes

# Cleanup orphaned files and cache
spm cleanup
spm cleanup --force                       # Skip confirmation
```

## Repository Management

```bash
# Add a repository
spm repo add debian --source deb \
  --mirror https://deb.debian.org/debian

spm repo add fedora --source rpm \
  --mirror https://mirror.example.com/fedora

# List repositories
spm repo list

# Remove a repository
spm repo remove debian

# Create a local repository
spm repo create myrepo --source native --path /var/spm/myrepo

# Publish a package to a repository
spm repo publish myrepo ./package.sam

# Generate signing keys for a repository
spm repo gen-key myrepo

# Sign a repository
spm repo sign myrepo
```

## Search & Query

```bash
# Search local repository index
spm search hello
spm search "build essential"
spm search nginx

# Search remote repositories (Debian / COPR)
spm gs ffmpeg                             # Global search
spm gs ffmpeg --yes                       # Install directly from results

# Find which package owns a file
spm search-file /usr/bin/curl

# View package details
spm info bash
spm info nginx

# List files owned by a package
spm files curl

# Show package dependencies
spm depends bash

# Show reverse dependencies
spm rdepends bash
```

## Transaction History

```bash
# View transaction log
spm history

# View specific transaction
spm history --id 5

# Undo a transaction
spm history --id 5 --undo
```

## Configuration

```bash
# Show current configuration
spm config show

# Set a configuration value
spm config set preferred_source deb
spm config set log_level debug

# Rebuild SONAME index (after adding repos)
spm index rebuild
```

## System Management

```bash
# Initialize SPM (directories, database, backends)
spm init
spm init --from-system                    # Import all system packages
spm init --fix-backend                    # Reinstall bundled backends
spm init --install-daemon                 # Install spmd systemd service

# Database integrity check
spm fsck
spm fsck --fix                            # Repair issues
spm fsck --files                          # Check file records

# Sync database with filesystem state
spm sync
spm sync --files                          # Sync file records
spm sync --prune                          # Remove stale records

# Process check (deleted shared libraries)
spm ps
```

## Sandbox Management

```bash
# List installed sandboxes
spm sandbox list

# Run a command in a sandbox
spm sandbox run nginx -- /bin/bash
```

## System Analysis

```bash
# Full system analysis
spm analyze

# Specific analysis modes
spm analyze --mode orphan                 # Find orphaned packages
spm analyze --mode conflicts              # Find file conflicts

# Trace a binary's library dependencies
spm analyze --trace /usr/bin/ffmpeg

# Hardware detection
spm detect                                # Detect GPUs and suggest drivers
spm detect --install                      # Auto-install suggested drivers
```

## Package Building

```bash
# Build a .sam package from a directory
spm build ./my-package
spm build ./my-package --output ./dist
spm build ./my-package --sign my-key      # Sign with Ed25519 key
```

## Snapshots (Btrfs)

```bash
# Create a snapshot
spm snapshot create

# Rollback to a snapshot
spm snapshot rollback --id 20240101_120000
```

## Group Management

```bash
# Create spm UNIX group
spm group create

# Add user to spm group
spm group add alice

# Remove user from spm group
spm group remove alice

# List group members
spm group list
```

## Self-Update

```bash
# Update spm + spmd to latest version
spm self-update

# Check for updates without installing
spm self-update --check

# Update to a specific version
spm self-update --version 0.3.5

# After update, restart the daemon
sudo systemctl restart spmd
```

## Shell Completions

```bash
spm completions bash > /etc/bash_completion.d/spm
spm completions zsh > /usr/share/zsh/site-functions/_spm
spm completions fish > ~/.config/fish/completions/spm.fish
```
