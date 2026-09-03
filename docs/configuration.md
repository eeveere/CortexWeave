# Configuration

Pass a TOML file with the global `--config` option. Omitting it uses the defaults
in `AppConfig`. `cortexweave.example.toml` is a complete starting point.

For the meaning of historical Experience selection and authority, see
[Verified Experience Core](verified-experience.md).

## Server

- `server.mcp_transport`: `stdio` in v0.1. MCP messages use stdout and logs use
  stderr.
- The MCP default workspace is launch configuration, not a TOML setting. Pass
  `serve --workspace-root <path>` or set `CORTEXWEAVE_WORKSPACE_ROOT`; the command
  option has precedence. Hints resolve registered workspaces but never create
  registrations.

## Database

- `database.path`: SQLite database path. Parent directories are created. Use one
  database for all registered workspaces unless isolation is required.

SQLite foreign keys, migrations, FTS5 tables, WAL journal mode, and a five-second
busy timeout are enabled automatically. WAL lets normal reads continue while a
short reconciliation transaction is writing; it is not a substitute for avoiding
multiple long-lived external writers against the same database.

## Embedding

- `embedding.base_url`: OpenAI-compatible server base URL.
- `embedding.endpoint`: embeddings route, normally `/v1/embeddings`.
- `embedding.model`: request model and persisted embedding-space identity.
- `embedding.dimension`: optional positive vector width. Set it when known so a
  same-name dimension change triggers re-indexing instead of appearing compatible.
- `embedding.batch_size`: positive maximum input count per HTTP request.
- `embedding.timeout_seconds`: request timeout.
- `embedding.document_prefix` and `embedding.query_prefix`: optional provider-
  specific retrieval instructions. They are applied respectively while indexing
  source and while embedding a search query. For `nomic-embed-text-v1.5`, use
  `search_document: ` and `search_query: `.
- `embedding.limits.max_input_tokens`: optional embedding context ceiling.
- `embedding.limits.max_batch_tokens`: optional aggregate transformed-token
  ceiling for one request. It is independent from `max_input_tokens`; when it
  is smaller, it also becomes the effective per-item segmentation ceiling.
- `embedding.limits.reserved_tokens`: safety allowance subtracted from the
  input ceiling.
- `embedding.limits.tokenizer`: `conservative_bytes` for the safe built-in
  estimator, or `huggingface` for a local Hugging Face tokenizer file.
- `embedding.limits.tokenizer_path`: required when `tokenizer = "huggingface"`;
  points to a local `tokenizer.json`.

Changing the model, configured dimension, or retrieval prefixes causes documents to rebuild their
embedding space while preserving old committed data until replacements succeed.
Status, metrics, and `doctor` report the active token-counter identity and
whether its counts are exact or conservative. If a Hugging Face tokenizer falls
back to conservative counting at runtime, later diagnostics report that state.

## Indexing

- `indexing.debounce_ms`: quiet period used to coalesce filesystem events.
- `indexing.max_file_bytes`: larger files are excluded and stale rows removed.
- `indexing.max_concurrent_embedding_jobs`: bounded indexing concurrency.
- `indexing.include_patterns`: optional root-relative glob allowlist.
- `indexing.exclude_patterns`: optional root-relative glob denylist. Excludes
  take precedence when a path matches both lists.
- `indexing.generic_chunks.target_chars`: fallback chunk target size.
- `indexing.generic_chunks.overlap_chars`: fallback context overlap.
- `indexing.embedding_segments.overlap_tokens`: overlap used only when an
  analyzer chunk must be split to fit the embedding input ceiling.

Standard ignore files, nested ignore rules, binary detection, and UTF-8 checks
also govern discovery. A transient read or metadata failure is reported and
preserves any previously indexed document for that path until discovery succeeds.

Fallback chunk configuration is part of the generic analyzer version. Changing
the target or overlap causes unchanged fallback documents to be reconciled with
the new boundaries.

Embedding segmentation is language-neutral and runs after analysis. Prefixes
and overlap count against configured capacity. Changing its effective ceiling,
overlap, tokenizer identity, or document transformation safely rebuilds the
affected document tree. Source is split at UTF-8-safe boundaries and is never
silently truncated.

## Retrieval

- `retrieval.default_k`: default CLI result count.
- `retrieval.semantic_weight`: semantic component weight for hybrid search.
- `retrieval.lexical_weight`: FTS component weight for hybrid search.
- `retrieval.structural.enabled`: enables current-graph structural expansion in
  hybrid search. If the graph is stale or in error, hybrid search deterministically
  continues without structural evidence.
- `retrieval.structural.weight`: structural component weight in the hybrid score.
- `retrieval.structural.max_depth`: traversal depth, capped by the structural
  service maximum.
- `retrieval.structural.candidate_limit`: maximum graph-seeded candidates.
- `retrieval.structural.distance_decay`: multiplier applied for each hop after
  the first, in `[0, 1]`.
