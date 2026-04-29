#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
asset_dir="$repo_root/assets"
asset_path="$asset_dir/cat_challenge.png"
asset_url="https://geckos.ink/zbit/cat_challenge.png"

pack_path="$repo_root/zbit-rs/benchmark_cat_challenge.zbpk"
report_path="$repo_root/zbit-rs/benchmark_cat_challenge_latest.txt"

mkdir -p "$asset_dir"

if [[ ! -f "$asset_path" ]]; then
  echo "Downloading cat challenge asset to $asset_path"
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 5 --retry-delay 2 "$asset_url" -o "$asset_path"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$asset_path" "$asset_url"
  else
    echo "Neither curl nor wget is available." >&2
    exit 1
  fi
else
  echo "Using existing asset at $asset_path"
fi

cargo run --manifest-path "$repo_root/zbit-rs/Cargo.toml" --bin zbit-benchmark -- \
  "$asset_path" \
  "$pack_path" \
  "$report_path"

rm -f "$pack_path"

echo "Benchmark report updated: $report_path"
