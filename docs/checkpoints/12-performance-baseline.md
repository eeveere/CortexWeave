# Breakpoint 12: Performance Baseline

Measured on Windows in the debug test profile on 2026-08-26. These values are a
local comparison point, not portable latency thresholds.

| Metric | Rust-only | Mixed, 5 languages |
| --- | ---: | ---: |
| Files / chunks | 1 / 2 | 5 / 7 |
| Initial indexing | 23.91 ms | 67.75 ms |
| Startup reconciliation | 9.31 ms | 23.64 ms |
| Startup embedding calls | 0 | 0 |
| Single-function edit | 18.89 ms | 34.10 ms |
| Embedding calls per edit | 1 | 1 |
| Re-embedded / actually changed | 1 / 1 | 1 / 1 |
| Semantic query | 1.55 ms | 1.00 ms |
| Hybrid query | 3.62 ms | 4.39 ms |
| SQLite file | 184,320 bytes | 184,320 bytes |
| Process working set | 16,113,664 bytes | 17,195,008 bytes |
| Analyzer average / max | 0.268 / 0.268 ms | 0.252 / 0.252 ms |

The critical incremental ratio is exactly `1:1` in both fixtures. The benchmark
is repeatable with:

```text
cargo test --test performance_baseline -- --nocapture
```

The test asserts the stable efficiency properties and reports timing, database,
working-set, and analyzer measurements for observation.
