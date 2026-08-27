mod analyzer;
mod generic;
pub mod languages;
mod registry;
mod tree_sitter_support;

pub use analyzer::LanguageAnalyzer;
pub use generic::GenericAnalyzer;
pub use registry::AnalyzerRegistry;
