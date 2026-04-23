# zbit-rs

Rust migration of the `zBit` compression model with:
- exact small-support minimization (Quine-McCluskey + exact cover)
- canonical DAG interning for gates
- model serialization/deserialization (`.zbit`)
- adaptive pack strategy (`raw-copy`, `indexed-raw`, `indexed-circuit`)
- real-file benchmark/report generation

## Build and Test

```bash
cargo test
```

## Demo Validation Binary

```bash
cargo run --bin zbit-rs
```

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
