# Checkpoint 03: Analyzer Architecture

**Status:** Passed

A new language is implemented by adding a `LanguageAnalyzer` and registering it.
The scanner chooses analyzers through `AnalyzerRegistry`; indexing receives only
`AnalyzedChunk`; storage persists normalized strings and JSON metadata. Retrieval,
watching, memory, CLI, and MCP have no language-specific branches.

Tree-sitter nodes remain private to analyzer support code. The core contract is
stable keys, normalized symbol kinds, byte/line ranges, content, capabilities,
and metadata. Generic UTF-8 chunking remains the fallback.

