# OPENCLAW.md

## Purpose
This guide explains how to continue improving `zBitCompressor.rs` with a simpler local AI agent ("OpenClaw" style workflow) while keeping changes safe, reviewable, and test-backed.

## 1. Keep the Local Agent on Bounded Tasks
Use the smaller agent for:
- small refactors in one module
- adding focused tests
- updating docs/examples
- debugging one failing test at a time

Avoid delegating to a smaller agent:
- cross-module architecture changes without a test harness
- format or parser redesigns touching serialization compatibility
- large objective-function changes without benchmark validation

## 2. Minimal Runtime Setup
From repo root:

```bash
cargo test --manifest-path zbit-rs/Cargo.toml
```

Fast loop during edits:

```bash
cargo test --manifest-path zbit-rs/Cargo.toml --lib
cargo test --manifest-path zbit-rs/Cargo.toml --test advanced_validation
```

Full validation before merging:

```bash
cargo test --manifest-path zbit-rs/Cargo.toml
cargo run --manifest-path zbit-rs/Cargo.toml --bin zbit-benchmark -- \
  papers/zbit-algorithmsResearch.md \
  zbit-rs/benchmark_algorithmsResearch.zbpk \
  zbit-rs/benchmark_latest.txt
```

## 3. Prompt Template for a Smaller Agent
Use this exact shape to reduce drift:

```text
Task:
<one concrete change>

Constraints:
- Keep compatibility with existing zbit-rs APIs unless explicitly asked.
- Add/update tests for the changed behavior.
- Do not remove license headers in Rust files.
- Update AGENTS.md with a dated bullet in "Recent Updates".

Validation:
- Run: cargo test --manifest-path zbit-rs/Cargo.toml
- Summarize failures or pass results.

Output:
- Files changed
- Why the change is correct
- Residual risks
```

## 4. Repo-Specific Rules the Agent Must Follow
- Rust crate root is `zbit-rs/`.
- Public API lives in `zbit-rs/src/lib.rs` exports.
- Core logic paths:
  - exact minimization: `zbit-rs/src/minimizer.rs`
  - advanced flow (heuristics/rewrite/SAT/objectives): `zbit-rs/src/advanced.rs`
  - SAT engine: `zbit-rs/src/sat.rs`
  - model integration: `zbit-rs/src/model.rs`
- Add the short license/copyright header to every new `.rs` file.
- After edits, append a dated entry to `AGENTS.md`.

## 5. Safety Checklist for Each PR
1. Behavior check: truth-table validation still passes for care minterms.
2. Serialization check: `.zbit` roundtrip tests still pass.
3. Compression check: `.zbpk` adaptive roundtrip still passes.
4. Objective check: if touching scoring, run advanced tests and confirm expected metric direction.
5. Changelog hygiene: `AGENTS.md` updated.

## 6. Suggested Work Queue for a Smaller Agent
1. Add micro-benchmarks for objective scoring variants in `advanced.rs`.
2. Add deterministic tie-break tests for equal weighted-cost candidates.
3. Add optional timeout/step cap in SAT solver for pathological CNFs.
4. Add CLI switch in `zbit-rs/src/main.rs` for choosing `MappingObjective`.
5. Add property-style randomized test for advanced cover validity vs care set.

## 7. Escalation Conditions
Escalate to a stronger agent/human review when:
- tests pass but objective metrics regress unexpectedly
- changes touch both format parsing and optimization logic in one PR
- SAT logic changes alter pruning behavior without clear proof/tests
- benchmark report shows regressions without an intentional tradeoff
