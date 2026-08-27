# Breakpoint 15: Final Adversarial Review

This review covers race conditions, transactions, stale chunks, identity,
paths, analyzer failure, events, provenance, embeddings, queues, adapter
boundaries, language leakage, and future harness use. No implementation changes
were made during the review.

## v0.1 Blockers

### B1: Generic UTF-8 chunking can stop making progress

`generic.rs:111-112` advances to `start + 1` and then rounds backward to a UTF-8
boundary. If the next character occupies multiple bytes, that can restore the
old `start` and repeat forever. For example, target `2`, overlap `1`, and text
starting with repeated `beta` characters represented as two-byte UTF-8 reaches
the same first chunk indefinitely. Indexing an unsupported text file must never
hang a worker.

### B2: Concurrent reconciliation can commit stale source or delete fresh rows

`reconcile_file` reads prior state and performs analysis/embedding before an
unconditional upsert. There is no per-path ordering or optimistic check between
that read and `apply_document_reconciliation`. An older slow embedding job can
therefore overwrite a newer completed job. Separately, `reindex_workspace`
deletes rows using its initial `discovered_paths` snapshot; another watcher or
process can index a newly created file after that snapshot and have the older
scan delete it.

Required direction: serialize workspace/path mutations in-process, revalidate
the source revision before commit, and make deletion conditional on current
eligibility so separate CortexWeave processes cannot commit stale state.

### B3: Generic chunk configuration is absent from compatibility identity

`GenericAnalyzer::analyzer_version` always returns `"1"`. Changing
`target_chars` or `overlap_chars` leaves language, analyzer ID, and version
unchanged, so an unchanged document short-circuits and retains chunks produced
by the old configuration. The generic analyzer compatibility key must include
every setting that affects chunk boundaries.

### B4: MCP advertises unsupported protocol versions

`initialize_result` echoes any non-empty client `protocolVersion`, even though
the server implements only `2025-06-18`. This falsely claims support for unknown
protocols. Negotiation must return an actually supported version and tests must
cover an unsupported client version.

### B5: Required discovery and Markdown controls are missing

The v0.1 plan requires explicit include/exclude configuration and generic
chunking that prefers Markdown headings. `IndexingConfig` and
`WorkspaceScanner` expose neither include nor exclude patterns, and the generic
chunker only looks for blank-line paragraph boundaries. These are missing
promised v0.1 behaviors rather than future roadmap items.

### B6: Go method symbols are not normalized like other languages

The Go analyzer places `Type.Method` into `SymbolDescriptor.name` to obtain a
receiver-qualified stable key. The shared normalizer copies that value into both
`symbol` and `qualified_symbol`, while every other language emits a simple
method name in `symbol`. Extend the analyzer descriptor so display symbol,
qualified symbol, and identity name can differ without leaking Go syntax into
indexing.

### B7: Event persistence failure abandons later watcher paths

`process_batch` uses `?` when inserting the event for a reconciled path. If that
secondary write fails, the function exits and every remaining coalesced path is
dropped without reconciliation or a recovery rescan. Event recording failure
must be isolated from filesystem convergence and reflected through metrics or a
recovery event.

## Hardening

### H1: Discovery errors are workspace-wide

`WorkspaceScanner::scan` returns on the first walk, metadata, open, or read
error. A transient deletion or one unreadable file can prevent all healthy files
from reconciling and can stop watcher startup. Discovery should collect scoped
failures and continue, while preserving old rows for failed paths.

### H2: Session/task provenance can contradict itself

The schema independently verifies `(session_id, workspace_id)` and
`(task_id, workspace_id)` for memories and events. It accepts a session from one
session and a task belonging to another session in the same workspace. Service
validation or schema triggers should require task/session coherence when both
are supplied. Starting tasks on ended sessions and repeatedly ending sessions
also need explicit lifecycle rules.

### H3: Duplicate identity signatures are incomplete

