use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{CortexError, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub embedding: EmbeddingConfig,
    pub indexing: IndexingConfig,
    pub retrieval: RetrievalConfig,
    pub working_set: WorkingSetConfig,
    pub temporal: TemporalConfig,
    pub context: ContextConfig,
    pub logging: LoggingConfig,
    pub languages: LanguageConfig,
}

impl AppConfig {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let config = if let Some(path) = path {
            let source = fs::read_to_string(path).map_err(|error| CortexError::Io {
                path: path.to_path_buf(),
                source: error,
            })?;
            toml::from_str(&source)
                .map_err(|error| CortexError::Configuration(error.to_string()))?
        } else {
            Self::default()
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        self.embedding.validate()?;
        self.indexing.validate()?;
        self.retrieval.validate()?;
        self.working_set.validate()?;
        self.temporal.validate()?;
        self.context.validate()
    }
}

pub const MAX_CONTEXT_CANDIDATE_POOL_LIMIT: usize = 10_000;
pub const MAX_CONTEXT_STRUCTURAL_EXPANSION_LIMIT: usize = 64;
pub const MAX_CONTEXT_EXPERIENCE_CANDIDATE_LIMIT: usize = 50;
pub const MAX_CONTEXT_EXPERIENCE_TOKEN_BUDGET: usize = 2_048;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub mcp_transport: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            mcp_transport: "stdio".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub path: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: ".cortexweave/cortexweave.db".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    pub base_url: String,
    pub endpoint: String,
    pub model: String,
    pub dimension: Option<usize>,
    pub batch_size: usize,
    pub timeout_seconds: u64,
    pub document_prefix: String,
    pub query_prefix: String,
    pub limits: EmbeddingLimitConfig,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8081".into(),
            endpoint: "/v1/embeddings".into(),
            model: "local-embedding-model".into(),
            dimension: None,
            batch_size: 16,
            timeout_seconds: 30,
            document_prefix: String::new(),
            query_prefix: String::new(),
            limits: EmbeddingLimitConfig::default(),
        }
    }
}

