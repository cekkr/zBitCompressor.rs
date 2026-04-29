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

python3 - "$asset_path" <<'PY'
import pathlib
import struct
import sys

path = pathlib.Path(sys.argv[1])
data = path.read_bytes()
size_mb = len(data) / (1024 * 1024)

msg = [f"asset-bytes={len(data)} ({size_mb:.2f} MiB)"]

if data[:8] != b"\x89PNG\r\n\x1a\n":
    msg.append("format=not-png")
    print(" | ".join(msg))
    sys.exit(0)

if len(data) >= 33 and data[12:16] == b'IHDR':
    width = struct.unpack(">I", data[16:20])[0]
    height = struct.unpack(">I", data[20:24])[0]
    bit_depth = data[24]
    color_type = data[25]
    msg.append(f"png={width}x{height}")
    msg.append(f"bit-depth={bit_depth}")
    msg.append(f"color-type={color_type}")
    if bit_depth != 16:
        msg.append("warning=expected-16bit-hdr-but-downloaded-file-is-different")
else:
    msg.append("warning=ihdr-not-found")

if len(data) < 40 * 1024 * 1024:
    msg.append("warning=file-size-below-40MB-reference")

print(" | ".join(msg))
PY

cargo run --manifest-path "$repo_root/zbit-rs/Cargo.toml" --bin zbit-benchmark -- \
  "$asset_path" \
  "$pack_path" \
  "$report_path"

rm -f "$pack_path"

echo "Benchmark report updated: $report_path"
