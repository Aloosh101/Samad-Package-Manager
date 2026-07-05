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

- CLI flags are defined in `src/cli/args.rs` via `clap`
- Package extraction is in `src/package/extract/`
- Repo configuration is in `src/config/repos.rs`
- Database schema is in `src/db/schema.rs`

## Versioning

SPM follows [Semantic Versioning](https://semver.org/). The current version is
reflected in `Cargo.toml` and tagged in git for releases.
