# Checkpoint 07: Retrieval Evaluation

**Status:** Passed; retain 0.70 semantic / 0.30 lexical hybrid weighting

## Evaluation Set

The fixed evaluation corpus contains seven chunks across Rust, Python,
TypeScript, and JavaScript. Five chunks are relevant targets for the required
questions, while two are distractors. One distractor repeats the TypeScript
retry question verbatim in documentation so that lexical matching alone cannot
reliably identify the implementation.

The evaluation questions are:

- `Where is document reconciliation implemented?`
- `Find the Python class that manages caching.`
- `Which TypeScript function handles retries?`
- `EmbeddingProvider`
- `E0425`

Each question has one expected chunk. The test compares semantic, lexical, and
hybrid retrieval at `k = 3`, then compares hybrid weightings at `k = 1`.

## Results

| Mode | Recall@3 | MRR | Mean latency |
| --- | ---: | ---: | ---: |
| Semantic | 1.00 | 1.00 | 599 us |
| Lexical | 0.40 | 0.40 | 529 us |
| Hybrid (0.70 / 0.30) | 1.00 | 1.00 | 1,097 us |

Hybrid recall@1 by semantic/lexical weight:

| Weights | Recall@1 |
| --- | ---: |
| 0.70 / 0.30 | 1.00 |
| 0.50 / 0.50 | 0.80 |
| 0.30 / 0.70 | 0.80 |

These latency measurements are regression baselines for the in-memory fixture,
not production performance estimates.

## Ranking Decision

Keep the existing `0.70` semantic and `0.30` lexical weights. The semantic
signal resolves paraphrased questions and distinguishes implementation from an
exact-match documentation distractor. A 30 percent lexical contribution still
supports exact identifiers and diagnostics without displacing the correct
semantic result. Equal or lexical-heavy weighting loses one top-ranked answer
in this evaluation.

The executable benchmark is `tests/retrieval_evaluation.rs`. It fails if the
recorded recall and ranking relationships regress.
