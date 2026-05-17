#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
client_manifest="$repo_root/client/Cargo.toml"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required but was not found in PATH" >&2
  exit 1
fi

echo "Building and installing tokenboard from $client_manifest"
cargo install \
  --locked \
  --force \
  --path "$repo_root/client"

installed_path="$(command -v tokenboard || true)"
if [[ -n "$installed_path" ]]; then
  echo "tokenboard is now available at $installed_path"
else
  echo "tokenboard was installed to Cargo's bin directory. Ensure that directory is on PATH." >&2
fi