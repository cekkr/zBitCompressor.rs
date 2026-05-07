# ROADMAP.md

## Purpose

This roadmap captures the next implementation work for improving zBit compression speed, streaming performance, cat challenge compression ratio, and circuit-map generation/simplification quality.

It is grounded in:

- `papers/zbit-algorithmsResearch.md`
- `zbit-rs/src/pack.rs`
- `zbit-rs/src/pack_rules.rs`
- `zbit-rs/src/circuit.rs`
- `zbit-rs/src/advanced.rs`
- current benchmark reports under `zbit-rs/`

## Current Baseline

As of 2026-05-07, the cat challenge real-file benchmark selects `recursive-circuit-xz`:

- Input: `assets/cat_challenge.png`
- Original bytes: `2,969,404`
- Compressed bytes: `2,670,718`
- Ratio: `0.899412`
- Savings: `10.06%`
- Compression time: about `617,252 ms`
- Decompression time: about `9,465 ms`

The stream multilevel benchmark reaches approximately the same ratio but remains very slow:

- `realtime-fast`: ratio `0.899597`, compression about `649,140 ms`
- `realtime-balanced`: ratio `0.899455`, compression about `1,079,594 ms`
- `realtime-deep`: ratio `0.899455`, compression about `1,054,403 ms`
- `wide-overfit`: ratio `0.899455`, compression about `622,578 ms`

The current quality is acceptable for a validation prototype but not for a practical compressor. The major speed issue is sequential candidate exploration across preflate reconstruction, transform selection, XZ/LZMA profiles, zstd probes, recursive validation, and stream block/group decisions.

## Implementation Observations

- `zbit-rs/Cargo.toml` currently has no parallel execution dependency. Most independent compression candidates are evaluated serially.
- `compress_adaptive_to_bytes` evaluates raw, indexed, Huffman, deflate, zstd, xz, framed, recursive-circuit, and monotonic candidates in one sequential path.
- `choose_best_codec` tries multiple XZ profiles, optional XZ extreme profiles, and zstd sequentially.
- `choose_adaptive_transform_plan` builds a large transform candidate set, scores samples, then fully evaluates selected plans sequentially.
- `build_recursive_circuit_stream` tries preflate chain-length candidates sequentially.
- Streaming compression builds key-piece blocks serially and calls recursive full-file/global-overfit compression for the best cat challenge ratios.
- The Boolean circuit optimizer in `advanced.rs` is mostly a cover-level optimizer; the byte-stream `recursive-circuit-xz` topology in `pack.rs` is a transform metadata graph, not a full AIG/XAG circuit-map optimizer.
- Current `indexed-circuit` is skipped for normal byte inputs because `symbol_bits <= 8`; this is correct for simple byte dictionaries but means the circuit-map machinery is not yet used to model byte-plane, bit-plane, row/column, or container reconstruction structure.

## Roadmap Themes

The paper's main guidance remains the right direction:

- use exact minimization only on bounded local windows,
- use graph/network rewriting as the scalable engine,
- exploit don't-cares aggressively but locally,
- use SAT as a selective exact oracle,
- keep representation and cost models fluid.

For this repository, those ideas should be applied to compression as a costed search problem:

- dictionary bytes,
- encoded payload bytes,
- correction bytes,
- decode cost,
- memory cost,
- key-piece restart cost,
- validation cost.

## Phase 0: Measurement and Candidate Control

Priority: immediate.

### Add Per-Candidate Timing

Extend `PackStats`, `StreamPackStats`, and benchmark reports with timings for:

- index stream construction,
- Huffman stream construction,
- raw deflate,
- raw zstd,
- raw xz,
- framed container extraction,
- preflate reconstruction,
- transform-plan sampling,
- transform-plan full evaluation,
- correction-stream modeling,
- final candidate validation,
- stream block tree construction,
- stream global shared/wide payload construction.

Acceptance criteria:

- Cat challenge report identifies the top three time consumers without `ZBIT_TRACE_*`.
- Stream report separates global-overfit time from local block/group selection time.

### Add Candidate Budgets

Introduce explicit compression profiles:

- `fast`: zstd-first, limited transform search, no XZ extreme, bounded preflate candidates.
- `balanced`: current quality target with parallelized candidate search.
- `deep`: broader transform/preflate/codec search with strict time and memory budgets.
- `research`: exhaustive/probing mode, allowed to be slow.

Acceptance criteria:

- Existing tests continue to use deterministic defaults.
- Benchmarks print the active profile and skipped candidates with reasons.

### Add Early Rejection

Add cheap lower-bound and upper-bound controls:

