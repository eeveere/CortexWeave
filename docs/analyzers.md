# Language Analyzer API

A language extension is an adapter from source text to normalized
`AnalyzedChunk` records. Indexing, storage, retrieval, watching, MCP, and memory
must remain language-neutral.

## Contract

Implement `LanguageAnalyzer` from `src/parsing/analyzer.rs`:

```rust
pub trait LanguageAnalyzer: Send + Sync {
    fn language_id(&self) -> &'static str;
    fn analyzer_id(&self) -> &'static str;
    fn analyzer_version(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];
    fn capabilities(&self) -> AnalyzerCapabilities;
    fn analyze(&self, path: &Path, source: &str) -> Result<Vec<AnalyzedChunk>>;
}
```

`language_id` is the normalized language label. `analyzer_id` identifies the
implementation, and `analyzer_version` changes when persisted output is no
longer compatible. Extensions omit the leading dot.

Every output chunk supplies:

- a deterministic stable key
- language and optional symbol/qualified symbol/kind
- zero-based byte bounds and one-based line bounds
- the exact source content to store
- a BLAKE3-independent metadata object

Stable keys represent logical identity, not body content or current line number.
They must distinguish duplicate names, overloads, nesting, and implementation
containers while surviving formatting, movement, and body edits.

## Add a Tree-sitter Language

1. Add the grammar crate to `Cargo.toml`.
2. Create `src/parsing/languages/<language>.rs`.
3. Implement `LanguageAnalyzer`; use `tree_sitter_support::analyze` and implement
   `StructureSpec` when the shared traversal fits the grammar.
4. Map grammar node kinds to `SymbolDescriptor`, including container behavior and
   the correct name field.
5. Export the analyzer from `src/parsing/languages/mod.rs`.
6. Add a boolean to `LanguageConfig`, default it deliberately, and register the
   analyzer in `AnalyzerRegistry::configured`.
7. Add the setting to `cortexweave.example.toml` and `docs/configuration.md`.
8. Add analyzer fixtures and identity tests.

No schema or indexing-core edit should be required.

## Required Tests

Cover a top-level function, nested type, method, duplicate names, overloads when
the language supports them, rename, body edit, format-only edit, insertion,
deletion, and movement. Assert both normalized metadata and stable-key behavior.
Also add the extension/language pair to the registry-selection test and include
the language in a mixed-workspace indexing or retrieval fixture.

Run:

```text
cargo test parsing::languages
cargo test parsing::registry
cargo test --all-targets
```

If the new analyzer requires changes to storage, reconciliation, retrieval,
watching, MCP, or memory, stop and identify the missing normalized capability.
Broad language-specific changes indicate a defect in the analyzer abstraction.
