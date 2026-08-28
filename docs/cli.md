# CLI Reference

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
```

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
reports indexed document/chunk totals and last indexing time. `metrics` adds
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
embedding endpoint, analyzer grammars, or registered workspace roots are
unhealthy.

When `CORTEXWEAVE_WORKSPACE_ROOT` is set, `doctor` also reports whether that
root resolves to a registered workspace. Without it, the report explains
whether singleton fallback is available or normal MCP calls need a selector.

`serve --workspace-root <path>` supplies the MCP adapter's default workspace
hint. If the option is omitted, `CORTEXWEAVE_WORKSPACE_ROOT` is used. The option
wins when both are present. The path may be a registered root or a subdirectory
inside one; it is resolved to the longest containing registered root. CortexWeave
does not use the server process's current directory as an implicit hint and does
not auto-register the supplied path.
