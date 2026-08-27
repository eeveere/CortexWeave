# Troubleshooting

Start with:

```text
cortexweave --config cortexweave.toml doctor
cortexweave --config cortexweave.toml metrics
```

## Embedding Failures

Confirm the URL, route, model, timeout, and server logs. A dimension mismatch
means the server output does not match `embedding.dimension`; update the setting
only after confirming that the model really changed, then reindex. Existing
committed vectors remain available until replacement succeeds.

## Empty or Stale Search

List workspaces and verify the requested ID. Run `status`, then `reindex`. Check
ignore files, the maximum file size, binary detection, and UTF-8 validity. A
semantic search only compares embeddings with the current model and query
dimension; lexical search can help distinguish indexing from embedding issues.

## Watcher Problems

Ensure the workspace root still exists and is a directory. `serve` starts
watchers; one-shot CLI commands do not remain resident. Queue overflow triggers a
full rescan. The filesystem remains authoritative, so a manual `reindex` is a
safe recovery step.

## Partial Indexing or Analyzer Errors

Workspace indexing isolates failures by file. `files_failed` reports affected
files while healthy files continue. The failed file keeps its last committed
document and embeddings. Enable `cortexweave=debug`, fix the source or analyzer,
and reindex.

## SQLite Locking

CortexWeave waits briefly for transient locks. Long-lived external writers can
still cause a database-locked error. Stop the other writer and retry. A failed
transaction rolls back atomically; do not delete the database as a first step.

## MCP Initialization

Use absolute executable/config paths and verify argument order. Run the exact
configured command with `doctor` in a terminal. Stdout must contain protocol data
only, so redirect diagnostics and custom wrapper output to stderr.

## Unknown or Ambiguous Workspace

Call `workspace_list` before retrying. It reports each registered UUID, name,
root, and the entry matched by the adapter's configured root hint. Duplicate
names are legal, so resolve them with an exact UUID, absolute root/subdirectory
path, or file URI. An omitted selector works only when the launch hint resolves
or exactly one workspace is registered.

For Crush, confirm that `CORTEXWEAVE_WORKSPACE_ROOT` comes from a project-local
`.crushrc` and contains the project path. Do not use `$PWD` for this value in a
global `crushrc`: it can resolve to Crush's configuration directory instead of
the repository. A hint does not auto-register a directory. Supplying both
`workspace_id` and `workspace` is allowed only when they resolve to the same
registration.
