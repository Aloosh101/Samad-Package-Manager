# Usage

## Repository Management

```bash
# Add a Debian repository
spm repo add debian --source deb \
  --mirrors https://deb.debian.org/debian \
  --codename stable --components main

# Add an RPM repository
spm repo add fedora --source rpm \
  --mirrors https://mirror.example.com/fedora \
  --codename 40 --components main

# List repositories
spm repo list

# Remove a repository
spm repo remove debian
```

## Package Operations

```bash
# Update repository metadata
spm update

# Search for packages
spm search hello
spm search "build essential"

# View package details
spm info bash

# List package files
spm files hello

# Install packages
spm install hello
spm install figlet cowsay --yes

# Remove packages
spm remove hello

# Purge packages (remove config too)
spm purge hello

# Autoremove orphaned dependencies
spm autoremove

# Upgrade all packages
spm upgrade

# Distribution upgrade
spm dist-upgrade
```

## Transaction History

```bash
# View history
spm history

# View specific transaction
spm history 5

# Undo a transaction
spm history undo 5
```

## Query & Analysis

```bash
# Show package dependencies
spm depends bash

# Show reverse dependencies
spm rdepends bash

# Find which package owns a file
spm search-file /usr/bin/hello

# Analyze system for orphans, conflicts
spm analyze

# Rebuild SONAME index
spm index

# Database integrity check
spm fsck

# Sync database with system state
spm sync
```

## Sandbox

```bash
# Run a command in a sandbox
spm sandbox run --cmd /bin/bash

# Install package into sandbox
spm sandbox install hello

# List sandboxes
spm sandbox list
```

## Configuration

```bash
# View configuration
spm config

# Set config value
spm config set preferred_source deb
spm config set prefer_newest true

# Generate shell completions
spm completions bash > spm.bash
spm completions zsh > spm.zsh
spm completions fish > spm.fish
```

## Daemon

```bash
# Start daemon
spmd

# Start as systemd service
systemctl start spmd
systemctl enable spmd

# Check daemon status
spm ps
```

## Build .sam Packages

```bash
# Build from source
spm build ./my-package
```
