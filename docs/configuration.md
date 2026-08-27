# Configuration

Pass a TOML file with the global `--config` option. Omitting it uses the defaults
in `AppConfig`. `cortexweave.example.toml` is a complete starting point.

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

Weights must be finite, non-negative, and not both zero.

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

- `context.candidate_pool_limit`: positive maximum number of deduplicated
  source-aware candidates retained before later context ranking and token
  selection.

## Logging and Languages

- `logging.level`: tracing filter such as `info`, `debug`, or
  `cortexweave=debug`.
- `languages.rust`, `python`, `javascript`, `typescript`, `csharp`, `go`: enable or
  disable each structural analyzer. Disabled and unsupported text formats use
  the deterministic generic analyzer.