impl EmbeddingConfig {
    pub fn validate(&self) -> Result<()> {
        if self.model.trim().is_empty() {
            return Err(CortexError::Configuration(
                "embedding.model cannot be empty".into(),
            ));
        }
        if self.batch_size == 0 {
            return Err(CortexError::Configuration(
                "embedding.batch_size must be greater than zero".into(),
            ));
        }
        if self.timeout_seconds == 0 {
            return Err(CortexError::Configuration(
                "embedding.timeout_seconds must be greater than zero".into(),
            ));
        }
        if self.dimension == Some(0) {
            return Err(CortexError::Configuration(
                "embedding.dimension must be greater than zero when set".into(),
            ));
        }
        self.limits.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingLimitConfig {
    pub max_input_tokens: Option<usize>,
    pub max_batch_tokens: Option<usize>,
    pub reserved_tokens: usize,
    pub tokenizer: TokenizerKind,
    pub tokenizer_path: Option<String>,
}

impl EmbeddingLimitConfig {
    fn validate(&self) -> Result<()> {
        if self.max_input_tokens == Some(0) {
            return Err(CortexError::Configuration(
                "embedding.limits.max_input_tokens must be greater than zero when set".into(),
            ));
        }
        if self.max_batch_tokens == Some(0) {
            return Err(CortexError::Configuration(
                "embedding.limits.max_batch_tokens must be greater than zero when set".into(),
            ));
        }
        if self
            .max_input_tokens
            .is_some_and(|limit| self.reserved_tokens >= limit)
        {
            return Err(CortexError::Configuration(
                "embedding.limits.reserved_tokens must be smaller than max_input_tokens".into(),
            ));
        }
        if self.tokenizer == TokenizerKind::HuggingFace
            && self
                .tokenizer_path
                .as_deref()
                .is_none_or(|path| path.trim().is_empty())
        {
            return Err(CortexError::Configuration(
                "embedding.limits.tokenizer_path is required for the hugging_face tokenizer".into(),
            ));
        }
        Ok(())
    }
}

impl Default for EmbeddingLimitConfig {
    fn default() -> Self {
        Self {
            max_input_tokens: None,
            max_batch_tokens: None,
            reserved_tokens: 32,
            tokenizer: TokenizerKind::ConservativeBytes,
            tokenizer_path: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenizerKind {
    #[default]
    ConservativeBytes,
    #[serde(rename = "huggingface", alias = "hugging_face")]
    HuggingFace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexingConfig {
    pub debounce_ms: u64,
    pub max_file_bytes: u64,
    pub max_concurrent_embedding_jobs: usize,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub generic_chunks: GenericChunkConfig,
    pub embedding_segments: EmbeddingSegmentConfig,
}

impl Default for IndexingConfig {
    fn default() -> Self {
        Self {
            debounce_ms: 300,
            max_file_bytes: 1_048_576,
            max_concurrent_embedding_jobs: 2,
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            generic_chunks: GenericChunkConfig::default(),
            embedding_segments: EmbeddingSegmentConfig::default(),
        }
    }
}

impl IndexingConfig {
    fn validate(&self) -> Result<()> {
        if self.max_concurrent_embedding_jobs == 0 {
            return Err(CortexError::Configuration(
                "indexing.max_concurrent_embedding_jobs must be greater than zero".into(),
            ));
        }
        if self.max_file_bytes == 0 {
            return Err(CortexError::Configuration(
                "indexing.max_file_bytes must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingSegmentConfig {
    pub overlap_tokens: usize,
}

impl Default for EmbeddingSegmentConfig {
    fn default() -> Self {
        Self { overlap_tokens: 64 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GenericChunkConfig {
    pub target_chars: usize,
    pub overlap_chars: usize,
}

impl Default for GenericChunkConfig {
    fn default() -> Self {
        Self {
            target_chars: 3_000,
            overlap_chars: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetrievalConfig {
    pub default_k: usize,
    pub semantic_weight: f32,
    pub lexical_weight: f32,
    pub structural: StructuralRetrievalConfig,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            default_k: 8,
            semantic_weight: 0.55,
            lexical_weight: 0.25,
            structural: StructuralRetrievalConfig::default(),
        }
    }
}

impl RetrievalConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.default_k == 0
            || !self.semantic_weight.is_finite()
            || self.semantic_weight < 0.0
            || !self.lexical_weight.is_finite()
            || self.lexical_weight < 0.0
            || self.semantic_weight + self.lexical_weight <= 0.0
        {
            return Err(CortexError::Configuration(
                "retrieval limits and weights must be finite, non-negative, and non-zero in total"
                    .into(),
            ));
        }
        self.structural.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StructuralRetrievalConfig {
    pub enabled: bool,
    pub weight: f32,
    pub max_depth: usize,
    pub candidate_limit: usize,
    pub distance_decay: f32,
    pub calls_weight: f32,
    pub references_weight: f32,
    pub implementations_weight: f32,
    pub tests_weight: f32,
    pub dependencies_weight: f32,
    pub other_weight: f32,
}

impl Default for StructuralRetrievalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            weight: 0.20,
            max_depth: 3,
            candidate_limit: 32,
            distance_decay: 0.65,
            calls_weight: 1.0,
            references_weight: 0.8,
            implementations_weight: 1.0,
            tests_weight: 1.0,
            dependencies_weight: 0.8,
            other_weight: 0.6,
        }
    }
}

impl StructuralRetrievalConfig {
    fn validate(&self) -> Result<()> {
        let weights = [
            self.weight,
            self.calls_weight,
            self.references_weight,
            self.implementations_weight,
            self.tests_weight,
            self.dependencies_weight,
            self.other_weight,
        ];
        if weights
            .iter()
            .any(|weight| !weight.is_finite() || *weight < 0.0)
            || !self.distance_decay.is_finite()
            || !(0.0..=1.0).contains(&self.distance_decay)
            || self.max_depth == 0
            || self.max_depth > crate::domain::MAX_STRUCTURAL_DEPTH
            || self.candidate_limit == 0
            || self.candidate_limit > crate::domain::MAX_STRUCTURAL_NODES
        {
            return Err(CortexError::Configuration(
                "retrieval.structural requires finite non-negative weights, distance_decay in 0..=1, and bounded depth/candidate limits"
                    .into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkingSetConfig {
    pub enabled: bool,
    pub decay_half_life_minutes: f64,
    pub activation_increment: f32,
    pub max_activation_score: f32,
    pub min_activation_score: f32,
    pub max_items: usize,
}

impl Default for WorkingSetConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            decay_half_life_minutes: 90.0,
            activation_increment: 0.35,
            max_activation_score: 1.0,
            min_activation_score: 0.01,
            max_items: 100,
        }
    }
}

impl WorkingSetConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if !self.decay_half_life_minutes.is_finite() || self.decay_half_life_minutes <= 0.0 {
            return Err(CortexError::Configuration(
                "working_set.decay_half_life_minutes must be finite and greater than zero".into(),
            ));
        }
        if !self.activation_increment.is_finite() || self.activation_increment <= 0.0 {
            return Err(CortexError::Configuration(
                "working_set.activation_increment must be finite and greater than zero".into(),
            ));
        }
        if !self.max_activation_score.is_finite() || self.max_activation_score <= 0.0 {
            return Err(CortexError::Configuration(
                "working_set.max_activation_score must be finite and greater than zero".into(),
            ));
        }
        if !self.min_activation_score.is_finite()
            || self.min_activation_score < 0.0
            || self.min_activation_score >= self.max_activation_score
        {
            return Err(CortexError::Configuration(
                "working_set.min_activation_score must be finite, non-negative, and smaller than max_activation_score".into(),
            ));
        }
        if self.max_items == 0 {
            return Err(CortexError::Configuration(
                "working_set.max_items must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TemporalConfig {
    pub recency_half_life_hours: f64,
}

impl Default for TemporalConfig {
    fn default() -> Self {
        Self {
            recency_half_life_hours: 72.0,
        }
    }
}

impl TemporalConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if !self.recency_half_life_hours.is_finite() || self.recency_half_life_hours <= 0.0 {
            return Err(CortexError::Configuration(
                "temporal.recency_half_life_hours must be finite and greater than zero".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    pub candidate_pool_limit: usize,
    pub structural_expansion_limit: usize,
    pub experience: ExperienceContextConfig,
    pub ranking: ContextRankingConfig,
    pub budget: ContextBudgetConfig,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            candidate_pool_limit: 50,
            structural_expansion_limit: 4,
            experience: ExperienceContextConfig::default(),
            ranking: ContextRankingConfig::default(),
            budget: ContextBudgetConfig::default(),
        }
    }
}

impl ContextConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.candidate_pool_limit == 0 {
            return Err(CortexError::Configuration(
                "context.candidate_pool_limit must be greater than zero".into(),
            ));
        }
        if self.candidate_pool_limit > MAX_CONTEXT_CANDIDATE_POOL_LIMIT {
            return Err(CortexError::Configuration(format!(
                "context.candidate_pool_limit must not exceed {MAX_CONTEXT_CANDIDATE_POOL_LIMIT}"
            )));
        }
        if self.structural_expansion_limit == 0 {
            return Err(CortexError::Configuration(
                "context.structural_expansion_limit must be greater than zero".into(),
            ));
        }
        if self.structural_expansion_limit > MAX_CONTEXT_STRUCTURAL_EXPANSION_LIMIT {
            return Err(CortexError::Configuration(format!(
                "context.structural_expansion_limit must not exceed {MAX_CONTEXT_STRUCTURAL_EXPANSION_LIMIT}"
            )));
        }
        self.experience.validate()?;
        self.ranking.validate()?;
        self.budget.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExperienceContextConfig {
    pub enabled: bool,
    pub candidate_limit: usize,
    pub token_budget: usize,
}

impl Default for ExperienceContextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            candidate_limit: 8,
            token_budget: 768,
        }
    }
}

impl ExperienceContextConfig {
    fn validate(&self) -> Result<()> {
        if self.candidate_limit == 0
            || self.candidate_limit > MAX_CONTEXT_EXPERIENCE_CANDIDATE_LIMIT
        {
            return Err(CortexError::Configuration(format!(
                "context.experience.candidate_limit must be between 1 and {MAX_CONTEXT_EXPERIENCE_CANDIDATE_LIMIT}"
            )));
        }
        if self.token_budget > MAX_CONTEXT_EXPERIENCE_TOKEN_BUDGET {
            return Err(CortexError::Configuration(format!(
                "context.experience.token_budget must not exceed {MAX_CONTEXT_EXPERIENCE_TOKEN_BUDGET}"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextBudgetConfig {
    pub code_fraction: f32,
    pub structural_fraction: f32,
    pub memory_fraction: f32,
    pub event_fraction: f32,
    pub state_fraction: f32,
}

impl Default for ContextBudgetConfig {
    fn default() -> Self {
        Self {
            code_fraction: 0.50,
            structural_fraction: 0.20,
            memory_fraction: 0.15,
            event_fraction: 0.10,
            state_fraction: 0.05,
        }
    }
}

impl ContextBudgetConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        let fractions = [
            self.code_fraction,
            self.structural_fraction,
            self.memory_fraction,
            self.event_fraction,
            self.state_fraction,
        ];
        if fractions
            .iter()
            .any(|fraction| !fraction.is_finite() || *fraction < 0.0)
            || fractions.iter().sum::<f32>() > 1.0
        {
            return Err(CortexError::Configuration(
                "context budget fractions must be finite, non-negative, and sum to at most one"
                    .into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextRankingConfig {
    pub semantic_weight: f32,
    pub lexical_weight: f32,
    pub task_weight: f32,
    pub working_set_weight: f32,
    pub recency_weight: f32,
    pub provenance_weight: f32,
    pub freshness_weight: f32,
    pub structural_weight: f32,
}

impl Default for ContextRankingConfig {
    fn default() -> Self {
        Self {
            semantic_weight: 0.24,
            lexical_weight: 0.16,
            task_weight: 0.14,
            working_set_weight: 0.14,
            recency_weight: 0.10,
            provenance_weight: 0.08,
            freshness_weight: 0.08,
            structural_weight: 0.06,
        }
    }
}

impl ContextRankingConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        let weights = [
            self.semantic_weight,
            self.lexical_weight,
            self.task_weight,
            self.working_set_weight,
            self.recency_weight,
            self.provenance_weight,
            self.freshness_weight,
            self.structural_weight,
        ];
        if weights
            .iter()
            .any(|weight| !weight.is_finite() || *weight < 0.0)
            || weights.iter().sum::<f32>() <= 0.0
        {
            return Err(CortexError::Configuration(
                "context ranking weights must be finite, non-negative, and non-zero in total"
                    .into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn total_weight(&self) -> f32 {
        self.semantic_weight
            + self.lexical_weight
            + self.task_weight
            + self.working_set_weight
            + self.recency_weight
            + self.provenance_weight
            + self.freshness_weight
            + self.structural_weight
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LanguageConfig {
    pub rust: bool,
    pub python: bool,
    pub javascript: bool,
    pub typescript: bool,
    pub csharp: bool,
    pub go: bool,
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            rust: true,
            python: true,
            javascript: true,
            typescript: true,
            csharp: true,
            go: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_configuration_keeps_defaults() {
        let config: AppConfig = toml::from_str("[logging]\nlevel = 'debug'\n").unwrap();
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.retrieval.default_k, 8);
        assert!(config.retrieval.structural.enabled);
        assert_eq!(config.retrieval.structural.weight, 0.20);
        assert_eq!(config.retrieval.structural.max_depth, 3);
        assert_eq!(config.working_set.max_items, 100);
        assert_eq!(config.temporal.recency_half_life_hours, 72.0);
        assert_eq!(config.context.candidate_pool_limit, 50);
        assert_eq!(config.context.structural_expansion_limit, 4);
        assert!(config.context.experience.enabled);
        assert_eq!(config.context.experience.candidate_limit, 8);
        assert_eq!(config.context.experience.token_budget, 768);
        assert_eq!(config.context.ranking.semantic_weight, 0.24);
        assert_eq!(config.context.budget.code_fraction, 0.50);
        assert!(config.languages.rust);
        assert_eq!(
            config.embedding.limits.tokenizer,
            TokenizerKind::ConservativeBytes
        );
    }

    #[test]
    fn validates_embedding_capacity_limits() {
        let invalid: AppConfig =
            toml::from_str("[embedding.limits]\nmax_input_tokens = 32\nreserved_tokens = 32\n")
                .unwrap();
        assert!(invalid.validate().is_err());

        let invalid: AppConfig = toml::from_str("[context.budget]\ncode_fraction = 1.1").unwrap();
        assert!(invalid.validate().is_err());

        let valid: AppConfig = toml::from_str(
            "[embedding.limits]\nmax_input_tokens = 2048\nmax_batch_tokens = 4096\nreserved_tokens = 32\n",
        )
        .unwrap();
        valid.validate().unwrap();

        let independent_batch: AppConfig = toml::from_str(
            "[embedding.limits]\nmax_input_tokens = 2048\nmax_batch_tokens = 2015\nreserved_tokens = 32\n",
        )
        .unwrap();
        independent_batch.validate().unwrap();

        let missing_tokenizer: AppConfig = toml::from_str(
            "[embedding.limits]\ntokenizer = 'huggingface'\nmax_input_tokens = 2048\n",
        )
        .unwrap();
        assert!(missing_tokenizer.validate().is_err());

        let exact_tokenizer: AppConfig = toml::from_str(
            "[embedding.limits]\ntokenizer = 'hugging_face'\ntokenizer_path = 'tokenizer.json'\nmax_input_tokens = 2048\n",
        )
        .unwrap();
        exact_tokenizer.validate().unwrap();
    }

    #[test]
    fn validates_working_set_coefficients() {
        let invalid_half_life: AppConfig =
            toml::from_str("[working_set]\ndecay_half_life_minutes = 0").unwrap();
        assert!(invalid_half_life.validate().is_err());

        let invalid_thresholds: AppConfig =
            toml::from_str("[working_set]\nmin_activation_score = 1.0\nmax_activation_score = 1.0")
                .unwrap();
        assert!(invalid_thresholds.validate().is_err());

        let valid: AppConfig = toml::from_str(
            "[working_set]\ndecay_half_life_minutes = 45\nactivation_increment = 0.25\nmax_activation_score = 2.0\nmin_activation_score = 0.05\nmax_items = 50",
        )
        .unwrap();
        valid.validate().unwrap();
    }

    #[test]
    fn validates_temporal_recency_half_life() {
        let invalid: AppConfig = toml::from_str("[temporal]\nrecency_half_life_hours = 0").unwrap();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn validates_candidate_pool_limit() {
        let invalid: AppConfig = toml::from_str("[context]\ncandidate_pool_limit = 0").unwrap();
        assert!(invalid.validate().is_err());

        let invalid: AppConfig =
            toml::from_str("[context]\nstructural_expansion_limit = 0").unwrap();
        assert!(invalid.validate().is_err());

        let invalid: AppConfig = toml::from_str(&format!(
            "[context]\ncandidate_pool_limit = {}",
            MAX_CONTEXT_CANDIDATE_POOL_LIMIT + 1
        ))
        .unwrap();
        assert!(invalid.validate().is_err());

        let invalid: AppConfig =
            toml::from_str("[context.experience]\ncandidate_limit = 0").unwrap();
        assert!(invalid.validate().is_err());

        let invalid: AppConfig = toml::from_str(&format!(
            "[context.experience]\ntoken_budget = {}",
            MAX_CONTEXT_EXPERIENCE_TOKEN_BUDGET + 1
        ))
        .unwrap();
        assert!(invalid.validate().is_err());

        let invalid: AppConfig = toml::from_str(&format!(
            "[context]\nstructural_expansion_limit = {}",
            MAX_CONTEXT_STRUCTURAL_EXPANSION_LIMIT + 1
        ))
        .unwrap();
        assert!(invalid.validate().is_err());

        let invalid: AppConfig = toml::from_str("[context.ranking]\nsemantic_weight = -1").unwrap();
        assert!(invalid.validate().is_err());

        let invalid: AppConfig = toml::from_str(
            "[context.ranking]\nsemantic_weight = 0\nlexical_weight = 0\ntask_weight = 0\nworking_set_weight = 0\nrecency_weight = 0\nprovenance_weight = 0\nfreshness_weight = 0\nstructural_weight = 0",
        )
        .unwrap();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn validates_structural_retrieval_bounds_and_weights() {
        let no_baseline: AppConfig = toml::from_str(
            "[retrieval]\nsemantic_weight = 0\nlexical_weight = 0\n[retrieval.structural]\nweight = 1",
        )
        .unwrap();
        assert!(no_baseline.validate().is_err());

        let invalid_depth: AppConfig =
            toml::from_str("[retrieval.structural]\nmax_depth = 0").unwrap();
        assert!(invalid_depth.validate().is_err());

        let invalid_decay: AppConfig =
            toml::from_str("[retrieval.structural]\ndistance_decay = 1.1").unwrap();
        assert!(invalid_decay.validate().is_err());

        let valid: AppConfig = toml::from_str(
            "[retrieval.structural]\nenabled = false\nweight = 0\nmax_depth = 1\ncandidate_limit = 1",
        )
        .unwrap();
        valid.validate().unwrap();
    }
}
