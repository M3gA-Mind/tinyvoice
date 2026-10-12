#!/usr/bin/env bash
# Exercise native fixtures and the real artifact before checking live-runtime files.
set -euo pipefail
minimum="${1:-90}"
report="${2:-live-module-coverage.json}"
manifest="crates/tinyvoice-module/Cargo.toml"
cargo llvm-cov --locked --manifest-path "$manifest" --all-features --lib --no-report
# all-features selects static-link; the loader needs the default dynamic build.
cargo llvm-cov run --locked --manifest-path "$manifest" --no-report --example verify_module -- crates/tinyvoice-module/target/llvm-cov-target/debug/deps/libtinyvoice_module.so
cargo llvm-cov report --manifest-path "$manifest" --json --output-path "$report"
require_runtime_coverage() {
  local prefix="$1"
  local expected="$2"
count="$(jq --arg prefix "$prefix" '[.data[].files[] | select(.filename | startswith($prefix)) | select(.filename | endswith("_tests.rs") | not) | select(.summary.lines.count > 0)] | length' "$report")"
if [[ "$count" -ne "$expected" ]]; then
  echo "expected $expected covered runtime source files under $prefix, found $count" >&2
  exit 1
fi
jq -r --arg prefix "$prefix" '.data[].files[] | select(.filename | startswith($prefix)) | select(.filename | endswith("_tests.rs") | not) | select(.summary.lines.count > 0) | "\(.filename): \(.summary.lines.percent)%"' "$report"
failures="$(jq -r --arg prefix "$prefix" --argjson minimum "$minimum" '.data[].files[] | select(.filename | startswith($prefix)) | select(.filename | endswith("_tests.rs") | not) | select(.summary.lines.count > 0) | select(.summary.lines.percent < $minimum) | "\(.filename): \(.summary.lines.percent)%"' "$report")"
if [[ -n "$failures" ]]; then
  printf 'Runtime files below %s%% line coverage:\n%s\n' "$minimum" "$failures" >&2
  exit 1
fi

}
require_runtime_coverage "$(pwd -P)/crates/tinyvoice-module/src/service/live/" 6
require_runtime_coverage "$(pwd -P)/crates/tinyvoice-module/src/service/relay_runtime/" 2
