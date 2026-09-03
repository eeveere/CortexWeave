# CLI Reference

For graph semantics, provenance, analyzer extension guidance, and limitations,
see [Structural Graph Architecture](graph-architecture.md).
For Episode and Experience semantics beyond this command reference, see
[Verified Experience Core](verified-experience.md).

The global configuration option precedes the command:

```text
cortexweave --config cortexweave.toml <command>
```

## Commands

```text
cortexweave serve [--workspace-root <path>]
cortexweave doctor
cortexweave status [workspace-id]
cortexweave readiness [workspace-id]
cortexweave metrics [workspace-id]
cortexweave workspace add <root-path> [--name <name>]
cortexweave workspace list
cortexweave graph status <workspace-id>
cortexweave graph find <workspace-id> <symbol-or-path> [graph options]
cortexweave graph neighbors <workspace-id> <node-id> [graph options]
cortexweave graph callers|callees|references <workspace-id> <node-id> [graph options]
cortexweave graph implementations|tests <workspace-id> <node-id> [graph options]
cortexweave graph dependencies|dependents <workspace-id> <node-id> [graph options]
cortexweave graph impact-symbol <workspace-id> <symbol> [graph options]
cortexweave graph impact-path <workspace-id> <relative-path> [graph options]
cortexweave reindex <workspace-id>
cortexweave search <workspace-id> <query> [--mode semantic|lexical|hybrid] [--limit N]
cortexweave context <workspace-id> <query> [context options]
cortexweave resume <workspace-id> [--session-id <id>] [--task-id <id>] [--token-budget N]
cortexweave working-set <workspace-id> <session-id> [--task-id <id>]
cortexweave context-pin <workspace-id> <session-id> <source-id> <source-type> [--task-id <id>]
cortexweave context-unpin <workspace-id> <session-id> <source-id> <source-type> [--task-id <id>]
cortexweave checkpoint create <workspace-id> <session-id> <content> [options]
cortexweave checkpoint latest <workspace-id> [--session-id <id>|--task-id <id>]
cortexweave memory add <workspace-id> <content> [options]
cortexweave episode start <workspace-id> --session-id <id> --type <type> [--task-id <id>] [--title <text>]
cortexweave episode add-events <workspace-id> <episode-id> --expected-version <n> --request-key <key> <event-id>...
cortexweave episode close|abandon <workspace-id> <episode-id> --expected-version <n> --request-key <key>
cortexweave episode show <workspace-id> <episode-id>
cortexweave episode list <workspace-id> [--session-id <id>] [--task-id <id>] [--limit N]
cortexweave experience preview <workspace-id> --episode-id <id> --expected-version <n>
cortexweave experience consolidate <workspace-id> --episode-id <id> --expected-version <n> --expected-fingerprint <hash> --expected-proposal-hash <hash>
cortexweave experience search <workspace-id> [--query <text>] [--failure-signature <json>] [--include-historical] [--limit N]
cortexweave experience show|explain <workspace-id> <experience-id>
cortexweave experience history <workspace-id> <experience-id> [--limit N] [--after-created-at <rfc3339> --after-id <id>]
cortexweave experience assess <workspace-id> <experience-id> --kind <kind> --reviewed-by <identity> --request-key <key> --reason <text> --evidence-event-id <id>...
cortexweave experience propose-dispute <workspace-id> --failure-signature <json> --recurring-failure-event-id <id>...
```

Graph commands return JSON with the graph snapshot, effective limits, nodes,
typed edges, paths, confidence, and a truncation flag. Use `graph find` first,
then pass one returned exact node ID to a relationship command. Impact commands
start from an exact symbol or indexed relative path and walk reverse graph
dependencies; they do not infer a git diff.

`graph neighbors` is always one hop. `graph tests` reports likely direct-call
test associations, not proven behavioral coverage. Exact node IDs must exist in
the selected workspace; oversized explicit seed lists fail rather than being
silently truncated.

Graph options are:

```text
--allow-stale
--max-nodes <count>
--max-edges <count>
--max-depth <count>
```

Reads require a current graph unless `--allow-stale` is supplied. The service
enforces hard bounds and records the effective limits plus any truncation in the
response. A graph revision changing during a read produces a retryable error
rather than a mixed-snapshot answer.

`context` returns a bounded `ContextPacket` assembled from ranked code,
documents, memories, events, and task or session state. Its options are:

```text
--session-id <id>
--task-id <id>
--token-budget <count>
--no-code
--no-documents
--no-memories
--no-events
--path-scope <path>
--language-scope <language>
--active-failure-signature <json>
--explain
```

