use std::{collections::HashMap, path::Path, sync::Arc};

use super::{
    GenericAnalyzer, LanguageAnalyzer,
    languages::{
        CSharpAnalyzer, GoAnalyzer, JavaScriptAnalyzer, PythonAnalyzer, RustAnalyzer,
        TypeScriptAnalyzer,
    },
};
use crate::config::{GenericChunkConfig, LanguageConfig};

pub struct AnalyzerRegistry {
    by_extension: HashMap<String, Arc<dyn LanguageAnalyzer>>,
    by_language: HashMap<String, Arc<dyn LanguageAnalyzer>>,
    fallback: Arc<dyn LanguageAnalyzer>,
}

impl AnalyzerRegistry {
    pub fn new(fallback: Arc<dyn LanguageAnalyzer>) -> Self {
        Self {
            by_extension: HashMap::new(),
            by_language: HashMap::new(),
            fallback,
        }
    }

    pub fn register(&mut self, analyzer: Arc<dyn LanguageAnalyzer>) {
        self.by_language
            .insert(analyzer.language_id().to_owned(), Arc::clone(&analyzer));
        for extension in analyzer.extensions() {
            self.by_extension.insert(
                extension.trim_start_matches('.').to_ascii_lowercase(),
                Arc::clone(&analyzer),
            );
        }
    }

    pub fn configured(languages: &LanguageConfig, generic: &GenericChunkConfig) -> Self {
        let mut registry = Self::new(Arc::new(GenericAnalyzer::new(
            generic.target_chars,
            generic.overlap_chars,
        )));
        if languages.rust {
            registry.register(Arc::new(RustAnalyzer));
        }
        if languages.python {
            registry.register(Arc::new(PythonAnalyzer));
        }
        if languages.javascript {
            registry.register(Arc::new(JavaScriptAnalyzer));
        }
        if languages.typescript {
            registry.register(Arc::new(TypeScriptAnalyzer));
        }
        if languages.csharp {
            registry.register(Arc::new(CSharpAnalyzer));
        }
        if languages.go {
            registry.register(Arc::new(GoAnalyzer));
        }
        registry
    }

    pub fn for_path(&self, path: &Path) -> Arc<dyn LanguageAnalyzer> {
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .and_then(|extension| self.by_extension.get(&extension))
            .cloned()
            .unwrap_or_else(|| Arc::clone(&self.fallback))
    }

    pub fn for_language(&self, language: &str) -> Option<Arc<dyn LanguageAnalyzer>> {
        self.by_language.get(language).cloned()
    }

    pub fn registered_languages(&self) -> Vec<String> {
        let mut languages: Vec<_> = self.by_language.keys().cloned().collect();
        languages.sort();
        languages
    }
}

impl Default for AnalyzerRegistry {
    fn default() -> Self {
        Self::new(Arc::new(GenericAnalyzer::default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_extension_uses_generic_fallback() {
        let registry = AnalyzerRegistry::default();
        assert_eq!(
            registry.for_path(Path::new("README.unknown")).analyzer_id(),
            "generic"
        );
    }

    #[test]
    fn configured_registry_selects_all_initial_languages() {
        let registry = AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        );
        let cases = [
            ("lib.rs", "rust"),
            ("main.py", "python"),
            ("app.js", "javascript"),
            ("app.tsx", "typescript"),
            ("Program.cs", "csharp"),
            ("main.go", "go"),
        ];
        for (path, language) in cases {
            let analyzer = registry.for_path(Path::new(path));
            assert_eq!(analyzer.language_id(), language);
            assert!(analyzer.capabilities().structural_chunks);
            assert!(analyzer.capabilities().qualified_symbols);
        }
    }
}
