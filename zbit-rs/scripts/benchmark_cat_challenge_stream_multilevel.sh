#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
asset_dir="$repo_root/assets"
asset_path="$asset_dir/cat_challenge.png"
asset_url="https://geckos.ink/zbit/cat_challenge.png"

out_report="$repo_root/zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt"
work_dir="$repo_root/zbit-rs/.stream_multilevel_tmp"
mkdir -p "$asset_dir" "$work_dir"

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
print(f"asset-bytes={len(data)} ({size_mb:.2f} MiB) | header8={data[:8].hex() if len(data) >= 8 else 'short'}")
PY

profiles=(
  "realtime-fast|65536|4|1|4|true|false|true"
  "realtime-balanced|262144|8|2|8|true|false|true"
  "realtime-deep|262144|8|3|8|true|false|true"
  "wide-overfit|262144|8|2|8|true|true|true"
)

for profile in "${profiles[@]}"; do
  IFS='|' read -r name chunk key depth group realtime wide carry <<<"$profile"

  pack_path="$work_dir/${name}.zbps"
  report_path="$work_dir/${name}.report.txt"

  echo "Running profile=$name chunk=$chunk key=$key depth=$depth group=$group realtime=$realtime wide=$wide carry=$carry"
  cargo run --manifest-path "$repo_root/zbit-rs/Cargo.toml" --bin zbit-benchmark-stream -- \
    "$asset_path" \
    "$pack_path" \
    "$report_path" \
    "$chunk" \
    "$key" \
    "$depth" \
    "$group" \
    "$realtime" \
    "$wide" \
    "$carry"

  rm -f "$pack_path"
done

python3 - "$work_dir" "$out_report" "$asset_path" <<'PY'
import pathlib
import re
import sys
from datetime import datetime, timezone

work = pathlib.Path(sys.argv[1])
out_report = pathlib.Path(sys.argv[2])
input_path = pathlib.Path(sys.argv[3])

profiles = [
    "realtime-fast",
    "realtime-balanced",
    "realtime-deep",
    "wide-overfit",
]


def pick(text: str, key: str) -> str:
    m = re.search(rf"^{re.escape(key)}:\s*(.+)$", text, re.MULTILINE)
    return m.group(1).strip() if m else "n/a"

rows = []
for name in profiles:
    report = (work / f"{name}.report.txt").read_text()
    rows.append({
        "Profile": name,
        "Ratio": pick(report, "Compression ratio (compressed/original)"),
        "Savings": pick(report, "Space savings (%)") + "%",
        "Orig": pick(report, "Original size (bytes)"),
        "Comp": pick(report, "Compressed size (bytes)"),
        "CompMs": pick(report, "Compression time (ms)"),
        "DecompMs": pick(report, "Decompression time (ms)"),
        "CompThroughput": pick(report, "Compression throughput (MiB/s)"),
        "DecompThroughput": pick(report, "Decompression throughput (MiB/s)"),
        "CompRssDelta": pick(report, "- Compression RSS delta"),
        "DecompRssDelta": pick(report, "- Decompression RSS delta"),
        "PeakRss": pick(report, "- Peak RSS (VmHWM)"),
        "Valid": pick(report, "Output validation"),
        "Resume": pick(report, "Key-piece resume validation"),
    })

headers = [
    "Profile",
    "Ratio",
    "Savings",
    "Orig",
    "Comp",
    "Comp ms",
    "Decomp ms",
    "Comp MiB/s",
    "Decomp MiB/s",
    "Comp RSS Δ KiB",
    "Decomp RSS Δ KiB",
    "Peak RSS KiB",
    "Validation",
    "Resume",
]

lines = []
lines.append("zBit-rs Cat Challenge Stream Multilevel Benchmark Report")
lines.append(f"Generated: {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M:%S')} UTC")
lines.append(f"Input file: {input_path}")
lines.append("")
lines.append("| " + " | ".join(headers) + " |")
lines.append("| " + " | ".join(["---"] * len(headers)) + " |")
for r in rows:
    lines.append(
        "| "
        + " | ".join(
            [
                r["Profile"],
                r["Ratio"],
                r["Savings"],
                r["Orig"],
                r["Comp"],
                r["CompMs"],
                r["DecompMs"],
                r["CompThroughput"],
                r["DecompThroughput"],
                r["CompRssDelta"],
                r["DecompRssDelta"],
                r["PeakRss"],
                r["Valid"],
                r["Resume"],
            ]
        )
        + " |"
    )

out_report.write_text("\n".join(lines) + "\n")
print(f"Multilevel stream benchmark report updated: {out_report}")
PY

rm -rf "$work_dir"
