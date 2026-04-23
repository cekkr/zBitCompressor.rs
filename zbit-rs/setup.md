# zbit-rs Setup Notes

This project is now aligned with the stronger implementation approach from `studies/algorithmsResearch.md`:
- exact methods are bounded and used for small functions
- representation-aware processing is explicit
- adaptive output strategies avoid negative compression outcomes
- benchmarking and validation are first-class and automated

## Local workflow

1. Run tests:
```bash
cargo test
```

2. Run the model demo:
```bash
cargo run --bin zbit-rs
```

3. Run the real-file benchmark:
```bash
cargo run --bin zbit-benchmark -- \
  ../studies/algorithmsResearch.md \
  benchmark_algorithmsResearch.zbpk \
  benchmark_latest.txt
```
