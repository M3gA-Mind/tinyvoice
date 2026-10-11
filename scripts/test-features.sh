#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
cargo test --locked --workspace
cargo test --locked --workspace --all-features
cargo fmt --manifest-path crates/tinyvoice-module/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path crates/tinyvoice-module/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --locked --manifest-path crates/tinyvoice-module/Cargo.toml --all-features
