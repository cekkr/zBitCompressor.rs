# zBitCompressor.rs

Rust implementation of the `zBit` compression/decompression model, with an exact two-level Boolean minimizer for small support functions, an adaptive binary pack format for real-file compression, and a chunked streaming container for real-time encode/decode workflows.

## Scope

This repository currently contains:

- `zbit-rs/`: the active Rust crate
- `papers/`: theory and implementation-guidance documents

The implementation is intentionally aligned with the paper guidance that exact methods are valuable for bounded local problems, while practical compression needs representation-aware heuristics and strict validation.

## Theory -> Implementation Mapping

### 1. Exact two-level minimization in bounded scope

Paper guidance: exact minimization is strongest on small support functions and should be bounded.

Implemented:

- Quine-McCluskey style prime implicant generation
- exact minimum cover selection with branch-and-bound search
- don't-care support in minimization
- hard exact limit: `ZBIT_MAX_INPUTS_EXACT = 16`

Code:

- `zbit-rs/src/minimizer.rs`
- `zbit-rs/src/model.rs`

### 2. Canonical structural representation + rewrite-ready flow

Paper guidance: representation choice matters.

Implemented:

- canonical node interning (`Pin`, `Not`, `And`, `Or`, `Xor`)
- commutative normalization and simplification rules
- deterministic serialized model format (`.zbit`)
- advanced rewrite flow with:
  - graph-style resubstitution (absorbed-term elimination)
  - AIG-like consensus merges (local rewriting)
  - balancing-aware objective estimation

Code:

- `zbit-rs/src/model.rs`
- `zbit-rs/src/advanced.rs`

### 3. Espresso-style iterative cover heuristics

Paper guidance: large search spaces need iterative heuristic improvements in addition to exact bounded methods.

Implemented:

- iterative expand/select loop inspired by Espresso-style cover refinement
- legal expansion under ON+DC constraints
- greedy objective-aware cube selection and irredundancy cleanup

Code:

- `zbit-rs/src/advanced.rs`

### 4. SAT-assisted local exactness

Paper guidance: SAT is useful as a bounded local oracle inside larger heuristic flows.

Implemented:

- lightweight CNF SAT solver (DPLL with unit propagation)
- SAT-driven local redundancy pruning for cubes in a candidate cover
- bounded SAT window control (`sat_local_exact_inputs`)

Code:

- `zbit-rs/src/sat.rs`
- `zbit-rs/src/advanced.rs`

### 5. Technology-aware mapping objectives

Paper guidance: objective function must match target technology, not just literal count.

Implemented:

- objective-aware scoring for:
  - literal minimization
  - ASIC area proxy
  - ASIC delay proxy
  - FPGA LUT4/LUT6 proxies
- advanced model entrypoints with explicit objective selection

Code:

- `zbit-rs/src/advanced.rs`
- `zbit-rs/src/model.rs`

### 6. Representation-aware adaptive packing

Paper guidance: choose method by objective/cost, avoid one fixed algorithm worldview.

Implemented:

- adaptive selection among:
  - `raw-copy`
  - `indexed-raw`
  - `indexed-circuit`
  - `indexed-huffman`
  - `raw-deflate`
  - `raw-zstd`
- rule-based gating for circuit-dictionary evaluation
- size-based final method choice, never worse than raw baseline by design
- strict `.zbpk` parser validation

Code:

- `zbit-rs/src/pack.rs`
- `zbit-rs/src/pack_rules.rs`

### 7. Streaming compression with multi-level grouping

Implemented:

- `.zbps` chunk-stream container with key-piece intervals for restartable decode
- per-chunk/per-group adaptive selection with configurable multi-level grouping depth
- deterministic block boundaries so receivers can start decode from key pieces without replaying full history
- optional grouping-history hints in block headers for sharing generalized grouping strategy over time
- optional shared-grouping payload layer in non-wide realtime mode, so blocks can reference global generalized circuits/slices when local piece compression is weaker

Code:

- `zbit-rs/src/pack.rs`
- `zbit-rs/src/bin/benchmark_stream_real_file.rs`

### 8. Validation and benchmark as first-class workflow

Paper guidance: implementation quality requires verification + measurement loops.

Implemented:

