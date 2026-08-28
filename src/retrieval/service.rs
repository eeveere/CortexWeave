use std::{collections::HashMap, sync::Arc, time::Instant};

use serde::{Deserialize, Serialize};

use crate::{
    CortexError, Result,
    domain::SymbolKind,
    embedding::EmbeddingProvider,
    instrumentation::RuntimeMetrics,
    storage::{CodeCandidate, LexicalCandidate, SemanticCandidate, SqliteStorage},
};

const MAX_RESULT_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalSource {
    Code,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalScores {
    pub semantic: Option<f32>,
    pub lexical: Option<f32>,
    pub hybrid: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalResult {
    pub source: RetrievalSource,
    pub workspace_id: String,
    pub chunk_id: String,
    pub language: String,
    pub path: String,
    pub symbol: Option<String>,
    pub qualified_symbol: Option<String>,
    pub symbol_kind: Option<SymbolKind>,
    pub start_byte: i64,
    pub end_byte: i64,
    pub start_line: i64,
    pub end_line: i64,
    pub content: String,
    pub scores: RetrievalScores,
}

pub struct RetrievalService {
    storage: Arc<SqliteStorage>,
    embeddings: Arc<dyn EmbeddingProvider>,
    semantic_weight: f32,
    lexical_weight: f32,
    metrics: Arc<RuntimeMetrics>,
}

impl RetrievalService {
    pub fn new(
        storage: Arc<SqliteStorage>,
        embeddings: Arc<dyn EmbeddingProvider>,
        semantic_weight: f32,
        lexical_weight: f32,
    ) -> Result<Self> {
        Self::with_metrics(
            storage,
            embeddings,
            semantic_weight,
            lexical_weight,
            Arc::new(RuntimeMetrics::default()),
        )
    }

    pub(crate) fn with_metrics(
        storage: Arc<SqliteStorage>,
        embeddings: Arc<dyn EmbeddingProvider>,
        semantic_weight: f32,
        lexical_weight: f32,
        metrics: Arc<RuntimeMetrics>,
    ) -> Result<Self> {
        if !semantic_weight.is_finite()
            || !lexical_weight.is_finite()
            || semantic_weight < 0.0
            || lexical_weight < 0.0
            || semantic_weight + lexical_weight == 0.0
        {
            return Err(CortexError::Configuration(
                "retrieval weights must be finite, non-negative, and non-zero in total".into(),
            ));
        }
        Ok(Self {
            storage,
            embeddings,
            semantic_weight,
            lexical_weight,
            metrics,
        })
    }

    pub async fn semantic_search(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        let started = Instant::now();
        let result = self.semantic_search_inner(workspace_id, query, limit).await;
        self.metrics.record_search(started.elapsed());
        result
    }

    async fn semantic_search_inner(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        let limit = limit.min(MAX_RESULT_LIMIT);
        if limit == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let vector = self.query_vector(query).await?;
        let candidates = self
            .storage
            .semantic_candidates(
                workspace_id,
                self.embeddings.embedding_space_id(),
                vector.len(),
            )
            .await?;
        let mut results: Vec<_> = candidates
            .into_iter()
            .filter_map(|candidate| {
                cosine_similarity(&vector, &candidate.vector).map(|score| (candidate, score))
            })
            .map(|(candidate, score)| semantic_result(candidate, score))
            .collect();
        results.sort_by(|left, right| {
            right
                .scores
                .semantic
                .unwrap_or(f32::NEG_INFINITY)
                .total_cmp(&left.scores.semantic.unwrap_or(f32::NEG_INFINITY))
                .then_with(|| left.chunk_id.cmp(&right.chunk_id))
        });
        results.truncate(limit);
        Ok(results)
    }

    pub async fn get_item(
        &self,
        workspace_id: &str,
        chunk_id: &str,
    ) -> Result<Option<RetrievalResult>> {
        Ok(self
            .storage
            .code_candidate(workspace_id, chunk_id)
            .await?
            .map(code_result))
    }

    pub async fn lexical_search(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        let started = Instant::now();
        let result = self.lexical_search_inner(workspace_id, query, limit).await;
        self.metrics.record_search(started.elapsed());
        result
    }

    async fn lexical_search_inner(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        let limit = limit.min(MAX_RESULT_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(match_query) = fts_query(query) else {
            return Ok(Vec::new());
        };
        Ok(self
            .storage
            .lexical_candidates(workspace_id, &match_query, limit)
            .await?
            .into_iter()
            .map(lexical_result)
            .collect())
    }

    pub async fn hybrid_search(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        let started = Instant::now();
        let result = self.hybrid_search_inner(workspace_id, query, limit).await;
        self.metrics.record_search(started.elapsed());
        result
    }

    async fn hybrid_search_inner(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        let limit = limit.min(MAX_RESULT_LIMIT);
        if limit == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let candidate_limit = limit.saturating_mul(3).max(limit);
        let (semantic, lexical) = tokio::join!(
            self.semantic_search_inner(workspace_id, query, candidate_limit),
            self.lexical_search_inner(workspace_id, query, candidate_limit)
        );
        let semantic = semantic?;
        let lexical = lexical?;
        let semantic_scores = normalized_component_scores(semantic.iter().filter_map(|result| {
            result
                .scores
                .semantic
                .map(|score| (result.chunk_id.clone(), score))
        }));
        let lexical_scores = normalized_component_scores(lexical.iter().filter_map(|result| {
            result
                .scores
                .lexical
                .map(|score| (result.chunk_id.clone(), score))
        }));
        let mut merged: HashMap<String, RetrievalResult> = HashMap::new();
        for result in semantic.into_iter().chain(lexical) {
            merged.entry(result.chunk_id.clone()).or_insert(result);
        }
        let total_weight = self.semantic_weight + self.lexical_weight;
        let mut results: Vec<_> = merged
            .into_values()
            .map(|mut result| {
                let semantic = semantic_scores.get(&result.chunk_id).copied();
                let lexical = lexical_scores.get(&result.chunk_id).copied();
                let hybrid = (semantic.unwrap_or_default() * self.semantic_weight
                    + lexical.unwrap_or_default() * self.lexical_weight)
                    / total_weight;
                result.scores.semantic = semantic;
                result.scores.lexical = lexical;
                result.scores.hybrid = Some(hybrid);
                result
            })
            .collect();
        results.sort_by(|left, right| {
            right
                .scores
                .hybrid
                .unwrap_or(f32::NEG_INFINITY)
                .total_cmp(&left.scores.hybrid.unwrap_or(f32::NEG_INFINITY))
                .then_with(|| left.chunk_id.cmp(&right.chunk_id))
        });
        results.truncate(limit);
        Ok(results)
    }

    async fn query_vector(&self, query: &str) -> Result<Vec<f32>> {
        let prepared = self.embeddings.prepare_query_input(query);
        let measured = self.embeddings.count_tokens(&prepared);
        let limits = self.embeddings.limits();
        if let Some(maximum) = limits.input_budget()
            && measured.tokens > maximum
        {
            return Err(CortexError::Embedding(format!(
                "semantic query is {} tokens after query transformation, exceeding the effective maximum of {maximum}; shorten the query",
                measured.tokens
            )));
        }
        if let Some(maximum) = limits.max_batch_tokens
            && measured.tokens > maximum
        {
            return Err(CortexError::Embedding(format!(
                "semantic query is {} tokens after query transformation, exceeding the batch maximum of {maximum}; shorten the query",
                measured.tokens
            )));
        }
        let started = Instant::now();
        let vectors = self.embeddings.embed_queries(&[query.to_owned()]).await;
        self.metrics
            .record_embedding(1, self.embeddings.batch_size(), started.elapsed());
        let vectors = vectors?;
        if vectors.len() != 1 {
            return Err(CortexError::Embedding(format!(
                "provider returned {} vectors for one query",
                vectors.len()
            )));
        }
        let vector = vectors.into_iter().next().expect("checked vector count");
        if vector.is_empty() || vector.iter().any(|value| !value.is_finite()) {
            return Err(CortexError::Embedding(
                "provider returned an invalid query vector".into(),
            ));
        }
        if self
            .embeddings
            .dimension()
            .is_some_and(|expected| vector.len() != expected)
        {
            return Err(CortexError::Embedding(format!(
                "provider declared dimension {:?} but returned {}",
                self.embeddings.dimension(),
                vector.len()
            )));
        }
        Ok(vector)
    }
}

fn code_result(candidate: CodeCandidate) -> RetrievalResult {
    result_from_chunk(
        candidate.workspace_id,
        candidate.relative_path,
        candidate.chunk,
        RetrievalScores {
            semantic: None,
            lexical: None,
            hybrid: None,
        },
    )
}

fn semantic_result(candidate: SemanticCandidate, score: f32) -> RetrievalResult {
    result_from_chunk(
        candidate.workspace_id,
        candidate.relative_path,
        candidate.chunk,
        RetrievalScores {
            semantic: Some(score),
            lexical: None,
            hybrid: None,
        },
    )
}

fn lexical_result(candidate: LexicalCandidate) -> RetrievalResult {
    result_from_chunk(
        candidate.workspace_id,
        candidate.relative_path,
        candidate.chunk,
        RetrievalScores {
            semantic: None,
            lexical: Some(candidate.score),
            hybrid: None,
        },
    )
}

fn result_from_chunk(
    workspace_id: String,
    path: String,
    chunk: crate::domain::StoredChunk,
    scores: RetrievalScores,
) -> RetrievalResult {
    RetrievalResult {
        source: RetrievalSource::Code,
        workspace_id,
        chunk_id: chunk.id,
        language: chunk.language,
        path,
        symbol: chunk.symbol,
        qualified_symbol: chunk.qualified_symbol,
        symbol_kind: chunk.symbol_kind,
        start_byte: chunk.start_byte,
        end_byte: chunk.end_byte,
        start_line: chunk.start_line,
        end_line: chunk.end_line,
        content: chunk.content,
        scores,
    }
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let (dot, left_norm, right_norm) = left.iter().zip(right).fold(
        (0.0_f64, 0.0_f64, 0.0_f64),
        |(dot, left_norm, right_norm), (left, right)| {
            (
                dot + f64::from(*left) * f64::from(*right),
                left_norm + f64::from(*left) * f64::from(*left),
                right_norm + f64::from(*right) * f64::from(*right),
            )
        },
    );
    if left_norm == 0.0 || right_norm == 0.0 {
        return None;
    }
    Some((dot / (left_norm.sqrt() * right_norm.sqrt())) as f32)
}

fn normalized_component_scores(
    entries: impl IntoIterator<Item = (String, f32)>,
) -> HashMap<String, f32> {
    let entries: Vec<_> = entries.into_iter().collect();
    let Some(minimum) = entries.iter().map(|(_, score)| *score).reduce(f32::min) else {
        return HashMap::new();
    };
    let maximum = entries
        .iter()
        .map(|(_, score)| *score)
        .reduce(f32::max)
        .unwrap_or(minimum);
    entries
        .into_iter()
        .map(|(id, score)| {
            let normalized = if maximum == minimum {
                1.0
            } else {
                (score - minimum) / (maximum - minimum)
            };
            (id, normalized)
        })
        .collect()
}

fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<_> = query
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != ':'
        })
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.replace('"', "")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::{
        domain::{Document, EmbeddingRecord, StoredChunk, Workspace},
        embedding::EmbeddingLimits,
        storage::SqliteStorage,
    };

    struct LookupProvider;

    #[async_trait]
    impl EmbeddingProvider for LookupProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .map(|text| {
                    if text.contains("embedding") || text.contains("Embedding") {
                        vec![1.0, 0.0]
                    } else {
                        vec![0.0, 1.0]
                    }
                })
                .collect())
        }

        fn model_name(&self) -> &str {
            "lookup"
        }

        fn limits(&self) -> EmbeddingLimits {
            EmbeddingLimits {
                max_input_tokens: Some(32),
                max_batch_tokens: Some(32),
                max_batch_items: 1,
                reserved_tokens: 0,
            }
        }
    }

    async fn populated_service() -> (RetrievalService, Workspace, Arc<SqliteStorage>) {
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/retrieval", "retrieval");
        storage.insert_workspace(&workspace).await.unwrap();
        let document = Document {
            id: "document".into(),
            workspace_id: workspace.id.clone(),
            relative_path: "src/reconcile.rs".into(),
            language: "rust".into(),
            analyzer_id: "test".into(),
            analyzer_version: "1".into(),
            segmentation_id: "test-segmentation".into(),
            content_hash: "hash".into(),
            size_bytes: 1,
            modified_at_ns: None,
            indexed_at: Utc::now(),
        };
        let mut embedding = StoredChunk::new(
            &document.id,
            "src/reconcile.rs::struct:EmbeddingProvider",
            "pub trait EmbeddingProvider { fn embed(); }",
        );
        embedding.language = "rust".into();
        embedding.symbol = Some("EmbeddingProvider".into());
        embedding.qualified_symbol = Some("EmbeddingProvider".into());
        embedding.start_line = 2;
        embedding.end_line = 4;
        embedding.metadata = json!({});
        let mut reconcile = StoredChunk::new(
            &document.id,
            "src/reconcile.rs::function:reconcile_document",
            "fn reconcile_document() { /* E0425 retry */ }",
        );
        reconcile.language = "rust".into();
        reconcile.symbol = Some("reconcile_document".into());
        reconcile.qualified_symbol = Some("reconcile_document".into());
        reconcile.start_line = 10;
        reconcile.end_line = 12;
        reconcile.metadata = json!({});
        let vectors = vec![
            EmbeddingRecord::new(&embedding.id, "lookup", vec![1.0, 0.0]),
            EmbeddingRecord::new(&reconcile.id, "lookup", vec![0.0, 1.0]),
        ];
        storage
            .persist_document_tree(&document, &[embedding, reconcile], &vectors)
            .await
            .unwrap();
        (
            RetrievalService::new(Arc::clone(&storage), Arc::new(LookupProvider), 0.7, 0.3)
                .unwrap(),
            workspace,
            storage,
        )
    }

    #[tokio::test]
    async fn semantic_search_returns_compatible_provenance_without_vectors() {
        let (service, workspace, _) = populated_service().await;
        let results = service
            .semantic_search(&workspace.id, "embedding behavior", 4)
            .await
            .unwrap();
        assert_eq!(results[0].symbol.as_deref(), Some("EmbeddingProvider"));
        assert_eq!(results[0].source, RetrievalSource::Code);
        assert_eq!(results[0].path, "src/reconcile.rs");
        assert!(results[0].scores.semantic.unwrap() > 0.9);
        assert_eq!(results[0].scores.lexical, None);
    }

    #[tokio::test]
    async fn lexical_search_handles_identifiers_errors_and_symbols() {
        let (service, workspace, _) = populated_service().await;
        let identifier = service
            .lexical_search(&workspace.id, "EmbeddingProvider", 4)
            .await
            .unwrap();
        let error = service
            .lexical_search(&workspace.id, "E0425", 4)
            .await
            .unwrap();
        let symbol = service
            .lexical_search(&workspace.id, "reconcile_document", 4)
            .await
            .unwrap();
        assert_eq!(identifier[0].symbol.as_deref(), Some("EmbeddingProvider"));
        assert_eq!(error[0].symbol.as_deref(), Some("reconcile_document"));
        assert_eq!(symbol[0].symbol.as_deref(), Some("reconcile_document"));
        assert!(identifier[0].scores.lexical.is_some());
    }

    #[tokio::test]
    async fn hybrid_search_deduplicates_and_keeps_component_scores() {
        let (service, workspace, _) = populated_service().await;
        let results = service
            .hybrid_search(&workspace.id, "EmbeddingProvider", 4)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].symbol.as_deref(), Some("EmbeddingProvider"));
        assert!(results.iter().all(|result| result.scores.hybrid.is_some()));
        assert_eq!(
            results
                .iter()
                .filter(|result| result.symbol.as_deref() == Some("EmbeddingProvider"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn semantic_search_excludes_incompatible_embedding_models() {
        let (service, workspace, storage) = populated_service().await;
        let document = Document {
            id: "old-document".into(),
            workspace_id: workspace.id.clone(),
            relative_path: "src/old.rs".into(),
            language: "rust".into(),
            analyzer_id: "test".into(),
            analyzer_version: "1".into(),
            segmentation_id: "test-segmentation".into(),
            content_hash: "old-hash".into(),
            size_bytes: 1,
            modified_at_ns: None,
            indexed_at: Utc::now(),
        };
        let mut stale = StoredChunk::new(
            &document.id,
            "src/old.rs::function:stale",
            "fn stale_embedding() {}",
        );
        stale.symbol = Some("stale_embedding".into());
        storage
            .persist_document_tree(
                &document,
                std::slice::from_ref(&stale),
                &[EmbeddingRecord::new(&stale.id, "old-model", vec![1.0, 0.0])],
            )
            .await
            .unwrap();

        let results = service
            .semantic_search(&workspace.id, "embedding behavior", 10)
            .await
            .unwrap();
        assert!(
            results
                .iter()
                .all(|result| result.symbol.as_deref() != Some("stale_embedding"))
        );
    }

    #[tokio::test]
    async fn rejects_query_over_effective_input_limit_without_truncating() {
        let (service, workspace, _) = populated_service().await;
        let error = service
            .semantic_search(&workspace.id, &"x".repeat(33), 4)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("effective maximum of 32"));
    }
}