Repeat `--path-scope` or `--language-scope` to narrow the candidate sources.
Omit `--token-budget` to use the request default. `ContextPacket` reports both
the requested budget and the selected item-token total, including whether any
required item was truncated.

Add `--explain` to `context` or `resume` to include a diagnostic for every
selected item: source identity, selection reasons, component scores, token
estimate, and truncation status. Explanation is not prompt content and does not
consume the packet token budget.

`--active-failure-signature` accepts the canonical JSON `FailureSignature`
returned by CortexWeave. It may add bounded historical Experience material only
after ordinary context has been selected; it neither asserts present code state
nor replaces current task or Event evidence.

## Episodes and Experiences

This is the transport surface for the verified historical-learning path; it
does not grant a CLI caller authority to invent evidence, accept a proposal
implicitly, or treat an old verification as current truth.

Episodes are explicit user-created records over existing session-scoped events.
Use `episode start`, add at most 100 event IDs with `episode add-events`, then
explicitly `close` or `abandon` the record. Membership and terminal operations
require both an expected version and caller-chosen request key, making retries
and concurrent changes visible rather than silently merged.

`experience preview` is read-only. Only a proposal whose disposition is
`automatic` can be accepted in v0.5; copy that proposal's exact fingerprint and
proposal hash into `experience consolidate`. A `review_required` proposal is
inspectable but remains read-only, and the command has no reviewed-acceptance
bypass. It never infers acceptance from an episode alone.
Experience search returns active records by default. `--include-historical` is
required to inspect disputed, refuted, or superseded records.

`experience assess` appends a caller-declared reviewed assessment with an
identity, idempotency key, reason, and one to 64 evidence event IDs. Repeating
the same key and review returns the original immutable row; changing the review
under that key is a conflict. `experience history` returns at most 50 rows and
uses its returned cursor with `--after-created-at` and `--after-id` to continue.
It records the supplied
review metadata but does not authenticate that identity or impose a policy on an
external agent. `experience propose-dispute` is read-only and never changes an
Experience lifecycle.

`resume` returns a `ResumeContext` envelope. It resolves the current interaction
session and task independently from the checkpoint evidence session, then
returns a bounded packet with task state, checkpoint, decisions, failures,
working-set material, change aggregates, and current source. With no explicit
scope it selects the latest active session and a resumable task deterministically.

`working-set`, `context-pin`, and `context-unpin` inspect or manage session
state. Use the source type names returned in context items, such as `code`,
`document`, `memory`, or `event`. `checkpoint create` accepts repeatable
`--completed`, `--decision-id`, `--open-problem`, `--related-path`, and
`--related-symbol` options plus `--objective` and `--next-action`.

`memory add` options:

```text
--kind decision|observation|failure|solution|todo|note|checkpoint
--session-id <id>
--task-id <id>
--related-path <path>
--metadata <json-object>
```

Repeat `--related-path` for multiple files. Output is formatted JSON. `status`
reports indexed document/chunk totals, last indexing time, and a graph summary:
the revision and staleness state, node/edge/unresolved counts, and each indexed
language's active analyzer capability set. `graph status` returns that graph
summary alone, which is useful before issuing a current-only graph query.
`metrics` adds
per-language and analyzer counts, fallback use, memory/event counts, current job
state, failures, embedding and search latency, watcher coalescing, chunk changes,
and the active embedding space.

`readiness` scans the workspace without changing configuration or indexing. It
reports discovered languages, configured and available analyzers, generic
fallback use, scan failures, and the document/chunk/embedding replacement cost
for both the current configuration and recommended analyzer enablement. A
supported disabled language is reported with its exact `languages.<name>` key;
apply the configuration change and run `reindex` explicitly.

`doctor` exits unsuccessfully when the database, migrations, FTS tables,
embedding endpoint, analyzer grammars, registered workspace roots, or a
registered workspace graph projection are unhealthy. Its graph section includes
the same per-workspace graph summaries so stale or failed graph analysis is
actionable rather than silently accepted.

When `CORTEXWEAVE_WORKSPACE_ROOT` is set, `doctor` also reports whether that
root resolves to a registered workspace. Without it, the report explains
whether singleton fallback is available or normal MCP calls need a selector.

`serve --workspace-root <path>` supplies the MCP adapter's default workspace
hint. If the option is omitted, `CORTEXWEAVE_WORKSPACE_ROOT` is used. The option
wins when both are present. The path may be a registered root or a subdirectory
inside one; it is resolved to the longest containing registered root. CortexWeave
does not use the server process's current directory as an implicit hint and does
not auto-register the supplied path.
