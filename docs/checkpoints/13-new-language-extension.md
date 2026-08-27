# Breakpoint 13: New-Language Extension Test

Go was added by following `docs/analyzers.md` without changing storage,
indexing core, retrieval, watcher, MCP, memory, or the service facade.

The extension adds the `tree-sitter-go` grammar, `GoAnalyzer`, a `languages.go`
setting, registry registration, an example configuration entry, and Go analyzer
fixtures. Go method identity includes the receiver type, producing stable keys
such as `service.go::method:Service.Changed`.

Tests cover receiver extraction, top-level and receiver-qualified methods,
structural type chunks, formatting/body-edit stability, duplicate method names
on separate receiver types, deletion, registry selection, and incremental
mixed-workspace reconciliation. The abstraction held without a core change.