- stop a codec profile once its output is already larger than the current best plus header overhead, when the encoder supports progress accounting or bounded output,
- skip recursive-circuit evaluation when framed extraction plus best-known correction lower bound cannot beat current best,
- skip full transform evaluation when sample score is worse than the current best by a calibrated margin,
- store candidate reasons in the report.

Acceptance criteria:

- `fast` cat challenge compression time drops substantially while retaining the current `framed-raw` or better fallback.
- `balanced` never loses more than a small configured ratio delta without reporting the skipped candidate.

## Phase 1: Parallelization

Priority: immediate after measurement.

### Add a Parallel Execution Layer

Add `rayon` or a small scoped-thread executor to run independent compression candidates concurrently.

Parallelize:

- top-level pack candidates in `compress_adaptive_to_bytes`,
- raw codec candidates (`deflate`, `zstd`, `xz`) where memory allows,
- XZ profile trials inside `build_raw_xz_payload` and `choose_best_codec`,
- selected transform-plan full evaluations,
- preflate chain-length candidates in `build_recursive_circuit_stream`,
- stream key-piece block construction,
- stream profile scripts when running benchmark matrices.

Guardrails:

- use a global worker budget to avoid running multiple high-memory XZ encoders at once,
- avoid cloning multi-MiB buffers per candidate when a shared slice is enough,
- collect deterministic results and sort by stable candidate ID before final selection,
- keep decompression single-thread-compatible.

Acceptance criteria:

- Same chosen method and same output bytes for deterministic profiles.
- Cat challenge `balanced` compression time target: below `180 s` first, then below `60 s`.
- Stream `wide-overfit` target: below `180 s` first, then below `60 s`, without ratio regression.

### Remove Duplicate Work

Add memoization for:

- transformed samples by `(input_hash, range, transform_plan)`,
- full transformed payloads selected for recursive candidates,
- preflate outputs by `(deflate_hash, max_chain_length)`,
- codec outputs by `(payload_hash, codec_profile)`,
- stream range pack candidates across adjacent blocks when the same merged range is evaluated.

Acceptance criteria:

- Stream non-wide shared payload mode does not recompute full-file recursive candidates unnecessarily.
- Benchmark reports include cache hit/miss counters.

## Phase 2: Cat Challenge Ratio Improvements

Priority: high.

The cat challenge input is an already-compressed image container. Generic byte compressors will have limited gains. Better ratios require modeling the container, inflated data, prediction/filter structure, and preflate correction stream more effectively.

### Improve Container Modeling

Keep the core method generic, but add optional container analyzers behind a common trait:

- detect chunked CRC containers,
- parse known chunks only after validating magic/header structure,
- model chunk lengths, tags, CRCs, and Adler/deflate wrapper fields as deterministic rebuild metadata,
- preserve byte-perfect output through validation.

For PNG-like data, add a dedicated analyzer module that can:

- concatenate image data chunks,
- inflate to filtered scanlines,
- split filter bytes from row payloads,
- detect width, height, color type, bit depth, and bytes-per-pixel,
- reconstruct chunks and CRCs exactly.

Acceptance criteria:

- Generic framed path remains available.
- PNG-like analyzer is selected only when validation proves exact reconstruction.
- Report prints analyzer type and deterministic rebuild fields.

### Add Image-Plane and Residual Transforms

Evaluate reversible transforms before entropy coding:

- row filter-byte stream split,
- byte-plane and bit-plane split,
- RGBA channel split,
- reversible color transforms such as green subtraction or YCoCg-style integer transforms,
- row/column delta,
- PNG predictor residuals: Sub, Up, Average, Paeth,
- local neighborhood predictors with residual escape coding,
- alpha-plane special cases,
- interleaved versus planar layout selection,
- small-tile transforms for nonstationary regions.

Use the existing transform-plan/topology structure as the serialized description, but extend it from simple periodic transforms into a typed transform DAG.

Acceptance criteria:

- Transform plans are validated by exact inverse tests.
- Cat challenge `deep` ratio target: below `0.895` first, then below `0.885` if image-plane modeling is effective.
- Reports show transformed payload bytes and correction payload bytes separately.

### Model Preflate Corrections

The current recursive path stores a transformed inflated payload plus preflate reconstruction corrections. Earlier notes show correction data can dominate gains. Split and model corrections by structure:

- correction record type,
- literal/match correction,
- distance/length correction,
- dynamic Huffman tree correction,
- block-boundary correction,
- raw bytes,
- sparse position deltas.

Then apply specialized encoding:

- RLE for zero/default correction runs,
- varint and delta coding for positions,
- bitset coding for sparse flags,
- context-coded small alphabets,
- zstd dictionary training over correction records,
- optional rANS/arithmetic coding for tiny skewed streams.

Acceptance criteria:

- Report breaks correction bytes down by substream.
- Correction stream compression improves on cat challenge without weakening preflate roundtrip validation.

### Improve Deflate Reconstruction Search

Replace coarse `max_chain_length` probing with structured search:

- infer likely compressor level and strategy,
- estimate block boundaries,
- reconstruct dynamic Huffman trees separately from LZ77 parse choices,
- cache repeated tree shapes,
- compare literal/match parse deltas rather than opaque correction blobs,
- stop exploring candidates that cannot reduce correction entropy.

Acceptance criteria:

- Same byte-perfect reconstruction.
- Fewer preflate candidates are needed for equal or better correction payload size.

## Phase 3: Circuit Map Generation and Simplification

Priority: high for ratio; medium for initial speed.

The current Boolean minimization engine is useful but not yet the main compression engine for large byte streams. The next step is to build a real circuit-map layer that represents predictors, transforms, and residual generation as a simplifiable network.

### Replace Hash-Only Gate Interning With ID-Based Structural Hashing

Implement a canonical graph core:

- stable node IDs,
- structural keys using `(op, fanins, params)`,
- complemented edges for cheap inversions,
- collision-safe equality checks,
- fanout/reference counts,
- topological order cache,
- level/depth cache,
- deterministic serialization.

Keep `BitsMap` compatibility, but route new optimization work through the canonical graph.

Acceptance criteria:

- Existing model tests pass.
- Graph node counts and depth are reported before and after optimization.
- No correctness depends only on a hash value.

### Add AIG/XAG-Oriented Map Generation

Generate maps from compression-relevant structures:

- byte-plane predictors,
- bit-plane predictors,
- row/column neighborhoods,
- previous-byte and previous-row contexts,
- LZ-style match flags,
- transform selection masks,
- correction-stream record classifiers.

Use:

- AIG nodes for AND/inverter structure,
- XAG nodes for XOR-heavy delta/parity/bit-plane transforms,
- optional MIG nodes later for majority-style predictors.

Acceptance criteria:

- A transform candidate can emit a graph topology and a compact parameter dictionary.
- The graph cost model includes both dictionary bytes and expected residual entropy.

### Add Cut Enumeration and Exact Local Replacement

Implement bounded cut enumeration:

- enumerate cuts up to 6 inputs for fast paths,
- allow 8 to 12 inputs in deep/research profiles,
- build truth tables for each cut,
- canonicalize cuts with NPN signatures,
- use exact synthesis or precomputed best forms only for high-value cuts,
- cache replacements by canonical signature.

Acceptance criteria:

- Local exact replacement is deterministic and budgeted.
- Reports include number of cuts tried, accepted, rejected, and SAT-validated.

### Add Resubstitution and Common-Subexpression Sharing

Add graph-level rewrites:

- constant propagation,
- double inversion removal,
- idempotence,
- absorption,
- consensus,
- factoring,
- common divisor extraction,
- fanout-aware resubstitution,
- balancing after area-reducing rewrites,
- shared predictor subgraphs across stream blocks.

Acceptance criteria:

- Node count and serialized dictionary bytes decrease on synthetic predictor-heavy tests.
- Stream blocks can share stable subgraph IDs where key-piece restart rules allow it.

### Add Don't-Care and Observability-Aware Simplification

Exploit local don't-cares from:

- unused/padding bits,
- deterministic container fields,
- recomputed CRC/Adler fields,
- impossible chunk states,
- predictor branches unused for the selected mode,
- key-piece boundaries,
- known row/tile geometry.

Use SAT or bit-parallel simulation to validate replacements inside bounded windows.

Acceptance criteria:

- Don't-care use is reported separately from ordinary rewriting.
- Every don't-care-based replacement has either exhaustive local truth-table validation or SAT validation.

### Add XOR/Affine Detection

Many byte and image transforms are XOR/delta/affine-heavy. Add:

- GF(2) Gaussian elimination for affine relationships,
- ANF extraction for small windows,
- XOR divisor extraction,
- XAG rewriting and balancing,
- parity predictor candidates.

Acceptance criteria:

- XOR-heavy synthetic tests compress better than equivalent AIG-only maps.
- The selected graph representation is visible in benchmark reports.

### Add Compression-Aware Cost Extraction

The current advanced optimizer has ASIC/FPGA-style objectives. Add compression objectives:

- serialized topology bytes,
- parameter bytes,
- residual entropy estimate,
- correction bytes,
- decoder work,
- stream restart overhead,
- memory footprint,
- validation cost.

Acceptance criteria:

- The optimizer can choose a larger graph if it reduces payload bytes enough.
- Reports show why a graph candidate won or lost.

## Phase 4: Streaming Architecture

Priority: high after parallel candidate work.

