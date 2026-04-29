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
- 2026-04-29: Improved adaptive packing by adding `indexed-huffman` (canonical Huffman dictionary + variable-length payload) with decode support and candidate selection logic updates; refreshed benchmark now selects `indexed-huffman` and improves `papers/zbit-algorithmsResearch.md` compression from ratio `0.877433` to `0.605595` (62015 -> 37556 bytes, validation PASS).
- 2026-04-29: Refreshed `zbit-rs/benchmark_latest.txt` from a new benchmark run on `papers/zbit-algorithmsResearch.md` (selected `indexed-raw`, 62015 -> 54414 bytes, ratio `0.877433`, savings `12.26%`, compression `8.764 ms`, decompression `9.791 ms`, output validation PASS).
- 2026-04-29: Implemented advanced library optimization flow in `zbit-rs` with Espresso-style iterative cover heuristics, AIG-style rewrite/resubstitution passes, SAT-assisted local redundancy pruning, and technology-aware objectives (ASIC area/delay, FPGA LUT4/LUT6), plus model entrypoints and new validation tests (`zbit-rs/src/advanced.rs`, `zbit-rs/src/sat.rs`, `zbit-rs/tests/advanced_validation.rs`).
- 2026-04-29: Added `OPENCLAW.md` with a practical handoff guide for continuing this repository with a simpler local AI agent (task scoping, prompt template, validation gates, and escalation criteria).
- 2026-04-23: Replaced root `README.md` with a theory-to-implementation guide aligned to `papers/zbit-algorithmsResearch.md` and current `zbit-rs` capabilities (exact bounded minimization, adaptive packing, validation workflow, and documented non-implemented roadmap items).
- 2026-04-23: Updated moved sample paper path references from `../studies/algorithmsResearch.md` to `../papers/zbit-algorithmsResearch.md` in tests, benchmark binary defaults, and crate README.
- 2026-04-23: Added short license/copyright headers to all Rust source/test files and markdown files under `papers/`.
- 2026-04-23: Updated copyright headers to include year and contact: `Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.`
