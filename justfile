# SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
# SPDX-License-Identifier: GPL-3.0-only

set dotenv-load := false

# List available recipes
default:
    @just --list

# Build everything with Cargo
build:
    cargo build --workspace

# Run the tests
test:
    cargo test --workspace

# Lint with Clippy, warnings as errors (as CI does)
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Check advisories, licenses, duplicate crates and sources
deny:
    cargo deny check

# Time the database on 100k items (not a test)
bench:
    cargo test --release -p pressboxd bench -- --ignored --nocapture

# Run the server with a local config (see "Development" in the README)
server config="pressbox.toml":
    cargo run -p pressboxd -- -c {{ config }}

# Run pressboxctl, e.g. `just ctl -s ./pressbox.sock status`
ctl *args:
    cargo run -q -p pressboxd --bin pressboxctl -- {{ args }}

# Run the terminal client, e.g. `just tui whoami`
tui *args:
    cargo run -q -p pressbox -- {{ args }}

# Build a package with Nix: pressboxd (default) or pressbox
package name="pressboxd":
    nix build .#{{ name }}

# Run the flake checks: builds, module checks, the NixOS VM test, reuse lint
check:
    nix flake check

# Format Rust, Nix and the other configured files
fmt:
    nix fmt

# Check formatting without modifying files (the flake's treefmt check, run on a copy)
fmt-check:
    nix build --no-link ".#checks.$(nix eval --impure --raw --expr builtins.currentSystem).treefmt"

# Update all flake inputs
update:
    nix flake update

# Update a specific flake input
update-input input:
    nix flake update {{ input }}

# Everything CI checks
verify: fmt-check lint test deny check

# Remove Cargo and Nix build output
clean:
    cargo clean
    rm -f result result-*
