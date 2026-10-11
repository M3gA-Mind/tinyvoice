#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <repository-root> <next-version>" >&2
  exit 2
fi

repo_root="$1"
next_version="$2"
export NEXT_VERSION="$next_version"

module_manifest="$repo_root/crates/tinyvoice-module/Cargo.toml"
perl -0pi -e '
  my $updated = s/(^tinyvoice\s*=\s*\{\s*version\s*=\s*")[^"]+("\s*,\s*path\s*=\s*"\.\.\/tinyvoice")/$1$ENV{NEXT_VERSION}$2/m;
  die "expected one local tinyvoice dependency version in $ARGV\n" unless $updated == 1;
' "$module_manifest"
