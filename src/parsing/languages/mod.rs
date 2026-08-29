mod csharp;
mod go;
mod javascript;
mod python;
mod rust;
mod typescript;

pub use csharp::CSharpAnalyzer;
pub use go::GoAnalyzer;
pub use javascript::JavaScriptAnalyzer;
pub use python::PythonAnalyzer;
pub use rust::RustAnalyzer;
pub use typescript::TypeScriptAnalyzer;

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, path::Path};

    use crate::{
        domain::{AnalyzedChunk, SymbolKind},
        parsing::LanguageAnalyzer,
    };

    use super::*;

    fn chunks(analyzer: &dyn LanguageAnalyzer, path: &str, source: &str) -> Vec<AnalyzedChunk> {
        analyzer.analyze(Path::new(path), source).unwrap().chunks
    }

    fn key_for(chunks: &[AnalyzedChunk], symbol: &str) -> String {
        chunks
            .iter()
            .find(|chunk| chunk.symbol.as_deref() == Some(symbol))
            .unwrap_or_else(|| panic!("missing symbol {symbol}: {chunks:#?}"))
            .stable_key
            .clone()
    }

    #[test]
    fn all_initial_analyzers_emit_normalized_methods() {
        let cases: Vec<(&dyn LanguageAnalyzer, &str, &str, &str)> = vec![
            (
                &RustAnalyzer,
                "src/engine.rs",
                "struct Engine; impl Engine { fn search(&self, q: &str) -> bool { !q.is_empty() } }",
                "search",
            ),
            (
                &PythonAnalyzer,
                "engine.py",
                "class Engine:\n    def search(self, q):\n        return bool(q)\n",
                "search",
            ),
            (
                &TypeScriptAnalyzer,
                "engine.ts",
                "class Engine { search(q: string): boolean { return !!q; } }",
                "search",
            ),
            (
                &JavaScriptAnalyzer,
                "engine.js",
                "class Engine { search(q) { return !!q; } }",
                "search",
            ),
            (
                &CSharpAnalyzer,
                "Engine.cs",
                "namespace Demo { class Engine { bool Search(string q) { return q.Length > 0; } } }",
                "Search",
            ),
            (
                &GoAnalyzer,
                "engine.go",
                "type Engine struct {}\nfunc (engine *Engine) Search(query string) bool { return query != \"\" }\n",
                "Search",
            ),
        ];

        for (analyzer, path, source, method) in cases {
            let result = chunks(analyzer, path, source);
            let chunk = result
                .iter()
                .find(|chunk| chunk.symbol.as_deref() == Some(method))
                .unwrap_or_else(|| panic!("{path} did not produce {method}: {result:#?}"));
            assert_eq!(chunk.symbol_kind, Some(SymbolKind::Method));
            assert!(!chunk.stable_key.is_empty());
            assert!(chunk.qualified_symbol.as_deref().unwrap().contains(method));
            assert!(chunk.start_byte < chunk.end_byte);
            assert!(chunk.start_line <= chunk.end_line);
        }
    }

    #[test]
    fn structured_analyzers_emit_symbols_and_foundational_relationships() {
        let cases: Vec<(&dyn LanguageAnalyzer, &str, &str)> = vec![
            (
                &RustAnalyzer,
                "src/lib.rs",
                "struct Engine; impl Engine { fn run(&self) {} }",
            ),
            (
                &PythonAnalyzer,
                "engine.py",
                "class Engine:\n    def run(self):\n        pass\n",
            ),
            (
                &JavaScriptAnalyzer,
                "engine.js",
                "class Engine { run() {} }",
            ),
            (
                &TypeScriptAnalyzer,
                "engine.ts",
                "class Engine { run(): void {} }",
            ),
            (
                &CSharpAnalyzer,
                "Engine.cs",
                "class Engine { void Run() {} }",
            ),
            (
                &GoAnalyzer,
                "engine.go",
                "package engine\ntype Engine struct{}\nfunc (e Engine) Run() {}",
            ),
        ];
        for (analyzer, path, source) in cases {
            let result = analyzer.analyze(Path::new(path), source).unwrap();
            assert!(!result.chunks.is_empty());
            assert!(!result.symbols.is_empty());
            assert!(
                result
                    .relationships
                    .iter()
                    .any(|relationship| relationship.relationship
                        == crate::domain::GraphEdgeType::Contains)
            );
            assert!(result.relationships.iter().any(|relationship| {
                relationship.relationship == crate::domain::GraphEdgeType::DeclaredIn
            }));
            assert!(!analyzer.structure_version().is_empty());
            assert!(analyzer.capabilities().contains);
            assert!(analyzer.capabilities().declared_in);
        }
    }

    #[test]
    fn structural_metadata_links_methods_to_normalized_containers() {
        let cases: Vec<(&dyn LanguageAnalyzer, &str, &str, &str)> = vec![
            (
                &RustAnalyzer,
                "src/engine.rs",
                "struct Engine; impl Engine { fn search(&self) {} }",
                "search",
            ),
            (
                &PythonAnalyzer,
                "engine.py",
                "class Engine:\n    def search(self):\n        pass\n",
                "search",
            ),
            (
                &TypeScriptAnalyzer,
                "engine.ts",
                "class Engine { search(): void {} }",
                "search",
            ),
            (
                &CSharpAnalyzer,
                "Engine.cs",
                "class Engine { void Search() {} }",
                "Search",
            ),
        ];

        for (analyzer, path, source, method) in cases {
            let chunks = chunks(analyzer, path, source);
            let method = chunks
                .iter()
                .find(|chunk| chunk.symbol.as_deref() == Some(method))
                .unwrap_or_else(|| panic!("{path} did not produce method: {chunks:#?}"));
            let parent_key = method
                .metadata
                .get("parent_stable_key")
                .and_then(|value| value.as_str())
                .unwrap_or_else(|| panic!("{path} did not link method to a parent: {method:#?}"));
            assert_eq!(
                chunks
                    .iter()
                    .find(|chunk| chunk.stable_key == parent_key)
                    .and_then(|chunk| chunk.symbol.as_deref()),
                Some("Engine"),
            );
            assert_eq!(
                method
                    .metadata
                    .get("container_symbol")
                    .and_then(|value| value.as_str()),
                Some("Engine"),
            );
            assert_eq!(
                method
                    .metadata
                    .get("structural_depth")
                    .and_then(|value| value.as_u64()),
                Some(1),
            );
            assert_eq!(
                method
                    .metadata
                    .get("ordinal_in_container")
                    .and_then(|value| value.as_u64()),
                Some(0),
            );
        }
    }

    #[test]
    fn body_edits_insertions_and_formatting_preserve_logical_keys() {
        let rust_before =
            "impl Engine {\n    fn search(&self, q: &str) -> bool { q.is_empty() }\n}\n";
        let rust_after = "fn unrelated() {}\n\nimpl Engine {\n fn search( &self, q: &str ) -> bool { !q.is_empty() }\n}\n";
        assert_eq!(
            key_for(&chunks(&RustAnalyzer, "engine.rs", rust_before), "search"),
            key_for(&chunks(&RustAnalyzer, "engine.rs", rust_after), "search")
        );

        let python_before = "class Engine:\n    def search(self, q):\n        return q\n";
        let python_after = "def unrelated():\n    pass\n\nclass Engine:\n    def search(self, q):\n        return q.upper()\n";
        assert_eq!(
            key_for(
                &chunks(&PythonAnalyzer, "engine.py", python_before),
                "search"
            ),
            key_for(
                &chunks(&PythonAnalyzer, "engine.py", python_after),
                "search"
            )
        );

        let typescript_before = "class Engine { search(q: string) { return q; } }";
        let typescript_after =
            "const unrelated = () => 1; class Engine { search( q: string ) { return q.trim(); } }";
        assert_eq!(
            key_for(
                &chunks(&TypeScriptAnalyzer, "engine.ts", typescript_before),
                "search"
            ),
            key_for(
                &chunks(&TypeScriptAnalyzer, "engine.ts", typescript_after),
                "search"
            )
        );

        let go_before = "type Engine struct {}\nfunc (engine *Engine) Search(query string) bool { return query == \"\" }\n";
        let go_after = "func unrelated() {}\n\ntype Engine struct {}\nfunc (engine *Engine) Search( query string ) bool { return query != \"\" }\n";
        assert_eq!(
            key_for(&chunks(&GoAnalyzer, "engine.go", go_before), "Search"),
            key_for(&chunks(&GoAnalyzer, "engine.go", go_after), "Search")
        );
    }

    #[test]
    fn overloads_are_distinct_and_stable_when_reordered() {
        let before = "class Engine { int Search(string q) { return 1; } int Search(string q, int limit) { return 2; } }";
        let after = "class Engine { int Search(string q, int limit) { return 3; } int Search(string q) { return 4; } }";
        let keys = |source| {
            chunks(&CSharpAnalyzer, "Engine.cs", source)
                .into_iter()
                .filter(|chunk| chunk.symbol.as_deref() == Some("Search"))
                .map(|chunk| chunk.stable_key)
                .collect::<BTreeSet<_>>()
        };
        let before_keys = keys(before);
        assert_eq!(before_keys.len(), 2);
        assert_eq!(before_keys, keys(after));
    }

    #[test]
    fn duplicate_container_keys_keep_exact_child_links() {
        let chunks = chunks(
            &CSharpAnalyzer,
            "Engine.cs",
            "partial class Engine { void First() {} } partial class Engine { void Second() {} }",
        );
        let parent_keys: BTreeSet<_> = ["First", "Second"]
            .into_iter()
            .map(|method| {
                let method = chunks
                    .iter()
                    .find(|chunk| chunk.symbol.as_deref() == Some(method))
                    .unwrap();
                let parent_key = method
                    .metadata
                    .get("parent_stable_key")
                    .and_then(|value| value.as_str())
                    .unwrap();
                assert!(chunks.iter().any(|chunk| chunk.stable_key == parent_key));
                parent_key.to_owned()
            })
            .collect();

        assert_eq!(parent_keys.len(), 2);
    }

    #[test]
    fn nesting_disambiguates_names_and_deletion_removes_key() {
        let before = "class Outer { class Inner { void Run() {} } void Run() {} void Gone() {} }";
        let after = "class Outer { class Inner { void Run() {} } void Run() {} }";
        let before_chunks = chunks(&CSharpAnalyzer, "Nested.cs", before);
        let after_chunks = chunks(&CSharpAnalyzer, "Nested.cs", after);
        let run_keys = before_chunks
            .iter()
            .filter(|chunk| chunk.symbol.as_deref() == Some("Run"))
            .map(|chunk| chunk.stable_key.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(run_keys.len(), 2);
        let gone_key = key_for(&before_chunks, "Gone");
        assert!(
            !after_chunks
                .iter()
                .any(|chunk| chunk.stable_key == gone_key)
        );
        assert!(
            run_keys
                .iter()
                .all(|key| after_chunks.iter().any(|chunk| &chunk.stable_key == key))
        );
    }

    #[test]
    fn go_methods_are_receiver_qualified_and_deleted_symbols_disappear() {
        let before = "type First struct {}\ntype Second struct {}\nfunc (first *First) Run() {}\nfunc (second *Second) Run() {}\nfunc Gone() {}\n";
        let after = "type First struct {}\ntype Second struct {}\nfunc (first *First) Run() {}\nfunc (second *Second) Run() {}\n";
        let before_chunks = chunks(&GoAnalyzer, "engine.go", before);
        let after_chunks = chunks(&GoAnalyzer, "engine.go", after);
        let run_keys = before_chunks
            .iter()
            .filter(|chunk| chunk.symbol.as_deref() == Some("Run"))
            .map(|chunk| chunk.stable_key.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(run_keys.len(), 2);
        let gone_key = key_for(&before_chunks, "Gone");
        assert!(
            !after_chunks
                .iter()
                .any(|chunk| chunk.stable_key == gone_key)
        );
        assert!(
            run_keys
                .iter()
                .all(|key| after_chunks.iter().any(|chunk| &chunk.stable_key == key))
        );
    }

    #[test]
    fn declaration_chunks_include_decorators_exports_and_attributes() {
        let python = chunks(
            &PythonAnalyzer,
            "decorated.py",
            "@cached\ndef value():\n    return 1\n",
        );
        assert!(
            python
                .iter()
                .find(|chunk| chunk.symbol.as_deref() == Some("value"))
                .unwrap()
                .content
                .starts_with("@cached")
        );

        let javascript = chunks(
            &JavaScriptAnalyzer,
            "exported.js",
            "export function value() { return 1; }\n",
        );
        assert!(
            javascript
                .iter()
                .find(|chunk| chunk.symbol.as_deref() == Some("value"))
                .unwrap()
                .content
                .starts_with("export")
        );

        let rust = chunks(&RustAnalyzer, "attributed.rs", "#[inline]\nfn value() {}\n");
        assert!(
            rust.iter()
                .find(|chunk| chunk.symbol.as_deref() == Some("value"))
                .unwrap()
                .content
                .starts_with("#[inline]")
        );
    }
}
