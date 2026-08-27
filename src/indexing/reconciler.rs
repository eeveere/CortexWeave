use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::{Instant, UNIX_EPOCH},
};

use chrono::Utc;
use serde::Serialize;
use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

use crate::{
    CortexError, Result,
    config::IndexingConfig,
    domain::{Document, EmbeddingRecord, StoredChunk, Workspace},
    embedding::EmbeddingProvider,
    instrumentation::RuntimeMetrics,
    parsing::AnalyzerRegistry,
    storage::SqliteStorage,
    workspace::{PathIdentity, WorkspaceScanner},
};

use super::{
    batcher::{self, BatchEmbeddingError},
    segmenter::{policy_identity, segment_chunks},
};

const MAX_ADAPTIVE_SEGMENTATION_ATTEMPTS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileStatus {
    Unchanged,
    Updated,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileOutcome {
    pub status: ReconcileStatus,
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub embedded: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorkspaceReindexOutcome {
    pub files_seen: usize,
    pub files_updated: usize,
    pub files_failed: usize,
    pub files_removed: usize,
    pub chunks_embedded: usize,
}

impl ReconcileOutcome {
    fn unchanged(chunk_count: usize) -> Self {
        Self {
            status: ReconcileStatus::Unchanged,
            added: 0,
            modified: 0,
            removed: 0,
            unchanged: chunk_count,
            embedded: 0,
        }
    }
}

pub struct IndexingService {
    storage: Arc<SqliteStorage>,
    embeddings: Arc<dyn EmbeddingProvider>,
    analyzers: Arc<AnalyzerRegistry>,
    permits: Semaphore,
    path_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    max_file_bytes: u64,
    include_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
    segment_overlap_tokens: usize,
    metrics: Arc<RuntimeMetrics>,
}

impl IndexingService {
    pub fn new(
        storage: Arc<SqliteStorage>,
        embeddings: Arc<dyn EmbeddingProvider>,
        analyzers: Arc<AnalyzerRegistry>,
        max_concurrent_jobs: usize,
        max_file_bytes: u64,
    ) -> Self {
        Self::with_metrics(
            storage,
            embeddings,
            analyzers,
            max_concurrent_jobs,
            max_file_bytes,
            Arc::new(RuntimeMetrics::default()),
        )
    }

    pub(crate) fn with_metrics(
        storage: Arc<SqliteStorage>,
        embeddings: Arc<dyn EmbeddingProvider>,
        analyzers: Arc<AnalyzerRegistry>,
        max_concurrent_jobs: usize,
        max_file_bytes: u64,
        metrics: Arc<RuntimeMetrics>,
    ) -> Self {
        let config = IndexingConfig {
            max_concurrent_embedding_jobs: max_concurrent_jobs,
            max_file_bytes,
            ..Default::default()
        };
        Self::with_config(storage, embeddings, analyzers, &config, metrics)
    }

    pub(crate) fn with_config(
        storage: Arc<SqliteStorage>,
        embeddings: Arc<dyn EmbeddingProvider>,
        analyzers: Arc<AnalyzerRegistry>,
        config: &IndexingConfig,
        metrics: Arc<RuntimeMetrics>,
    ) -> Self {
        Self {
            storage,
            embeddings,
            analyzers,
            permits: Semaphore::new(config.max_concurrent_embedding_jobs.max(1)),
            path_locks: Mutex::new(HashMap::new()),
            max_file_bytes: config.max_file_bytes,
            include_patterns: config.include_patterns.clone(),
            exclude_patterns: config.exclude_patterns.clone(),
            segment_overlap_tokens: config.embedding_segments.overlap_tokens,
            metrics,
        }
    }

    pub async fn reconcile_file(
        &self,
        workspace: &Workspace,
        absolute_path: &Path,
        relative_path: &Path,
    ) -> Result<ReconcileOutcome> {
        validate_reconcile_path(workspace, absolute_path, relative_path)?;
        let lock_key = format!("{}:{}", workspace.id, normalize_path(relative_path));
        let path_lock = {
            let mut locks = self.path_locks.lock().await;
            Arc::clone(
                locks
                    .entry(lock_key)
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _path_guard = path_lock.lock().await;
        self.metrics.job_started();
        let result = self
            .reconcile_file_inner(workspace, absolute_path, relative_path)
            .await;
        self.metrics.job_finished(result.is_err());
        if let Ok(outcome) = &result {
            self.metrics
                .record_chunks(outcome.added, outcome.modified, outcome.removed);
        }
        result
    }

    async fn reconcile_file_inner(
        &self,
        workspace: &Workspace,
        absolute_path: &Path,
        relative_path: &Path,
    ) -> Result<ReconcileOutcome> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| CortexError::Analysis("indexing service is shutting down".into()))?;
        let relative_path_string = normalize_path(relative_path);
        let metadata = match tokio::fs::metadata(absolute_path).await {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return self.remove_document(workspace, &relative_path_string).await;
            }
            Err(source) => {
                return Err(CortexError::Io {
                    path: absolute_path.to_path_buf(),
                    source,
                });
            }
        };
        if metadata.len() > self.max_file_bytes {
            return self.remove_document(workspace, &relative_path_string).await;
        }
        let bytes = match tokio::fs::read(absolute_path).await {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return self.remove_document(workspace, &relative_path_string).await;
            }
            Err(source) => {
                return Err(CortexError::Io {
                    path: absolute_path.to_path_buf(),
                    source,
                });
            }
        };
        if bytes[..bytes.len().min(8_192)].contains(&0) {
            return self.remove_document(workspace, &relative_path_string).await;
        }
        let source = match std::str::from_utf8(&bytes) {
            Ok(source) => source,
            Err(_) => return self.remove_document(workspace, &relative_path_string).await,
        };
        let content_hash = blake3::hash(&bytes).to_hex().to_string();
        let analyzer = self.analyzers.for_path(relative_path);
        let previous = self
            .storage
            .find_document(&workspace.id, &relative_path_string)
            .await?;
        let analyzer_compatible = previous.as_ref().is_some_and(|document| {
            document.language == analyzer.language_id()
                && document.analyzer_id == analyzer.analyzer_id()
                && document.analyzer_version == analyzer.analyzer_version()
        });
        let persisted_old_chunks = if let Some(document) = &previous {
            self.storage.list_chunks(&document.id).await?
        } else {
            Vec::new()
        };
        let configured_max_input_tokens = initial_segment_input_limit(self.embeddings.limits());
        let configured_segmentation_id = policy_identity(
            self.embeddings.as_ref(),
            self.segment_overlap_tokens,
            configured_max_input_tokens,
        );
        let initial_tree_compatible = analyzer_compatible
            && previous
                .as_ref()
                .is_some_and(|document| document.segmentation_id == configured_segmentation_id);
        let initial_embedding_space_compatible = match (initial_tree_compatible, previous.as_ref())
        {
            (true, Some(document)) => {
                self.storage
                    .document_embeddings_compatible(
                        &document.id,
                        self.embeddings.embedding_space_id(),
                        self.embeddings.dimension(),
                    )
                    .await?
            }
            _ => false,
        };

        if initial_tree_compatible
            && initial_embedding_space_compatible
            && previous
                .as_ref()
                .is_some_and(|document| document.content_hash == content_hash)
        {
            return Ok(ReconcileOutcome::unchanged(persisted_old_chunks.len()));
        }

        let analyzer_started = Instant::now();
        let analyzed = analyzer.analyze(relative_path, source);
        self.metrics.record_analyzer(analyzer_started.elapsed());
        let analyzed = analyzed?;
        let document_id = previous
            .as_ref()
            .map(|document| document.id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let now = Utc::now();
        let mut effective_max_input_tokens = configured_max_input_tokens;
        let mut adaptive_attempts = 0;
        let (
            segmentation_id,
            chunks,
            changed_indices,
            vectors,
            removed_ids,
            tree_compatible,
            added,
            modified,
            unchanged,
        ) = loop {
            let segmented = segment_chunks(
                &analyzed,
                self.embeddings.as_ref(),
                self.segment_overlap_tokens,
                effective_max_input_tokens,
            )?;
            self.metrics.record_capacity_splits(segmented.split_count);
            let tree_compatible = analyzer_compatible
                && previous
                    .as_ref()
                    .is_some_and(|document| document.segmentation_id == segmented.identity);
            let embedding_space_compatible = match (tree_compatible, previous.as_ref()) {
                (true, Some(document)) => {
                    self.storage
                        .document_embeddings_compatible(
                            &document.id,
                            self.embeddings.embedding_space_id(),
                            self.embeddings.dimension(),
                        )
                        .await?
                }
                _ => false,
            };
            let old_chunks = if tree_compatible {
                persisted_old_chunks.clone()
            } else {
                Vec::new()
            };
            let old_by_key: HashMap<String, StoredChunk> = old_chunks
                .iter()
                .cloned()
                .map(|chunk| (chunk.stable_key.clone(), chunk))
                .collect();
            let mut chunks = Vec::with_capacity(segmented.chunks.len());
            let mut changed_indices = Vec::new();
            let mut added = 0;
            let mut modified = 0;
            let mut unchanged = 0;

            for analyzed_chunk in segmented.chunks {
                let hash = blake3::hash(analyzed_chunk.content.as_bytes())
                    .to_hex()
                    .to_string();
                let old = old_by_key.get(&analyzed_chunk.stable_key);
                let is_unchanged = embedding_space_compatible
                    && old.is_some_and(|chunk| chunk.content_hash == hash);
                let id = old
                    .map(|chunk| chunk.id.clone())
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                let created_at = old.map_or(now, |chunk| chunk.created_at);
                let chunk = StoredChunk {
                    id,
                    document_id: document_id.clone(),
                    stable_key: analyzed_chunk.stable_key,
                    language: analyzed_chunk.language,
                    symbol: analyzed_chunk.symbol,
                    qualified_symbol: analyzed_chunk.qualified_symbol,
                    symbol_kind: analyzed_chunk.symbol_kind,
                    start_byte: checked_i64(analyzed_chunk.start_byte, "start_byte")?,
                    end_byte: checked_i64(analyzed_chunk.end_byte, "end_byte")?,
                    start_line: checked_i64(analyzed_chunk.start_line, "start_line")?,
                    end_line: checked_i64(analyzed_chunk.end_line, "end_line")?,
                    content: analyzed_chunk.content,
                    content_hash: hash,
                    metadata: analyzed_chunk.metadata,
                    created_at,
                    updated_at: now,
                };
                if is_unchanged {
                    unchanged += 1;
                } else {
                    if old.is_some() {
                        modified += 1;
                    } else {
                        added += 1;
                    }
                    changed_indices.push(chunks.len());
                }
                chunks.push(chunk);
            }

            let current_keys: HashSet<&str> = chunks
                .iter()
                .map(|chunk| chunk.stable_key.as_str())
                .collect();
            let removed_ids: Vec<String> = old_chunks
                .iter()
                .filter(|chunk| !current_keys.contains(chunk.stable_key.as_str()))
                .map(|chunk| chunk.id.clone())
                .collect();
            let texts: Vec<String> = changed_indices
                .iter()
                .map(|index| chunks[*index].content.clone())
                .collect();
            match batcher::embed_documents(&texts, self.embeddings.as_ref(), &self.metrics).await {
                Ok(vectors) => {
                    break (
                        segmented.identity,
                        chunks,
                        changed_indices,
                        vectors,
                        removed_ids,
                        tree_compatible,
                        added,
                        modified,
                        unchanged,
                    );
                }
                Err(BatchEmbeddingError::NeedsSmallerInput {
                    measured_tokens, ..
                }) if adaptive_attempts < MAX_ADAPTIVE_SEGMENTATION_ATTEMPTS => {
                    let next_limit = reduced_input_limit(
                        effective_max_input_tokens,
                        measured_tokens,
                        self.embeddings.limits().reserved_tokens,
                    )
                    .inspect_err(|_| {
                        self.metrics.record_terminal_capacity_failure();
                    })?;
                    effective_max_input_tokens = Some(next_limit);
                    adaptive_attempts += 1;
                }
                Err(BatchEmbeddingError::NeedsSmallerInput { .. }) => {
                    self.metrics.record_terminal_capacity_failure();
                    return Err(CortexError::Embedding(format!(
                        "embedding input remained over capacity after {MAX_ADAPTIVE_SEGMENTATION_ATTEMPTS} adaptive segmentation attempts"
                    )));
                }
                Err(BatchEmbeddingError::Fatal(error)) => return Err(error),
            }
        };
        validate_vectors(&vectors, changed_indices.len(), self.embeddings.dimension())?;
        let embedding_records: Vec<EmbeddingRecord> = changed_indices
            .iter()
            .zip(vectors)
            .map(|(index, vector)| {
                EmbeddingRecord::new(
                    &chunks[*index].id,
                    self.embeddings.embedding_space_id(),
                    vector,
                )
            })
            .collect();
        let document = Document {
            id: document_id,
            workspace_id: workspace.id.clone(),
            relative_path: relative_path_string,
            language: analyzer.language_id().into(),
            analyzer_id: analyzer.analyzer_id().into(),
            analyzer_version: analyzer.analyzer_version(),
            segmentation_id,
            content_hash,
            size_bytes: checked_i64(metadata.len(), "size_bytes")?,
            modified_at_ns: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .and_then(|duration| i64::try_from(duration.as_nanos()).ok()),
            indexed_at: now,
        };
        let final_bytes =
            tokio::fs::read(absolute_path)
                .await
                .map_err(|source| CortexError::Io {
                    path: absolute_path.to_path_buf(),
                    source,
                })?;
        if blake3::hash(&final_bytes).to_hex().as_str() != document.content_hash {
            return Err(CortexError::Analysis(format!(
                "source changed while indexing {}; retry reconciliation",
                absolute_path.display()
            )));
        }
        self.storage
            .apply_document_reconciliation(
                &document,
                &chunks,
                &embedding_records,
                &removed_ids,
                previous.is_some() && !tree_compatible,
            )
            .await?;

        Ok(ReconcileOutcome {
            status: ReconcileStatus::Updated,
            added,
            modified,
            removed: if tree_compatible {
                removed_ids.len()
            } else {
                persisted_old_chunks.len()
            },
            unchanged,
            embedded: embedding_records.len(),
        })
    }

    pub async fn reindex_workspace(
        &self,
        workspace: &Workspace,
    ) -> Result<WorkspaceReindexOutcome> {
        let scanner = WorkspaceScanner::with_patterns(
            Arc::clone(&self.analyzers),
            self.max_file_bytes,
            self.include_patterns.clone(),
            self.exclude_patterns.clone(),
        );
        let root = Path::new(&workspace.root_path);
        let discovered = scanner.scan(root)?;
        let mut outcome = WorkspaceReindexOutcome {
            files_seen: discovered.files.len(),
            ..Default::default()
        };
        for file in discovered.files {
            let result = match self
                .reconcile_file(workspace, &file.absolute_path, &file.relative_path)
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    outcome.files_failed += 1;
                    tracing::warn!(
                        path = %file.relative_path.display(),
                        error = %error,
                        "file reconciliation failed"
                    );
                    continue;
                }
            };
            if result.status == ReconcileStatus::Updated {
                outcome.files_updated += 1;
            }
            outcome.chunks_embedded += result.embedded;
        }
        let current_scan = scanner.scan(root)?;
        let current_paths: HashSet<String> = current_scan
            .files
            .into_iter()
            .map(|file| normalize_path(&file.relative_path))
            .collect();
        for document in self.storage.list_documents(&workspace.id).await? {
            if !current_paths.contains(&document.relative_path)
                && !current_scan
                    .failed_relative_paths
                    .contains(&document.relative_path)
                && self
                    .storage
                    .delete_document(&workspace.id, &document.relative_path)
                    .await?
            {
                outcome.files_removed += 1;
            }
        }
        Ok(outcome)
    }

    async fn remove_document(
        &self,
        workspace: &Workspace,
        relative_path: &str,
    ) -> Result<ReconcileOutcome> {
        let removed = self
            .storage
            .delete_document(&workspace.id, relative_path)
            .await?;
        Ok(ReconcileOutcome {
            status: if removed {
                ReconcileStatus::Removed
            } else {
                ReconcileStatus::Unchanged
            },
            added: 0,
            modified: 0,
            removed: usize::from(removed),
            unchanged: 0,
            embedded: 0,
        })
    }

    pub(crate) fn record_filesystem_events(&self, raw: usize, coalesced: usize) {
        self.metrics.record_filesystem_events(raw, coalesced);
    }
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn checked_i64<T>(value: T, field: &str) -> Result<i64>
where
    T: TryInto<i64>,
{
    value
        .try_into()
        .map_err(|_| CortexError::Analysis(format!("{field} exceeds SQLite integer range")))
}