- unit + integration tests for:
  - exact minimization
  - Espresso-style heuristic optimization
  - SAT local pruning
  - objective-aware advanced compression
  - model and pack roundtrip validation
- benchmark binary with method rationale, candidate sizes, timings, throughput, ratio, and output validation

Code:

- `zbit-rs/tests/`
- `zbit-rs/src/bin/benchmark_real_file.rs`

## Repository Layout

- `README.md`: this file
- `LICENSE`: PolyForm Noncommercial License 1.0.0
- `papers/zbit-algorithmsResearch.md`: theory and architecture recommendations
- `zbit-rs/`: Rust crate

Inside `zbit-rs/`:

- `src/lib.rs`: public API
- `src/model.rs`: exact Boolean model + `.zbit` serialization
- `src/minimizer.rs`: exact minimization engine
- `src/advanced.rs`: heuristic/rewrite/SAT/objective optimization flow
- `src/sat.rs`: internal SAT solver used by local exactness pruning
- `src/pack.rs`: adaptive `.zbpk` + streaming `.zbps` compression/decompression
- `src/pack_rules.rs`: method-selection rules
- `src/bin/benchmark_real_file.rs`: real-file benchmark binary
- `src/bin/benchmark_stream_real_file.rs`: real-file stream benchmark binary
- `tests/`: integration tests

## Build and Run

From repository root:

```bash
cargo test --manifest-path zbit-rs/Cargo.toml
```

Run the model validation demo:

```bash
cargo run --manifest-path zbit-rs/Cargo.toml --bin zbit-rs
```

Run the real-file benchmark (defaults already target `papers/zbit-algorithmsResearch.md`):

```bash
cargo run --manifest-path zbit-rs/Cargo.toml --bin zbit-benchmark -- \
  papers/zbit-algorithmsResearch.md \
  zbit-rs/benchmark_algorithmsResearch.zbpk \
  zbit-rs/benchmark_latest.txt
```

Run the cat challenge benchmark with auto-download (if missing in `assets/`):

```bash
bash zbit-rs/scripts/benchmark_cat_challenge.sh
```

Run the streaming benchmark (chunked/key-piece mode):

```bash
cargo run --manifest-path zbit-rs/Cargo.toml --bin zbit-benchmark-stream -- \
  assets/cat_challenge.png \
  zbit-rs/benchmark_cat_challenge_stream.zbps \
  zbit-rs/benchmark_cat_challenge_stream_latest.txt \
  262144 8 2 8
```

Optional trailing flags: `realtime_mode`, `wide_overfitting_circuits`, `carry_grouping_history`
as boolean values (`true`/`false` or `1`/`0`).

Compression profile control is available for both real-file and stream paths via
`ZBIT_COMPRESSION_PROFILE` (`fast`, `balanced`, `deep`, `research`), defaulting to `balanced`.

Run the cat challenge streaming benchmark script (auto-download if missing):

```bash
bash zbit-rs/scripts/benchmark_cat_challenge_stream.sh
```

Run the cat challenge multilevel streaming benchmark matrix (multiple profiles):

```bash
bash zbit-rs/scripts/benchmark_cat_challenge_stream_multilevel.sh
```

## Latest Benchmark Result Files

Current snapshot (reports generated on 2026-05-07):

### Latest Single-Run Benchmarks

| Test | Input | Selected method/profile | Original -> Compressed (bytes) | Ratio | Savings | Compression ms | Decompression ms | Peak RSS KiB | Validation |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Paper benchmark | `papers/zbit-algorithmsResearch.md` | `raw-xz` | `62015 -> 20632` | `0.332694` | `66.73%` | `221.608` | `1.119` | `145936` | `PASS` |
| Primary binary benchmark | `assets/primary.3b.bin` | `monotonic-delta` | `3233613 -> 562836` | `0.174058` | `82.59%` | `16634.566` | `151.082` | `588248` | `PASS` |
| Cat challenge benchmark | `assets/cat_challenge.png` | `recursive-circuit-xz` | `2969404 -> 2670718` | `0.899412` | `10.06%` | `112499.682` | `9637.900` | `3410588` | `PASS` |
| Cat challenge stream benchmark | `assets/cat_challenge.png` | `wide-overfit stream` | `2969404 -> 2670846` | `0.899455` | `10.05%` | `115086.113` | `9613.753` | `3357636` | `PASS` |

