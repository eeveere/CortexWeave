# Implementation status

## Implemented and connected

* Rust CLI and stdio JSON-RPC MCP adapter; native application facade.
* SQLite migrations, FTS5 chunk/memory/Experience indexes, repository layer,
  workspace registration/selector/readiness/doctor/metrics surfaces.
* Ignore-aware scan, watcher-driven reconciliation, bounded concurrency,
  adaptive segmenting and OpenAI-compatible embedding provider abstraction.
* Tree-sitter analyzers for Rust, Python, JavaScript, TypeScript, C# and Go;
  deterministic generic fallback for unsupported text.
* Incremental chunk/embedding preservation, durable graph projection/repair,
  structural reads/impact, semantic/lexical/hybrid retrieval and context
  packets.
* Explicit memory/session/task/event/checkpoint/working-set/pin state.
* Explicit Episodes; typed evidence; deterministic failure normalization;
  immutable Experience preview/acceptance/search; reviewed assessment history.

## Partial or deliberately constrained

* Vector retrieval is exact in-process cosine scanning of SQLite BLOBs, rather
  than an approximate/vector-database index.
* Structural retrieval only uses relationships/analyzer capabilities emitted by
  the supported language analyzers; unresolved relations remain unresolved.
* MCP is stdio only and starts watchers only while an MCP server runs.
* Failure normalization supports defined v1 Rust compiler, Cargo test, generic
  verifier and related registered evidence contracts—not arbitrary log parsing.
* Experience retrieval is deterministic signature/lexical/filter based; no
  semantic Experience embeddings are in the release path.
* Operational metrics are local observations, not a monitoring backend or
  feedback loop.

## Experimental

The paired real-repository Experience evaluation is explicitly a narrow,
frozen v0.5 experiment (`docs/experiments/`). It demonstrates conservative
historical context behavior, not broad agent-performance claims. The runtime
Experience feature itself is implemented; the generalization claim is
experimental.

## Planned/deferred (not current architecture)

The v0.5 boundary and verified-experience documentation defer automatic
Episode grouping, background/idle consolidation, model-backed consolidation,
semantic duplicate merging, automatic memory merging/promotion, automatic
retention/deletion, reuse-driven confidence updates, workspace-to-global
promotion, graph-driven Experience generalization, Git diff/base inspection,
diagnostic-to-symbol mapping beyond capability-gated evidence, high-frequency
event streaming, a durable action-to-outcome model, and a dedicated coding
agent harness. Do not draw these as active runtime components.

## Uncertain

No external issue tracker is present in the repository, so this classification
does not represent work tracked outside the tree. Historical checkpoint labels
occasionally describe earlier planned breakpoints; current code and the v0.5
release boundary take precedence where they differ.