### Parallel Key-Piece Blocks

Build stream blocks independently in parallel, then serialize them deterministically.

Acceptance criteria:

- Resume decode behavior remains unchanged.
- Stream output is byte-identical across runs for deterministic profiles.

### Replace Recursive Split Search With Bottom-Up DP

Current stream grouping recursively tries splits within each key-piece block. Convert to bottom-up dynamic programming with precomputed range candidates:

- precompute piece and group costs,
- restrict group spans by profile,
- reuse previous-block grouping shape hints,
- use local/global candidate lower bounds,
- keep a fallback exact search for small blocks.

Acceptance criteria:

- Same or better stream encoded size for current default chunk/key settings.
- Stream block planning time is visible and materially reduced.

### Add Shared Dictionaries Without Full Wide Overfit

Generalize the current shared-grouping payload into explicit shared dictionaries:

- shared transform graph dictionary,
- shared preflate correction model,
- shared codec dictionaries,
- per-key-piece residual payloads,
- restart-safe dependency metadata.

Acceptance criteria:

- Non-wide realtime mode improves ratio without requiring every block to reference a full-file decoded global slice.
- Key-piece resume validation remains PASS.

## Phase 5: Benchmark, Validation, and Regression Policy

Priority: ongoing.

### Add Ratio and Time Gates

Track separate gates for:

- paper benchmark,
- `assets/primary.3b.bin`,
- cat challenge real-file,
- cat challenge stream,
- synthetic predictor-heavy corpus,
- synthetic XOR/affine corpus,
- random incompressible corpus.

Acceptance criteria:

- CI can run quick gates.
- Deep/research gates remain ignored/manual unless explicitly requested.

### Add Competitor Baselines

Record optional comparison numbers for:

- raw zstd levels,
- raw xz presets,
- deflate/zlib,
- PNG optimizer path where applicable,
- current zBit candidate winner.

Acceptance criteria:

- Benchmark reports make it clear when zBit is beating a generic codec and when it is not.

### Add Determinism Checks

For every profile:

- same input,
- same options,
- same output bytes,
- same decompressed bytes,
- stable report fields except timings and resource usage.

Acceptance criteria:

- Deterministic benchmark mode can be used for regression diffs.

## Suggested Implementation Order

1. Add per-candidate timing and profile selection.
2. Add parallel top-level candidate evaluation with deterministic result collection.
3. Parallelize transform-plan, codec-profile, and preflate-chain searches.
4. Add container analyzer trait and PNG-like analyzer.
5. Add image-plane/residual transform DAGs.
6. Split and model preflate corrections.
7. Add canonical ID-based graph core for circuit maps.
8. Add AIG/XAG map generation for predictors and correction classifiers.
9. Add cut enumeration, NPN cache, and exact local replacement.
10. Add compression-aware graph cost extraction.
11. Rework stream block planning as parallel bottom-up DP.
12. Add shared graph/correction dictionaries for non-wide stream mode.

## General References

Local reference:

- `papers/zbit-algorithmsResearch.md`

Logic synthesis and circuit simplification:

- R. E. Bryant, "Graph-Based Algorithms for Boolean Function Manipulation," IEEE Transactions on Computers, 1986.
- R. E. Bryant, "Symbolic Boolean Manipulation with Ordered Binary Decision Diagrams," ACM Computing Surveys, 1992.
- R. K. Brayton, G. D. Hachtel, C. T. McMullen, A. Sangiovanni-Vincentelli, "Logic Synthesis for VLSI Design," Kluwer, 1984/1989.
- R. Rudell, "Multiple-Valued Logic Minimization for PLA Synthesis," Berkeley technical report, 1986.
- A. Mishchenko, S. Chatterjee, R. Brayton, "DAG-Aware AIG Rewriting: A Fresh Look at Combinational Logic Synthesis," DAC 2006.
- R. Brayton, A. Mishchenko, "ABC: An Academic Industrial-Strength Verification Tool," CAV 2010.
- A. Mishchenko, R. Brayton, J.-H. Jiang, S. Jang, "Scalable Don't-Care-Based Logic Optimization and Resynthesis," ACM TRETS, 2011.
- M. Soeken et al., "Practical Exact Synthesis," DATE 2018.

Compression and container modeling:

- P. Deutsch, "DEFLATE Compressed Data Format Specification," RFC 1951.
- P. Deutsch, "ZLIB Compressed Data Format Specification," RFC 1950.
- PNG Specification, W3C Recommendation / ISO/IEC 15948.
- LZMA SDK documentation and XZ Utils design notes for LZMA2 tuning.
- Zstandard documentation and dictionary training references.
- Preflate-style deflate reconstruction literature and implementation notes.