### Latest Cat Stream Multilevel Profiles

| Profile | Ratio | Savings | Original -> Compressed (bytes) | Compression ms | Decompression ms | Compression MiB/s | Decompression MiB/s | Compression RSS delta KiB | Decompression RSS delta KiB | Peak RSS KiB | Validation | Resume |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `realtime-fast` | `0.899597` | `10.04%` | `2969404 -> 2671266` | `137820.970` | `9654.720` | `0.021` | `0.293` | `553200` | `624` | `3407188` | `PASS` | `PASS` |
| `realtime-balanced` | `0.899455` | `10.05%` | `2969404 -> 2670846` | `127023.717` | `9803.588` | `0.022` | `0.289` | `464940` | `18588` | `3392504` | `PASS` | `PASS` |
| `realtime-deep` | `0.899455` | `10.05%` | `2969404 -> 2670846` | `127886.368` | `9626.080` | `0.022` | `0.294` | `573212` | `2768` | `3412220` | `PASS` | `PASS` |
| `wide-overfit` | `0.899455` | `10.05%` | `2969404 -> 2670846` | `109886.203` | `9612.017` | `0.026` | `0.295` | `515828` | `1900` | `3410564` | `PASS` | `PASS` |

Latest outputs for the tracked tests are written to:

- `zbit-rs/benchmark_latest.txt`: paper benchmark (`papers/zbit-algorithmsResearch.md`)
- `zbit-rs/benchmark_primary.3b_latest.txt`: primary binary benchmark (`assets/primary.3b.bin`)
- `zbit-rs/benchmark_cat_challenge_latest.txt`: cat challenge benchmark (`assets/cat_challenge.png`)
- `zbit-rs/benchmark_cat_challenge_stream_latest.txt`: cat challenge stream benchmark (`assets/cat_challenge.png`, 256 KiB chunks)
- `zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt`: cat challenge multilevel stream profile matrix

## Programmatic Usage (Library)

```rust
use zbit_rs::{
    ZbitModel, StreamPackOptions, compress_adaptive_stream_to_file, compress_adaptive_to_file,
    decompress_file, decompress_stream_file,
};

// 2-input XOR truth table
let outputs = [0u8, 1, 1, 0];
let mut model = ZbitModel::new(2)?;
model.compress_from_table(&outputs, None)?;
model.validate_against_table(&outputs)?;

// Advanced flow with technology-aware objective
let report = model.compress_from_table_with_objective(
    &outputs,
    None,
    zbit_rs::MappingObjective::FpgaLut6,
)?;
assert!(report.selected.estimated_luts > 0);

// Pack/unpack bytes
let input = b"abcabcabc";
let _stats = compress_adaptive_to_file(input, "example.zbpk")?;
let output = decompress_file("example.zbpk")?;
assert_eq!(output, input);

let stream_options = StreamPackOptions::default();
let _stream_stats = compress_adaptive_stream_to_file(input, "example.zbps", &stream_options)?;
let stream_output = decompress_stream_file("example.zbps")?;
assert_eq!(stream_output, input);
# Ok::<(), zbit_rs::ZbitError>(())
```

## File Formats (Current)

### `.zbit` model

- magic: `ZBIT` (`0x5A42_4954`)
- version: `1`
- stores canonical node DAG and root id

### `.zbpk` pack

- magic: `ZBPK` (`0x5A42_504B`)
- version: `2`
- 36-byte fixed header + dictionary + payload
- methods: `raw-copy`, `indexed-raw`, `indexed-circuit`, `indexed-huffman`, `raw-deflate`, `raw-zstd`

### `.zbps` stream pack

- magic: `ZBPS` (`0x5A42_5053`)
- version: `1`
- fixed stream header + independent key-piece blocks
- each block stores a multi-level piece/group topology and embedded `.zbpk` payloads
- key-piece interval enables restartable decode from block boundaries

## References

- Main theory and recommendations: `papers/zbit-algorithmsResearch.md`
- Crate internals and API: `zbit-rs/src/`

## License

PolyForm Noncommercial License 1.0.0. See `LICENSE`.
