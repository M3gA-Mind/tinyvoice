#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT

mkdir -p "$fixture/crates/tinyvoice/src" "$fixture/crates/tinyvoice-module/src"
cat > "$fixture/crates/tinyvoice/Cargo.toml" <<'EOF'
[package]
name = "tinyvoice"
version = "0.2.0"
edition = "2024"

[lib]
path = "src/lib.rs"
EOF
cat > "$fixture/crates/tinyvoice/src/lib.rs" <<'EOF'
pub fn version() -> &'static str { env!("CARGO_PKG_VERSION") }
EOF
cat > "$fixture/crates/tinyvoice-module/Cargo.toml" <<'EOF'
[package]
name = "tinyvoice-module"
version = "0.2.0"
edition = "2024"

[dependencies]
tinyvoice = { version = "0.1.3", path = "../tinyvoice" }
EOF
cat > "$fixture/crates/tinyvoice-module/src/lib.rs" <<'EOF'
pub fn version() -> &'static str { tinyvoice::version() }
EOF

if cargo metadata --offline --format-version 1 --manifest-path \
  "$fixture/crates/tinyvoice-module/Cargo.toml" >"$fixture/stale-metadata.json" 2>"$fixture/stale-metadata.err"; then
  echo "expected the stale tinyvoice version requirement to fail" >&2
  exit 1
fi
grep -q 'failed to select a version' "$fixture/stale-metadata.err"

"$repo_root/.github/scripts/update-module-dependency-version.sh" "$fixture" 0.2.0
cargo metadata --offline --format-version 1 --manifest-path \
  "$fixture/crates/tinyvoice-module/Cargo.toml" \
  | jq -e '[.packages[] | select(.name == "tinyvoice" or .name == "tinyvoice-module") | .version] == ["0.2.0", "0.2.0"]' >/dev/null
cargo update --offline --manifest-path "$fixture/crates/tinyvoice-module/Cargo.toml"
grep -A1 '^name = "tinyvoice"$' "$fixture/crates/tinyvoice-module/Cargo.lock" \
  | grep -q '^version = "0.2.0"$'