- `retrieval.structural.calls_weight`, `references_weight`,
  `implementations_weight`, `tests_weight`, `dependencies_weight`, and
  `other_weight`: finite non-negative relation-type multipliers.

Semantic and lexical weights must be finite, non-negative, and non-zero in
total. Structural limits are validated against hard service ceilings. Every
structural retrieval result retains its relationship path, graph revision,
applied limits, and truncation state.

## Working Set

- `working_set.enabled`: enables persisted session working sets and pins.
- `working_set.decay_half_life_minutes`: positive half-life used for deterministic
  activation decay.
- `working_set.activation_increment`: positive score added when a source is
  activated again.
- `working_set.max_activation_score`: upper bound for accumulated activation.
- `working_set.min_activation_score`: unpinned entries below this score are
  removed during inspection or activation.
- `working_set.max_items`: maximum retained entries and maximum pins per session.

Decay is calculated from the persisted activation score and last activation
time, so restarts do not refresh old entries. Pins are explicit and remain
eligible until unpinned, even when their working-set activation has decayed.

## Temporal Retrieval

- `temporal.recency_half_life_hours`: positive half-life used to calculate a
  source's standalone recency score. It does not replace source provenance or
  determine final context rank on its own.

## Context Candidates

Experience settings control bounded historical supplemental context; they do
not enable automatic Memory creation, tool use, or current-state claims.

- `context.candidate_pool_limit`: positive maximum number of deduplicated
  source-aware candidates retained before later context ranking and token
  selection, capped at `10000`.
- `context.structural_expansion_limit`: positive maximum number of
  language-neutral container, neighboring-symbol, and direct-child candidates
  expanded from each relevant code chunk, capped at `64`.
- `context.experience.enabled`: enables bounded historical Experience context
  only when the request supplies an active normalized failure signature.
- `context.experience.candidate_limit`: positive maximum number of eligible
  historical Experiences examined for one packet, capped at `50`.
- `context.experience.token_budget`: an independent historical-context ceiling,
  capped at `2048` tokens. It can consume only capacity left after ordinary
  context selection, so it cannot displace current code, task state, memory,
  or event context.

Experience is not a generic temporal or working-set source and cannot be
activated or pinned. Selected Experience explanations identify their authority
as `historical_supplemental` and retain the lifecycle, outcome, verification,
evidence-strength, and deterministic search-score facts used for evaluation.

Operational output may report that historical Experience evidence is stale or
ineligible. These warnings describe the recorded episode scope, lifecycle, and
graph/source snapshot at creation time; they are not claims about the current
workspace. Current code, task, and Event evidence remain authoritative.
- `context.ranking.semantic_weight`, `lexical_weight`, `task_weight`,
  `working_set_weight`, `recency_weight`, `provenance_weight`,
  `freshness_weight`, and `structural_weight`: finite non-negative component
  weights for deterministic candidate ranking. At least one must be positive;
  the weighted score is divided by their total, so weights need not sum to one.

Semantic and lexical retrieval scores are normalized before ranking. Working-set
activation is normalized by `working_set.max_activation_score`; recency,
provenance, freshness, and structural relationship scores remain in `[0, 1]`.
Every normalized component and the resulting `final_score` is retained on the
candidate for diagnostics and later context selection.

## Context Budgeting

- `context.budget.code_fraction`: soft allocation for direct code and document
  sources.
- `context.budget.structural_fraction`: soft allocation for code included by a
  structural relationship.
- `context.budget.memory_fraction`, `event_fraction`, and `state_fraction`:
  soft allocations for their respective provenance domains.

All fractions must be finite and non-negative, and together cannot exceed one.
The defaults are 50% code, 20% structural code, 15% memory, 10% events, and 5%
state. Category allocations guide the first selection pass; unused capacity is
available to the highest-value remaining candidates in a second pass.

Each context request supplies its total token budget, so the same configuration
supports compact 2K and 4K packets as well as 8K and 16K packets. Production
assembly uses the configured embedding provider's shared token counter, keeping
context estimates in the same accounting space as embedding segmentation.
Active-task references and pins are selected first and may be UTF-8-safely
truncated to the remaining total budget. Other oversized candidates are skipped.
No packet intentionally exceeds its requested total budget.

## Logging and Languages

- `logging.level`: tracing filter such as `info`, `debug`, or
  `cortexweave=debug`.
- `languages.rust`, `python`, `javascript`, `typescript`, `csharp`, `go`: enable or
  disable each structural analyzer. Disabled and unsupported text formats use
  the deterministic generic analyzer.

Run `cortexweave readiness [workspace-id]` before indexing a newly registered
workspace or after changing language settings. The report distinguishes a
disabled bundled analyzer from an unsupported text format and estimates the old
chunks and embeddings that an explicit reindex will replace. Inspection does
not rewrite these settings.
