# Context Orchestration

CortexWeave builds bounded, source-aware context without making core code depend
on an agent harness or MCP. The application service accepts transport-neutral
requests; CLI and MCP are thin adapters over the same operations.

```text
code + documents + memories + events + task/session state
                         |
                    candidates
                         |
       deduplication -> ranking -> token-bounded packet
                         |
               semantic_context / resume_context
```

## Semantic Context

`ContextRequest` combines an optional query with session, task, path, and
language scope. The service gathers direct semantic and lexical code matches,
temporal evidence, task and session state, and working-set entries. Code
expansion uses analyzer-normalized container and neighbor relationships, never
language-specific logic in the indexing core.

Candidates retain semantic, lexical, task, working-set, recency, provenance,
freshness, structural, and final scores. Stable source identity deduplicates
them before bounded ranking. Current source is preferred over historical evidence
when otherwise comparable.

An explicit task or session request scopes task and session state to that
effective session; an explicit task also selects only its own task state.
Trusted durable memories and events remain workspace evidence, including across
sessions. Imported memory stays outside automatic context until an explicit
trust review accepts it; see [Memory Integrity](memory-integrity.md).

## Working Set and Time

Working-set activation decays from persisted timestamps. Pins are explicit and
remain eligible despite decay. Temporal retrieval can constrain created or
modified time, recent windows, and current or prior sessions. Ordinary context
excludes superseded memories, including direct lexical memory matches.

## Token Budgeting

The embedding provider's token counter is reused so indexing and context assembly
share accounting. Soft category allocations guide an initial selection pass,
then remaining capacity is filled by score per token. Active task state,
checkpoints during resume, and pins are retained first; only required items may
be UTF-8-safely truncated. The packet reports its selected-token estimate.

## Checkpoints and Resume

Checkpoints persist objective, completed work, referenced decisions, open
problems, related paths and symbols, and next action. `resume_context` separates
the selected interaction session from the evidence session, allowing a new
Session B to resume a task and checkpoint from ended Session A. It assembles
task state, checkpoint, unsuperseded decisions and failures, build evidence,
working-set material, aggregated file changes, and current source in one bounded
packet. Raw file events remain stored; resume aggregates path-bearing changes
and reports scoped versus unscoped watcher counts.

## Explainability and Evaluation

Set `include_explanation` in MCP or `--explain` in the CLI to attach source
identity, selection reasons, component scores, token estimate, and truncation
status for each selected item. It contains no duplicate prompt content and is
outside the packet budget.

The evaluator scores final packets: relevance recall, precision, MRR, token
utilization, source-range-aware duplicate-token ratio, current-source coverage, resume-task
accuracy, and selection latency. Named implementation-coverage cases separately
declare required path/symbol pairs. An expectation passes only when the final
packet contains a code item with both exact values; plan, documentation, or
configuration text that merely names the symbol is reported as mention-only
evidence.

For a dedicated harness, `prepare_harness_context` requires an active
workspace/session/task scope and always attaches packet explanation. The result
also carries one audit record per selected source with workspace, path, symbol,
and component scores. A caller-owned `HarnessContextPolicy` evaluates
sufficiency. Exact hydration accepts selected code IDs directly and rejects
out-of-packet IDs unless the caller supplies an explicit reason, which is stored
as a factual override event.

## Interfaces

```text
cortexweave context <workspace-id> <query> [--explain]
cortexweave resume <workspace-id> [--session-id <id>] [--task-id <id>] [--explain]
```

MCP provides `semantic_context`, `resume_context`, working-set, pin, and
checkpoint operations. See [CLI Reference](cli.md), [MCP Setup](mcp-setup.md),
and [Configuration](configuration.md) for exact arguments and keys.
