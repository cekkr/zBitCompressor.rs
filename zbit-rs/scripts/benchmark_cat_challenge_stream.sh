#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
asset_dir="$repo_root/assets"
asset_path="$asset_dir/cat_challenge.png"
asset_url="https://geckos.ink/zbit/cat_challenge.png"

pack_path="$repo_root/zbit-rs/benchmark_cat_challenge_stream.zbps"
report_path="$repo_root/zbit-rs/benchmark_cat_challenge_stream_latest.txt"

chunk_size=262144
key_piece_interval=8
max_group_depth=2
max_group_pieces=8

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

python3 - "$asset_path" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = path.read_bytes()
size_mb = len(data) / (1024 * 1024)
chunk = 256 * 1024
chunks = (len(data) + chunk - 1) // chunk if data else 0

msg = [f"asset-bytes={len(data)} ({size_mb:.2f} MiB)"]
msg.append(f"stream-chunk-bytes={chunk}")
msg.append(f"stream-chunks={chunks}")
msg.append(f"header8={data[:8].hex() if len(data) >= 8 else 'short'}")

if len(data) < 40 * 1024 * 1024:
    msg.append("warning=file-size-below-40MB-reference")

print(" | ".join(msg))
PY

cargo run --manifest-path "$repo_root/zbit-rs/Cargo.toml" --bin zbit-benchmark-stream -- \
  "$asset_path" \
  "$pack_path" \
  "$report_path" \
  "$chunk_size" \
  "$key_piece_interval" \
  "$max_group_depth" \
  "$max_group_pieces"

rm -f "$pack_path"

echo "Stream benchmark report updated: $report_path"
