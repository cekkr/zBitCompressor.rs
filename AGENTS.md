# AGENTS.md

## Working Rules
- Update this document after every repo edit to help rapid understanding and navigation of the project.
- Put the short license and copyright comment at the top of every `.rs` file.

## Project Navigation
- Rust crate root: `zbit-rs/`
- Main sources: `zbit-rs/src/`
- Integration tests: `zbit-rs/tests/`
- Research/input papers: `papers/`
- License file: `LICENSE`

## Recent Updates
- 2026-04-30: Reworked `png-preflate-xz` internals in `zbit-rs/src/pack.rs` from PNG-specific row assumptions to a generic adaptive circuit-transform search (identity/delta/xor/periodic-head-tail) driven by stream self-correlation, with explicit recursive circuit topology metadata (unique node IDs, series/parallel relations, and order indices) serialized in the pack dictionary for deterministic pattern matching and reconstruction.
- 2026-04-30: Added adaptive `png-preflate-xz` in `zbit-rs/src/pack.rs` + `zbit-rs/src/pack_rules.rs` (exact `IDAT` deflate reconstruction using preflate corrections plus reversible high-dimensional scanline transform with XZ coding), integrated into pack selection, decode validation, and benchmark reporting (`zbit-rs/src/bin/benchmark_real_file.rs`); refreshed cat benchmark now selects `png-preflate-xz` with ratio `0.901633` (2969404 -> 2677315 bytes, validation PASS), improving over prior `png-idat-raw` ratio `0.998557`.
- 2026-04-30: Added a new adaptive `png-idat-raw` method in `zbit-rs/src/pack.rs` + `zbit-rs/src/pack_rules.rs` that repacks contiguous PNG `IDAT` chunks by storing only concatenated IDAT payload and deterministic chunk layout metadata (CRC recomputed on decode), with full roundtrip validation and benchmark reporting (`zbit-rs/src/bin/benchmark_real_file.rs`); refreshed cat benchmark now selects `png-idat-raw` and improves from no gain (`1.000012`) to positive savings ratio `0.998557` (2969404 -> 2965120 bytes, validation PASS).
- 2026-04-30: Enhanced `zbit-rs/scripts/benchmark_cat_challenge.sh` with PNG sanity reporting (size, resolution, bit depth, color type) and warnings when the downloaded asset differs from the expected 40MB/16-bit HDR profile; current download is ~2.83 MiB, 8-bit RGBA PNG.
- 2026-04-30: Added `raw-zstd` as a new adaptive packing candidate/method with roundtrip decode support and benchmark-candidate reporting; refreshed paper benchmark now selects `raw-zstd` with ratio `0.338176` (62015 -> 20972 bytes, validation PASS), improving over prior `raw-deflate` ratio `0.355849`.
- 2026-04-30: Added cat challenge automation under `zbit-rs/scripts/benchmark_cat_challenge.sh` and ignored test hook `zbit-rs/tests/cat_challenge_benchmark.rs`; script downloads `assets/cat_challenge.png` only if missing and regenerates tracked report `zbit-rs/benchmark_cat_challenge_latest.txt`.
- 2026-04-30: Updated `.gitignore` to ignore `assets/cat_challenge.png` and generated `zbit-rs/*.zbpk` artifacts.
- 2026-04-29: Added `raw-deflate` adaptive pack method (zlib/deflate) with selection-rule integration and benchmark reporting; latest benchmark on `papers/zbit-algorithmsResearch.md` now selects `raw-deflate` with ratio `0.355849` (62015 -> 22068 bytes, validation PASS), improving over prior `indexed-huffman` ratio `0.605595`.
- 2026-04-29: Added large-file decode safety bound in pack parser (`original_size` hard cap at 1 GiB) to prevent unbounded expansion risk during decompression.
- 2026-04-29: Improved adaptive packing by adding `indexed-huffman` (canonical Huffman dictionary + variable-length payload) with decode support and candidate selection logic updates; refreshed benchmark now selects `indexed-huffman` and improves `papers/zbit-algorithmsResearch.md` compression from ratio `0.877433` to `0.605595` (62015 -> 37556 bytes, validation PASS).
- 2026-04-29: Refreshed `zbit-rs/benchmark_latest.txt` from a new benchmark run on `papers/zbit-algorithmsResearch.md` (selected `indexed-raw`, 62015 -> 54414 bytes, ratio `0.877433`, savings `12.26%`, compression `8.764 ms`, decompression `9.791 ms`, output validation PASS).
- 2026-04-29: Implemented advanced library optimization flow in `zbit-rs` with Espresso-style iterative cover heuristics, AIG-style rewrite/resubstitution passes, SAT-assisted local redundancy pruning, and technology-aware objectives (ASIC area/delay, FPGA LUT4/LUT6), plus model entrypoints and new validation tests (`zbit-rs/src/advanced.rs`, `zbit-rs/src/sat.rs`, `zbit-rs/tests/advanced_validation.rs`).
- 2026-04-29: Added `OPENCLAW.md` with a practical handoff guide for continuing this repository with a simpler local AI agent (task scoping, prompt template, validation gates, and escalation criteria).
- 2026-04-23: Replaced root `README.md` with a theory-to-implementation guide aligned to `papers/zbit-algorithmsResearch.md` and current `zbit-rs` capabilities (exact bounded minimization, adaptive packing, validation workflow, and documented non-implemented roadmap items).
- 2026-04-23: Updated moved sample paper path references from `../studies/algorithmsResearch.md` to `../papers/zbit-algorithmsResearch.md` in tests, benchmark binary defaults, and crate README.
- 2026-04-23: Added short license/copyright headers to all Rust source/test files and markdown files under `papers/`.
- 2026-04-23: Updated copyright headers to include year and contact: `Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.`
