# zbit-rs

Rust migration of the `zBit` compression model with:
- exact small-support minimization (Quine-McCluskey + exact cover)
- Espresso-style iterative cover heuristics
- AIG-style rewriting/resubstitution + balancing-aware scoring
- SAT-assisted local redundancy pruning
- technology-aware objective mapping (ASIC area/delay and FPGA LUT4/LUT6 proxies)
- canonical DAG interning for gates
- model serialization/deserialization (`.zbit`)
- adaptive pack strategy (`raw-copy`, `indexed-raw`, `indexed-circuit`, `indexed-huffman`, `raw-deflate`, `raw-zstd`, `framed-raw`, `recursive-circuit-xz`, `monotonic-delta`)
- real-file benchmark/report generation

## Build and Test

```bash
cargo test
```

## Demo Validation Binary

```bash
cargo run --bin zbit-rs
```

## Advanced API (Library)

`ZbitModel` now exposes:
- `compress_from_table_advanced(outputs, dont_cares, &AdvancedOptions)`
- `compress_from_table_with_objective(outputs, dont_cares, MappingObjective)`

## Real File Benchmark

```bash
cargo run --bin zbit-benchmark -- \
  ../papers/zbit-algorithmsResearch.md \
  benchmark_algorithmsResearch.zbpk \
  benchmark_latest.txt
```

The benchmark writes `benchmark_latest.txt` with:
- selected method and rule rationale
- candidate size comparison
- compression/decompression timings and throughput
- compression ratio and output validation result

## Cat Challenge Benchmark

Run the reproducible cat challenge flow (auto-downloads asset if missing):

```bash
bash scripts/benchmark_cat_challenge.sh
```

This updates:
- `benchmark_cat_challenge_latest.txt` (tracked benchmark reference)
