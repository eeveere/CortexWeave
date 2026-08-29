use std::path::Path;

use crate::{
    Result,
    domain::{AnalysisResult, AnalyzerCapabilities},
};

pub trait LanguageAnalyzer: Send + Sync {
    fn language_id(&self) -> &'static str;
    fn analyzer_id(&self) -> &'static str;
    fn analyzer_version(&self) -> String;
    fn structure_version(&self) -> String;
    fn extensions(&self) -> &'static [&'static str];
    fn capabilities(&self) -> AnalyzerCapabilities;
    fn analyze(&self, path: &Path, source: &str) -> Result<AnalysisResult>;
}