fn reduced_input_limit(
    current_max_input_tokens: Option<usize>,
    measured_tokens: usize,
    reserved_tokens: usize,
) -> Result<usize> {
    let current_usable = current_max_input_tokens
        .map(|limit| limit.saturating_sub(reserved_tokens))
        .unwrap_or(measured_tokens);
    let next_usable = current_usable
        .saturating_sub(1)
        .min(measured_tokens.saturating_sub(1))
        .min(current_usable / 2);
    if next_usable == 0 {
        return Err(CortexError::Embedding(
            "embedding provider rejected the smallest recoverable input".into(),
        ));
    }
    let next = reserved_tokens.saturating_add(next_usable);
    if current_max_input_tokens.is_some_and(|current| next >= current) {
        return Err(CortexError::Embedding(
            "adaptive embedding ceiling could not be reduced further".into(),
        ));
    }
    Ok(next)
}

fn initial_segment_input_limit(limits: crate::embedding::EmbeddingLimits) -> Option<usize> {
    let batch_as_input_limit = limits
        .max_batch_tokens
        .map(|limit| limit.saturating_add(limits.reserved_tokens));
    match (limits.max_input_tokens, batch_as_input_limit) {
        (Some(input), Some(batch)) => Some(input.min(batch)),
        (Some(input), None) => Some(input),
        (None, Some(batch)) => Some(batch),
        (None, None) => None,
    }
}

