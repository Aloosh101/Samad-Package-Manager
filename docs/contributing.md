# Contributing to SPM

Thank you for your interest in contributing to SPM.

## Contributor License Agreement (CLA)

By submitting a contribution (including pull requests, code edits, documentation, etc.) to this repository, you agree to the following terms:

1. **License Grant to the Project**: You grant the project authors (The SPM Project Authors) a perpetual, worldwide, non-exclusive, sublicensable, no-charge, royalty-free, irrevocable copyright and patent license to use, reproduce, modify, distribute, display, perform, and sublicense your contributions.
2. **Right to Re-License**: You explicitly agree that the project authors have the right to re-license your contributions under other licenses, including commercial licenses or standard open-source licenses, at their sole discretion.
3. **Representation of Ownership**: You represent that your contribution is your original creation and that you have the legal right to submit it under these terms.

By submitting a Pull Request, you acknowledge and agree to these terms.

## Development Setup

```bash
git clone https://github.com/Aloosh101/Samad-Package-Manager
cd spm

# Check the code compiles
cargo check

# Run tests
cargo test --lib

# Build
cargo build --release

# Install locally for testing
sudo cp target/release/spm /usr/local/bin/
sudo cp target/release/spmd /usr/local/bin/
```

## Code Style

- Follow Rust 2021 edition idioms
- Run `cargo clippy --no-deps` before committing
- Keep pure Rust — zero external package manager dependencies
- No unsafe code unless absolutely necessary (with documentation)

## Testing

```bash
# Run all lib tests
cargo test --lib

# Run specific test
cargo test --lib -- test_name

# Full integration test (requires root + bwrap)
sudo cargo test --test integration
```

## Pull Request Checklist

1. `cargo check` — clean
2. `cargo clippy --no-deps` — no warnings
3. `cargo test --lib` — all passing
4. Update relevant docs if changing behavior
5. Add tests for new functionality

## Architecture Notes

- **Thin client**: all CLI commands send JSON over a Unix socket to `spmd`. No command executes package operations directly. See `src/cli/client.rs`.
- **Command definitions**: clap derive structs in `src/cli/args.rs` (~800 lines, 30+ subcommands).
- **Daemon handlers**: all operations are handled in `src/daemon/mod.rs` (~1740 lines, 28+ handlers).
- **Package extraction**: pure Rust parsers in `src/package/extract/` (ar, cpio, deb822, RPM header).
- **Repo management**: `src/config/repos.rs` (~1070 lines) handles CRUD, update, create, sign, publish.
- **Database schema**: SQLite via `src/db/mod.rs` with read/write lock separation.
- **New handlers**: add the action string to the match block in `daemon/mod.rs`, then implement the handler function. Add the CLI variant to `args.rs` and dispatch via `client::send_command()`.

## Versioning

SPM follows [Semantic Versioning](https://semver.org/). The current version is
reflected in `Cargo.toml` (`env!("CARGO_PKG_VERSION")`) and tagged in git for releases.
