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
cortexweave metrics [workspace-id]
cortexweave workspace add <root-path> [--name <name>]
cortexweave workspace list
cortexweave reindex <workspace-id>
cortexweave search <workspace-id> <query> [--mode semantic|lexical|hybrid] [--limit N]
cortexweave memory add <workspace-id> <content> [options]
```

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