The shared disambiguator chooses parameters or type parameters, never both.
Valid generic overloads with equal value parameters can exchange identities when
reordered. Rust impl identity omits the implemented trait, so equal methods from
two trait impls have the same base key. Identity signatures should include all
declaration components that distinguish legal symbols.

### H4: Workspace roots and per-file paths are not canonicalized

Registration stores raw root text, permitting relative roots and multiple spellings
of the same directory. A later MCP process may resolve a relative root from a
different working directory. Canonicalize existing roots at registration,
normalize platform case/prefix behavior, and ensure direct indexing paths remain
inside their workspace.

### H5: Doctor does not initialize analyzer grammars

Doctor calls each analyzer with empty source, but the shared analyzer returns
before constructing a parser or setting its language. Use a minimal valid source
or add an explicit analyzer health operation.

### H6: Adapter and service request sizes need caps

MCP accepts an unbounded input line and arbitrarily large limits. Semantic search
loads every compatible vector before truncation, and `recent_events` casts
`usize` to `i64` without checking. Add sensible configurable limits, checked
conversions, and a maximum MCP frame size. Full-vector scan remains acceptable
for small v0.1 workspaces once requests themselves are bounded.

### H7: MCP lifecycle and watcher readiness are underreported

Tools are accepted before initialization, and watcher startup failures are only
logged. Track MCP initialization state and expose failed watcher readiness in
status/doctor output.

### H8: Structurally significant wrappers can be omitted

Some grammar chunks exclude decorators, attributes, or export wrappers because
the classified declaration node is nested inside them. A wrapper-only change can
update the document hash without changing any chunk hash or embedding. Include
semantically relevant wrappers in analyzer chunk content; imports and reference
graphs remain post-v0.1 work.

### H9: Runtime metrics are global inside workspace-filtered snapshots

Persisted counts honor the requested workspace, while runtime job, watcher, and
latency counters aggregate every workspace. Either label them process-global or
maintain workspace-tagged counters to avoid misleading callers.

### H10: Custom embedding providers are not checked against their declared dimension

The OpenAI-compatible provider enforces configured dimension, but the general
indexing and query paths validate only non-empty, finite, internally consistent
vectors. Compare returned widths to `EmbeddingProvider::dimension()` whenever it
is declared.

## Future

- Replace full in-memory semantic candidate scans with an indexed vector backend
  when repository scale requires it.
- Add imports, exports, references, call relationships, and neighboring context
  in the later structural-intelligence roadmap.
- Persist or export time-series metrics when operational deployment exists.
- Add cancellation/progress and concurrent MCP request execution after v0.1
  correctness is stable.
- Generalize the concrete SQLite service construction only if another storage
  implementation becomes a real requirement.

## Rejected Concerns

- **Partial document transactions:** rejected. Document, chunks, embeddings,
  deletions, and FTS triggers commit or roll back together; fault tests cover
  mid-transaction failure.
- **Silent vector-space mixing:** rejected. Stored model/dimension filters and
  response validation prevent incompatible comparisons. H10 tightens provider
  contract enforcement but no current mixed comparison was found.
- **Unbounded watcher queue:** rejected. The queue is bounded and overflow causes
  a convergence rescan.
- **Cross-language key collisions:** rejected. Chunk uniqueness is document
  scoped and retrieval identity uses chunk IDs with workspace/path provenance.
- **MCP coupling in core:** rejected. MCP types remain in the adapter and call the
  transport-neutral facade.
- **Language syntax in indexing:** rejected. Syntax remains in analyzers and the
  shared tree-sitter normalization helper.
- **Workspace cascade blocked by session/task restrictions:** rejected. SQLite
  successfully cascades a workspace containing session-bound tasks.
- **Future harness incompatibility:** rejected. A harness can use the facade,
  custom embedding provider, watcher handles, sessions, tasks, events, memories,
  and retrieval without MCP.

## Verification State

The pre-review suite remains green: formatting, strict Clippy, and 53 tests pass.
The listed blockers are gaps in adversarial coverage or requirement completion,
not failures in the existing suite.
