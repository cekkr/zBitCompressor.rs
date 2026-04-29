# zBitCompressor.rs

Rust implementation of the `zBit` compression/decompression model, with an exact two-level Boolean minimizer for small support functions and an adaptive binary pack format for real-file compression.

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
- rule-based gating for circuit-dictionary evaluation
- size-based final method choice, never worse than raw baseline by design
- strict `.zbpk` parser validation

Code:

- `zbit-rs/src/pack.rs`
- `zbit-rs/src/pack_rules.rs`

### 7. Validation and benchmark as first-class workflow

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
- `src/pack.rs`: adaptive `.zbpk` compression/decompression
- `src/pack_rules.rs`: method-selection rules
- `src/bin/benchmark_real_file.rs`: real-file benchmark binary
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

## Programmatic Usage (Library)

```rust
use zbit_rs::{ZbitModel, compress_adaptive_to_file, decompress_file};

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
- methods: `raw-copy`, `indexed-raw`, `indexed-circuit`, `indexed-huffman`

## References

- Main theory and recommendations: `papers/zbit-algorithmsResearch.md`
- Crate internals and API: `zbit-rs/src/`

## License

PolyForm Noncommercial License 1.0.0. See `LICENSE`.