fn validate_vectors(
    vectors: &[Vec<f32>],
    expected: usize,
    declared_dimension: Option<usize>,
) -> Result<()> {
    if vectors.len() != expected {
        return Err(CortexError::Embedding(format!(
            "provider returned {} vectors for {expected} inputs",
            vectors.len()
        )));
    }
    let mut dimension = None;
    for vector in vectors {
        if vector.is_empty() {
            return Err(CortexError::Embedding(
                "provider returned an empty vector".into(),
            ));
        }
        if declared_dimension.is_some_and(|expected| vector.len() != expected) {
            return Err(CortexError::Embedding(format!(
                "provider declared dimension {:?} but returned {}",
                declared_dimension,
                vector.len()
            )));
        }
        if vector.iter().any(|value| !value.is_finite()) {
            return Err(CortexError::Embedding(
                "provider returned a non-finite vector".into(),
            ));
        }
        match dimension {
            Some(value) if value != vector.len() => {
                return Err(CortexError::Embedding(
                    "provider returned mixed dimensions".into(),
                ));
            }
            None => dimension = Some(vector.len()),
            _ => {}
        }
    }
    Ok(())
}

fn validate_reconcile_path(
    workspace: &Workspace,
    absolute_path: &Path,
    relative_path: &Path,
) -> Result<()> {
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(CortexError::Configuration(format!(
            "relative path escapes workspace: {}",
            relative_path.display()
        )));
    }
    let root = PathIdentity::existing_directory(Path::new(&workspace.root_path))?;
    let candidate = PathIdentity::from_path(absolute_path)?;
    if !root.contains(&candidate) {
        return Err(CortexError::Configuration(format!(
            "path is outside workspace: {}",
            absolute_path.display()
        )));
    }
    let expected = PathIdentity::from_path(&root.display_path().join(relative_path))?;
    if candidate.comparison_key() != expected.comparison_key() {
        return Err(CortexError::Configuration(format!(
            "path does not match declared workspace-relative path: {}",
            absolute_path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use async_trait::async_trait;
    use tempfile::tempdir;
    use tokio::sync::Notify;

    use super::*;
    use crate::{
        config::{GenericChunkConfig, IndexingConfig, LanguageConfig},
        domain::{AnalyzedChunk, AnalyzerCapabilities},
        embedding::provider::MockEmbeddingProvider,
        embedding::{EmbeddingFailure, EmbeddingFailureKind, EmbeddingLimits},
        parsing::LanguageAnalyzer,
        retrieval::RetrievalService,
    };

    struct FailingProvider;

    struct SelectiveProvider;

    struct FailingRustAnalyzer;

    struct IncorrectDimensionProvider;

    struct AdaptiveStrictProvider {
        calls: AtomicUsize,
    }

    struct LimitedProvider {
        max_input_tokens: usize,
        embedded: AtomicUsize,
    }

    struct StrictCapacityProvider {
        calls: AtomicUsize,
        inputs: AtomicUsize,
        rejected: AtomicUsize,
    }

    struct MalformedResponseProvider;

    struct AlwaysTooLargeProvider {
        calls: AtomicUsize,
    }

    struct BlockingProvider {
        calls: AtomicUsize,
        started: Notify,
        release: Notify,
    }

    #[test]
    fn batch_capacity_can_tighten_the_initial_segment_ceiling() {
        let limits = EmbeddingLimits {
            max_input_tokens: Some(2_048),
            max_batch_tokens: Some(512),
            max_batch_items: 8,
            reserved_tokens: 32,
        };
        assert_eq!(initial_segment_input_limit(limits), Some(544));
    }

    #[async_trait]
    impl EmbeddingProvider for FailingProvider {
        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Err(CortexError::Embedding("offline".into()))
        }

        fn model_name(&self) -> &str {
            "offline-model"
        }
    }

    #[async_trait]
    impl EmbeddingProvider for SelectiveProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            if texts.iter().any(|text| text.contains("FAIL_EMBEDDING")) {
                return Err(CortexError::Embedding("selected input failed".into()));
            }
            Ok(texts.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
        }

        fn model_name(&self) -> &str {
            "selective"
        }

        fn dimension(&self) -> Option<usize> {
            Some(4)
        }
    }

    #[async_trait]
    impl EmbeddingProvider for IncorrectDimensionProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0, 0.0]).collect())
        }

        fn model_name(&self) -> &str {
            "incorrect-dimension"
        }

        fn dimension(&self) -> Option<usize> {
            Some(4)
        }
    }

    #[async_trait]
    impl EmbeddingProvider for AdaptiveStrictProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if texts.iter().any(|text| text.len() > 24) {
                return Err(EmbeddingFailure::new(
                    EmbeddingFailureKind::InputTooLarge,
                    "strict runtime input limit",
                )
                .into());
            }
            if texts.iter().map(String::len).sum::<usize>() > 40 {
                return Err(EmbeddingFailure::new(
                    EmbeddingFailureKind::BatchTooLarge,
                    "strict runtime batch limit",
                )
                .into());
            }
            Ok(texts.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
        }

        fn model_name(&self) -> &str {
            "adaptive-strict"
        }

        fn dimension(&self) -> Option<usize> {
            Some(4)
        }

        fn limits(&self) -> EmbeddingLimits {
            EmbeddingLimits {
                max_input_tokens: Some(64),
                max_batch_tokens: Some(96),
                max_batch_items: 4,
                reserved_tokens: 0,
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for LimitedProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.embedded.fetch_add(texts.len(), Ordering::Relaxed);
            Ok(texts.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
        }

        fn model_name(&self) -> &str {
            "limited-space"
        }

        fn dimension(&self) -> Option<usize> {
            Some(4)
        }

        fn limits(&self) -> EmbeddingLimits {
            EmbeddingLimits {
                max_input_tokens: Some(self.max_input_tokens),
                max_batch_tokens: Some(self.max_input_tokens * 2),
                max_batch_items: 4,
                reserved_tokens: 0,
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for StrictCapacityProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.inputs.fetch_add(texts.len(), Ordering::Relaxed);
            let limits = self.limits();
            let prepared: Vec<String> = texts
                .iter()
                .map(|text| self.prepare_document_input(text))
                .collect();
            let input_budget = limits.input_budget().unwrap();
            if prepared.iter().any(|text| text.len() > input_budget)
                || prepared.iter().map(String::len).sum::<usize>()
                    > limits.max_batch_tokens.unwrap()
                || prepared.len() > limits.max_batch_items
            {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                return Err(EmbeddingFailure::new(
                    EmbeddingFailureKind::InputTooLarge,
                    "strict capacity provider rejected an invalid request",
                )
                .into());
            }
            Ok(prepared
                .iter()
                .map(|text| {
                    let sum = text.bytes().map(f32::from).sum::<f32>();
                    vec![sum, text.len() as f32, 1.0, 0.0]
                })
                .collect())
        }

        fn model_name(&self) -> &str {
            "strict-2048-4096"
        }

        fn dimension(&self) -> Option<usize> {
            Some(4)
        }

        fn prepare_document_input(&self, text: &str) -> String {
            format!("search_document: {text}")
        }

        fn limits(&self) -> EmbeddingLimits {
            EmbeddingLimits {
                max_input_tokens: Some(2_048),
                max_batch_tokens: Some(4_096),
                max_batch_items: 8,
                reserved_tokens: 32,
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for MalformedResponseProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(vec![
                vec![1.0, 0.0, 0.0, 0.0];
                texts.len().saturating_sub(1)
            ])
        }

        fn model_name(&self) -> &str {
            "healthy"
        }

        fn dimension(&self) -> Option<usize> {
            Some(4)
        }
    }

    #[async_trait]
    impl EmbeddingProvider for AlwaysTooLargeProvider {
        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Err(EmbeddingFailure::new(
                EmbeddingFailureKind::InputTooLarge,
                "provider rejects every input",
            )
            .into())
        }

        fn model_name(&self) -> &str {
            "healthy"
        }

        fn dimension(&self) -> Option<usize> {
            Some(4)
        }

        fn limits(&self) -> EmbeddingLimits {
            EmbeddingLimits {
                max_input_tokens: Some(8),
                max_batch_tokens: Some(8),
                max_batch_items: 1,
                reserved_tokens: 0,
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for BlockingProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(texts.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
        }

        fn model_name(&self) -> &str {
            "blocking"
        }

        fn dimension(&self) -> Option<usize> {
            Some(4)
        }
    }

    impl LanguageAnalyzer for FailingRustAnalyzer {
        fn language_id(&self) -> &'static str {
            "rust"
        }

        fn analyzer_id(&self) -> &'static str {
            "tree-sitter-rust"
        }

        fn analyzer_version(&self) -> String {
            env!("CARGO_PKG_VERSION").into()
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["rs"]
        }

        fn capabilities(&self) -> AnalyzerCapabilities {
            AnalyzerCapabilities::default()
        }

        fn analyze(&self, _path: &Path, _source: &str) -> Result<Vec<AnalyzedChunk>> {
            Err(CortexError::Analysis("parser unavailable".into()))
        }
    }

    #[tokio::test]
    async fn embeds_only_changed_logical_chunks_and_survives_restart() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("cortex.db");
        let rust_path = directory.path().join("lib.rs");
        let python_path = directory.path().join("main.py");
        let typescript_path = directory.path().join("app.ts");
        let go_path = directory.path().join("service.go");
        fs::write(
            &rust_path,
            "fn alpha() -> i32 { 1 }\nfn beta() -> i32 { 2 }\n",
        )
        .unwrap();
        fs::write(&python_path, "class Stable:\n    pass\n").unwrap();
        fs::write(
            &typescript_path,
            "function stable() { return 1; }\nfunction changed() { return 2; }\n",
        )
        .unwrap();
        fs::write(
            &go_path,
            "type Service struct {}\nfunc (service *Service) Stable() int { return 1 }\nfunc (service *Service) Changed() int { return 2 }\n",
        )
        .unwrap();

        let workspace = Workspace::new(directory.path().to_string_lossy(), "mixed");
        let storage = Arc::new(SqliteStorage::open(&database).await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let provider = Arc::new(MockEmbeddingProvider::new("test-space", 8));
        let indexer = IndexingService::new(
            Arc::clone(&storage),
            provider.clone(),
            Arc::clone(&analyzers),
            2,
            1_048_576,
        );

        indexer
            .reconcile_file(&workspace, &rust_path, Path::new("lib.rs"))
            .await
            .unwrap();
        indexer
            .reconcile_file(&workspace, &python_path, Path::new("main.py"))
            .await
            .unwrap();
        indexer
            .reconcile_file(&workspace, &typescript_path, Path::new("app.ts"))
            .await
            .unwrap();
        indexer
            .reconcile_file(&workspace, &go_path, Path::new("service.go"))
            .await
            .unwrap();
        let initial_embeds = provider.embedded_text_count();

        fs::write(
            &rust_path,
            "fn alpha() -> i32 { 1 }\nfn beta() -> i32 { 20 }\n",
        )
        .unwrap();
        fs::write(
            &typescript_path,
            "function stable() { return 1; }\nfunction changed() { return 20; }\n",
        )
        .unwrap();
        fs::write(
            &go_path,
            "type Service struct {}\nfunc (service *Service) Stable() int { return 1 }\nfunc (service *Service) Changed() int { return 20 }\n",
        )
        .unwrap();
        let rust = indexer
            .reconcile_file(&workspace, &rust_path, Path::new("lib.rs"))
            .await
            .unwrap();
        let python = indexer
            .reconcile_file(&workspace, &python_path, Path::new("main.py"))
            .await
            .unwrap();
        let typescript = indexer
            .reconcile_file(&workspace, &typescript_path, Path::new("app.ts"))
            .await
            .unwrap();
        let go = indexer
            .reconcile_file(&workspace, &go_path, Path::new("service.go"))
            .await
            .unwrap();
        assert_eq!(rust.embedded, 1);
        assert_eq!(typescript.embedded, 1);
        assert_eq!(go.embedded, 1);
        assert_eq!(python.status, ReconcileStatus::Unchanged);
        assert_eq!(provider.embedded_text_count(), initial_embeds + 3);

        drop(indexer);
        drop(storage);
        let restarted_storage = Arc::new(SqliteStorage::open(&database).await.unwrap());
        let restarted_provider = Arc::new(MockEmbeddingProvider::new("test-space", 8));
        let restarted = IndexingService::new(
            Arc::clone(&restarted_storage),
            restarted_provider.clone(),
            analyzers,
            1,
            1_048_576,
        );
        let outcome = restarted
            .reconcile_file(&workspace, &rust_path, Path::new("lib.rs"))
            .await
            .unwrap();
        assert_eq!(outcome.status, ReconcileStatus::Unchanged);
        assert_eq!(restarted_provider.embedded_text_count(), 0);
    }

    #[tokio::test]
    async fn adaptively_segments_and_repacks_without_dropping_source() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("large.md");
        let content = format!(
            "BEGIN_MARKER {} MIDDLE_MARKER {} END_MARKER",
            "alpha ".repeat(24),
            "beta ".repeat(24)
        );
        fs::write(&file, &content).unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "capacity");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let provider = Arc::new(AdaptiveStrictProvider {
            calls: AtomicUsize::new(0),
        });
        let mut config = IndexingConfig::default();
        config.embedding_segments.overlap_tokens = 4;
        let indexer = IndexingService::with_config(
            Arc::clone(&storage),
            Arc::clone(&provider) as Arc<dyn EmbeddingProvider>,
            Arc::new(AnalyzerRegistry::configured(
                &LanguageConfig::default(),
                &GenericChunkConfig::default(),
            )),
            &config,
            Arc::new(RuntimeMetrics::default()),
        );

        let outcome = indexer
            .reconcile_file(&workspace, &file, Path::new("large.md"))
            .await
            .unwrap();
        assert!(outcome.embedded > 3);
        assert!(provider.calls.load(Ordering::Relaxed) > 3);
        let document = storage
            .find_document(&workspace.id, "large.md")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            document.segmentation_id,
            policy_identity(provider.as_ref(), 4, Some(64))
        );
        let chunks = storage.list_chunks(&document.id).await.unwrap();
        assert!(chunks.iter().all(|chunk| chunk.content.len() <= 24));
        let mut covered = vec![false; content.len()];
        for chunk in &chunks {
            let start = usize::try_from(chunk.start_byte).unwrap();
            let end = usize::try_from(chunk.end_byte).unwrap();
            covered[start..end].fill(true);
        }
        assert!(covered.into_iter().all(|is_covered| is_covered));
        assert!(chunks.iter().all(|chunk| {
            chunk.metadata.get("parent_logical_stable_key").is_some()
                && chunk.metadata.get("segment_part_count").is_some()
        }));
    }

    #[tokio::test]
    async fn strict_2048_4096_provider_indexes_multilingual_oversized_sources() {
        let directory = tempdir().unwrap();
        let fixtures = [
            (
                "huge.rs",
                format!(
                    "fn bounded_rust() -> bool {{ true }}\n\nfn huge_rust() {{\n// RUST_BEGIN\n{}\n// RUST_MIDDLE\n{}\n// RUST_END\n}}\n",
                    "let value = \"rust payload\";\n".repeat(120),
                    "let tail = \"rust tail\";\n".repeat(120),
                ),
                "RUST_MIDDLE",
            ),
            (
                "huge.py",
                format!(
                    "class HugePython:\n    def build(self):\n        # PYTHON_BEGIN\n{}        # PYTHON_MIDDLE\n{}        # PYTHON_END\n        return True\n",
                    "        value = 'python payload'\n".repeat(120),
                    "        tail = 'python tail'\n".repeat(120),
                ),
                "PYTHON_MIDDLE",
            ),
            (
                "huge.ts",
                format!(
                    "export function hugeTypeScript() {{\n// TYPESCRIPT_BEGIN\n{}\n// TYPESCRIPT_MIDDLE\n{}\n// TYPESCRIPT_END\n}}\n",
                    "const value = 'typescript payload';\n".repeat(120),
                    "const tail = 'typescript tail';\n".repeat(120),
                ),
                "TYPESCRIPT_MIDDLE",
            ),
            (
                "huge.md",
                format!(
                    "# Markdown Beginning\nMARKDOWN_BEGIN\n\n{}\n## Markdown Middle\nMARKDOWN_MIDDLE\n\n{}\nMARKDOWN_END\n",
                    "markdown payload words for capacity testing\n".repeat(100),
                    "markdown tail words for capacity testing\n".repeat(100),
                ),
                "MARKDOWN_MIDDLE",
            ),
        ];
        for (name, source, _) in &fixtures {
            fs::write(directory.path().join(name), source).unwrap();
        }
        let workspace = Workspace::new(directory.path().to_string_lossy(), "strict-capacity");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let provider = Arc::new(StrictCapacityProvider {
            calls: AtomicUsize::new(0),
            inputs: AtomicUsize::new(0),
            rejected: AtomicUsize::new(0),
        });
        let indexer = IndexingService::new(
            Arc::clone(&storage),
            Arc::clone(&provider) as Arc<dyn EmbeddingProvider>,
            Arc::new(AnalyzerRegistry::configured(
                &LanguageConfig::default(),
                &GenericChunkConfig::default(),
            )),
            1,
            1_048_576,
        );

        let first = indexer.reindex_workspace(&workspace).await.unwrap();
        assert_eq!(first.files_seen, fixtures.len());
        assert_eq!(first.files_failed, 0);
        assert!(first.chunks_embedded > fixtures.len());
        assert_eq!(provider.rejected.load(Ordering::Relaxed), 0);
        let input_count = provider.inputs.load(Ordering::Relaxed);
        assert!(input_count > fixtures.len());
        let rust_document = storage
            .find_document(&workspace.id, "huge.rs")
            .await
            .unwrap()
            .unwrap();
        assert!(
            storage
                .list_chunks(&rust_document.id)
                .await
                .unwrap()
                .iter()
                .any(|chunk| chunk.stable_key.ends_with("function:bounded_rust"))
        );

        let retrieval = RetrievalService::new(
            Arc::clone(&storage),
            Arc::clone(&provider) as Arc<dyn EmbeddingProvider>,
            0.7,
            0.3,
        )
        .unwrap();
        for (name, _, marker) in &fixtures {
            let results = retrieval
                .lexical_search(&workspace.id, marker, 10)
                .await
                .unwrap();
            assert!(results.iter().any(|result| result.path == *name));
        }
        for marker in ["RUST_BEGIN", "RUST_MIDDLE", "RUST_END"] {
            let results = retrieval
                .lexical_search(&workspace.id, marker, 10)
                .await
                .unwrap();
            assert!(results.iter().any(|result| result.path == "huge.rs"));
        }

        let second = indexer.reindex_workspace(&workspace).await.unwrap();
        assert_eq!(second.files_failed, 0);
        assert_eq!(second.chunks_embedded, 0);
        assert_eq!(provider.inputs.load(Ordering::Relaxed), input_count);
    }

    #[tokio::test]
    async fn segmentation_policy_change_rebuilds_then_reuses_the_new_tree() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("policy.md");
        fs::write(&file, "small bounded source").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "policy");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let first_provider = Arc::new(LimitedProvider {
            max_input_tokens: 64,
            embedded: AtomicUsize::new(0),
        });
        let first = IndexingService::new(
            Arc::clone(&storage),
            first_provider,
            Arc::clone(&analyzers),
            1,
            1_024,
        );
        first
            .reconcile_file(&workspace, &file, Path::new("policy.md"))
            .await
            .unwrap();
        let document = storage
            .find_document(&workspace.id, "policy.md")
            .await
            .unwrap()
            .unwrap();
        let first_chunk_id = storage.list_chunks(&document.id).await.unwrap()[0]
            .id
            .clone();

        let changed_provider = Arc::new(LimitedProvider {
            max_input_tokens: 32,
            embedded: AtomicUsize::new(0),
        });
        let changed = IndexingService::new(
            Arc::clone(&storage),
            Arc::clone(&changed_provider) as Arc<dyn EmbeddingProvider>,
            Arc::clone(&analyzers),
            1,
            1_024,
        );
        let rebuilt = changed
            .reconcile_file(&workspace, &file, Path::new("policy.md"))
            .await
            .unwrap();
        assert_eq!(rebuilt.embedded, 1);
        assert_ne!(
            storage.list_chunks(&document.id).await.unwrap()[0].id,
            first_chunk_id
        );
        assert_eq!(changed_provider.embedded.load(Ordering::Relaxed), 1);

        let unchanged = changed
            .reconcile_file(&workspace, &file, Path::new("policy.md"))
            .await
            .unwrap();
        assert_eq!(unchanged.status, ReconcileStatus::Unchanged);
        assert_eq!(changed_provider.embedded.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn deletion_removes_document_and_search_rows() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("gone.rs");
        fs::write(&file, "fn gone() {}\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "delete");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let indexer = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("test", 4)),
            Arc::new(AnalyzerRegistry::configured(
                &LanguageConfig::default(),
                &GenericChunkConfig::default(),
            )),
            1,
            1_024,
        );
        indexer
            .reconcile_file(&workspace, &file, Path::new("gone.rs"))
            .await
            .unwrap();
        fs::remove_file(&file).unwrap();
        let outcome = indexer
            .reconcile_file(&workspace, &file, Path::new("gone.rs"))
            .await
            .unwrap();
        assert_eq!(outcome.status, ReconcileStatus::Removed);
        assert!(
            storage
                .find_document(&workspace.id, "gone.rs")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn declared_embedding_dimension_is_enforced_before_persistence() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("bad.py");
        fs::write(&file, "def value():\n    return 1\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "bad-dimension");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let indexer = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(IncorrectDimensionProvider),
            Arc::new(AnalyzerRegistry::configured(
                &LanguageConfig::default(),
                &GenericChunkConfig::default(),
            )),
            1,
            1_024,
        );

        assert!(
            indexer
                .reconcile_file(&workspace, &file, Path::new("bad.py"))
                .await
                .is_err()
        );
        assert!(
            storage
                .find_document(&workspace.id, "bad.py")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn same_path_reconciliation_rejects_stale_source_before_commit() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("current.rs");
        fs::write(&file, "fn current() -> i32 { 1 }\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "race");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let provider = Arc::new(BlockingProvider {
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
        });
        let indexer = Arc::new(IndexingService::new(
            Arc::clone(&storage),
            Arc::clone(&provider) as Arc<dyn EmbeddingProvider>,
            Arc::new(AnalyzerRegistry::configured(
                &LanguageConfig::default(),
                &GenericChunkConfig::default(),
            )),
            2,
            1_024,
        ));
        let first_indexer = Arc::clone(&indexer);
        let first_workspace = workspace.clone();
        let first_file = file.clone();
        let first = tokio::spawn(async move {
            first_indexer
                .reconcile_file(&first_workspace, &first_file, Path::new("current.rs"))
                .await
        });
        provider.started.notified().await;
        fs::write(&file, "fn current() -> i32 { 2 }\n").unwrap();
        let second_indexer = Arc::clone(&indexer);
        let second_workspace = workspace.clone();
        let second_file = file.clone();
        let second = tokio::spawn(async move {
            second_indexer
                .reconcile_file(&second_workspace, &second_file, Path::new("current.rs"))
                .await
        });
        provider.release.notify_one();

        assert!(first.await.unwrap().is_err());
        assert_eq!(
            second.await.unwrap().unwrap().status,
            ReconcileStatus::Updated
        );
        let document = storage
            .find_document(&workspace.id, "current.rs")
            .await
            .unwrap()
            .unwrap();
        assert!(
            storage.list_chunks(&document.id).await.unwrap()[0]
                .content
                .contains("{ 2 }")
        );
    }

    #[tokio::test]
    async fn embedding_failure_preserves_last_committed_document() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("safe.rs");
        fs::write(&file, "fn value() -> i32 { 1 }\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "safe");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let healthy = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("healthy", 4)),
            Arc::clone(&analyzers),
            1,
            1_024,
        );
        healthy
            .reconcile_file(&workspace, &file, Path::new("safe.rs"))
            .await
            .unwrap();
        let before = storage
            .find_document(&workspace.id, "safe.rs")
            .await
            .unwrap()
            .unwrap();

        fs::write(&file, "fn value() -> i32 { 2 }\n").unwrap();
        let offline = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(FailingProvider),
            analyzers,
            1,
            1_024,
        );
        assert!(
            offline
                .reconcile_file(&workspace, &file, Path::new("safe.rs"))
                .await
                .is_err()
        );
        let after = storage
            .find_document(&workspace.id, "safe.rs")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.content_hash, after.content_hash);
        assert!(
            storage.list_chunks(&after.id).await.unwrap()[0]
                .content
                .contains("{ 1 }")
        );
    }

    #[tokio::test]
    async fn malformed_replacement_response_preserves_last_committed_document() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("safe.rs");
        fs::write(&file, "fn value() -> i32 { 1 }\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "malformed");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let healthy = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("healthy", 4)),
            Arc::clone(&analyzers),
            1,
            1_024,
        );
        healthy
            .reconcile_file(&workspace, &file, Path::new("safe.rs"))
            .await
            .unwrap();
        let before = storage
            .find_document(&workspace.id, "safe.rs")
            .await
            .unwrap()
            .unwrap();

        fs::write(&file, "fn value() -> i32 { 2 }\n").unwrap();
        let malformed = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MalformedResponseProvider),
            analyzers,
            1,
            1_024,
        );
        assert!(
            malformed
                .reconcile_file(&workspace, &file, Path::new("safe.rs"))
                .await
                .is_err()
        );

        let after = storage
            .find_document(&workspace.id, "safe.rs")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.content_hash, after.content_hash);
        assert!(
            storage.list_chunks(&after.id).await.unwrap()[0]
                .content
                .contains("{ 1 }")
        );
    }

    #[tokio::test]
    async fn terminal_capacity_failure_is_bounded_and_preserves_last_document() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("safe.rs");
        fs::write(&file, "fn value() -> i32 { 1 }\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "bounded-failure");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let healthy = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("healthy", 4)),
            Arc::clone(&analyzers),
            1,
            1_024,
        );
        healthy
            .reconcile_file(&workspace, &file, Path::new("safe.rs"))
            .await
            .unwrap();
        let before = storage
            .find_document(&workspace.id, "safe.rs")
            .await
            .unwrap()
            .unwrap();

        fs::write(&file, "fn value() -> i32 { 2 }\n").unwrap();
        let provider = Arc::new(AlwaysTooLargeProvider {
            calls: AtomicUsize::new(0),
        });
        let rejecting = IndexingService::new(
            Arc::clone(&storage),
            Arc::clone(&provider) as Arc<dyn EmbeddingProvider>,
            analyzers,
            1,
            1_024,
        );
        assert!(
            rejecting
                .reconcile_file(&workspace, &file, Path::new("safe.rs"))
                .await
                .is_err()
        );
        assert!(provider.calls.load(Ordering::Relaxed) <= 16);
        let after = storage
            .find_document(&workspace.id, "safe.rs")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.content_hash, after.content_hash);
    }

    #[tokio::test]
    async fn embedding_model_change_rebuilds_the_document_space() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("space.py");
        fs::write(&file, "def stable():\n    return 1\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "space");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let first = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("model-a", 4)),
            Arc::clone(&analyzers),
            1,
            1_024,
        );
        first
            .reconcile_file(&workspace, &file, Path::new("space.py"))
            .await
            .unwrap();

        let second_provider = Arc::new(MockEmbeddingProvider::new("model-b", 6));
        let second = IndexingService::new(
            Arc::clone(&storage),
            second_provider.clone(),
            analyzers,
            1,
            1_024,
        );
        let outcome = second
            .reconcile_file(&workspace, &file, Path::new("space.py"))
            .await
            .unwrap();
        assert_eq!(outcome.embedded, 1);
        let document = storage
            .find_document(&workspace.id, "space.py")
            .await
            .unwrap()
            .unwrap();
        assert!(
            storage
                .document_embeddings_compatible(&document.id, "model-b", Some(6))
                .await
                .unwrap()
        );
        assert_eq!(second_provider.embedded_text_count(), 1);
    }

    #[tokio::test]
    async fn embedding_dimension_change_rebuilds_same_named_space() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("space.py");
        fs::write(&file, "def stable():\n    return 1\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "dimension");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let first = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("same-model", 4)),
            Arc::clone(&analyzers),
            1,
            1_024,
        );
        first
            .reconcile_file(&workspace, &file, Path::new("space.py"))
            .await
            .unwrap();

        let changed = Arc::new(MockEmbeddingProvider::new("same-model", 7));
        let second =
            IndexingService::new(Arc::clone(&storage), changed.clone(), analyzers, 1, 1_024);
        let outcome = second
            .reconcile_file(&workspace, &file, Path::new("space.py"))
            .await
            .unwrap();
        assert_eq!(outcome.embedded, 1);
        let document = storage
            .find_document(&workspace.id, "space.py")
            .await
            .unwrap()
            .unwrap();
        assert!(
            storage
                .document_embeddings_compatible(&document.id, "same-model", Some(7))
                .await
                .unwrap()
        );
        assert_eq!(changed.embedded_text_count(), 1);
    }

    #[tokio::test]
    async fn analyzer_failure_preserves_last_committed_document() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("safe.rs");
        fs::write(&file, "fn safe() -> i32 { 1 }\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "analyzer-failure");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let healthy_registry = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let healthy = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("stable", 4)),
            healthy_registry,
            1,
            1_024,
        );
        healthy
            .reconcile_file(&workspace, &file, Path::new("safe.rs"))
            .await
            .unwrap();
        let before = storage
            .find_document(&workspace.id, "safe.rs")
            .await
            .unwrap()
            .unwrap();

        fs::write(&file, "fn safe() -> i32 { 2 }\n").unwrap();
        let mut failing_registry = AnalyzerRegistry::default();
        failing_registry.register(Arc::new(FailingRustAnalyzer));
        let failing = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("stable", 4)),
            Arc::new(failing_registry),
            1,
            1_024,
        );
        assert!(
            failing
                .reconcile_file(&workspace, &file, Path::new("safe.rs"))
                .await
                .is_err()
        );
        let after = storage
            .find_document(&workspace.id, "safe.rs")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.content_hash, after.content_hash);
        assert!(
            storage.list_chunks(&after.id).await.unwrap()[0]
                .content
                .contains("{ 1 }")
        );
    }

    #[tokio::test]
    async fn workspace_reindex_continues_after_one_file_fails() {
        let directory = tempdir().unwrap();
        let failing_file = directory.path().join("a.rs");
        let healthy_file = directory.path().join("z.rs");
        fs::write(&failing_file, "fn first() -> i32 { 1 }\n").unwrap();
        fs::write(&healthy_file, "fn second() -> i32 { 1 }\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "partial");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let initial = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("selective", 4)),
            Arc::clone(&analyzers),
            1,
            1_024,
        );
        initial.reindex_workspace(&workspace).await.unwrap();
        let failing_before = storage
            .find_document(&workspace.id, "a.rs")
            .await
            .unwrap()
            .unwrap();

        fs::write(
            &failing_file,
            "fn first() -> &'static str { \"FAIL_EMBEDDING\" }\n",
        )
        .unwrap();
        fs::write(&healthy_file, "fn second() -> i32 { 2 }\n").unwrap();
        let partial = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(SelectiveProvider),
            analyzers,
            1,
            1_024,
        );
        let outcome = partial.reindex_workspace(&workspace).await.unwrap();
        assert_eq!(outcome.files_failed, 1);
        assert_eq!(outcome.files_updated, 1);
        let failing_after = storage
            .find_document(&workspace.id, "a.rs")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failing_before.content_hash, failing_after.content_hash);
        let healthy_after = storage
            .find_document(&workspace.id, "z.rs")
            .await
            .unwrap()
            .unwrap();
        assert!(
            storage.list_chunks(&healthy_after.id).await.unwrap()[0]
                .content
                .contains("{ 2 }")
        );
    }

    #[tokio::test]
    async fn database_lock_preserves_last_committed_document() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("locked.db");
        let file = directory.path().join("safe.rs");
        fs::write(&file, "fn safe() -> i32 { 1 }\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "locked");
        let storage = Arc::new(SqliteStorage::open(&database).await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let indexer = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("stable", 4)),
            analyzers,
            1,
            1_024,
        );
        indexer
            .reconcile_file(&workspace, &file, Path::new("safe.rs"))
            .await
            .unwrap();
        let before = storage
            .find_document(&workspace.id, "safe.rs")
            .await
            .unwrap()
            .unwrap();
        fs::write(&file, "fn safe() -> i32 { 2 }\n").unwrap();

        let mut lock = storage.pool().acquire().await.unwrap();
        sqlx::query("BEGIN EXCLUSIVE")
            .execute(&mut *lock)
            .await
            .unwrap();
        assert!(
            indexer
                .reconcile_file(&workspace, &file, Path::new("safe.rs"))
                .await
                .is_err()
        );
        sqlx::query("ROLLBACK").execute(&mut *lock).await.unwrap();

        let after = storage
            .find_document(&workspace.id, "safe.rs")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.content_hash, after.content_hash);
        assert!(
            storage.list_chunks(&after.id).await.unwrap()[0]
                .content
                .contains("{ 1 }")
        );
    }
}
