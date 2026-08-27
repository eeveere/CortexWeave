use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{
    CortexError, Result,
    embedding::{EmbeddingLimits, EmbeddingProvider, TokenCountAccuracy},
    storage::SqliteStorage,
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum WorkspaceResolutionKind {
    Id,
    Name,
    Path,
    Default,
    Singleton,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LatencySummary {
    pub samples: u64,
    pub total_ms: f64,
    pub average_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct InstrumentationSnapshot {
    pub documents: usize,
    pub chunks: usize,
    pub chunks_by_language: BTreeMap<String, usize>,
    pub analyzer_usage: BTreeMap<String, usize>,
    pub fallback_usage: usize,
    pub memories: usize,
    pub events: usize,
    pub pending_jobs: usize,
    pub failed_jobs: usize,
    pub embedding_calls: usize,
    pub embedding_batches: usize,
    pub embedding_inputs: usize,
    pub capacity_splits: usize,
    pub capacity_retries: usize,
    pub terminal_capacity_failures: usize,
    pub batch_repacks: usize,
    pub workspace_resolutions_by_id: usize,
    pub workspace_resolutions_by_name: usize,
    pub workspace_resolutions_by_path: usize,
    pub workspace_resolutions_by_default: usize,
    pub workspace_resolutions_by_singleton: usize,
    pub workspace_resolution_misses: usize,
    pub workspace_resolution_ambiguities: usize,
    pub embedding_latency: LatencySummary,
    pub search_latency: LatencySummary,
    pub analyzer_latency: LatencySummary,
    pub filesystem_events: usize,
    pub coalesced_events: usize,
    pub chunks_added: usize,
    pub chunks_modified: usize,
    pub chunks_deleted: usize,
    pub last_indexed_time: Option<DateTime<Utc>>,
    pub embedding_model: String,
    pub embedding_space: String,
    pub embedding_dimension: Option<usize>,
    pub embedding_limits: Option<EmbeddingLimits>,
    pub token_counter: String,
    pub token_counter_accuracy: TokenCountAccuracy,
    pub capacity_limit_source: String,
    pub runtime_metrics_scope: String,
}

#[derive(Default)]
pub(crate) struct RuntimeMetrics {
    pending_jobs: AtomicUsize,
    failed_jobs: AtomicUsize,
    embedding_calls: AtomicUsize,
    embedding_batches: AtomicUsize,
    embedding_inputs: AtomicUsize,
    capacity_splits: AtomicUsize,
    capacity_retries: AtomicUsize,
    terminal_capacity_failures: AtomicUsize,
    batch_repacks: AtomicUsize,
    workspace_resolutions_by_id: AtomicUsize,
    workspace_resolutions_by_name: AtomicUsize,
    workspace_resolutions_by_path: AtomicUsize,
    workspace_resolutions_by_default: AtomicUsize,
    workspace_resolutions_by_singleton: AtomicUsize,
    workspace_resolution_misses: AtomicUsize,
    workspace_resolution_ambiguities: AtomicUsize,
    embedding_latency: LatencyAccumulator,
    search_latency: LatencyAccumulator,
    analyzer_latency: LatencyAccumulator,
    filesystem_events: AtomicUsize,
    coalesced_events: AtomicUsize,
    chunks_added: AtomicUsize,
    chunks_modified: AtomicUsize,
    chunks_deleted: AtomicUsize,
}

impl RuntimeMetrics {
    pub(crate) fn job_started(&self) {
        self.pending_jobs.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn job_finished(&self, failed: bool) {
        self.pending_jobs.fetch_sub(1, Ordering::Relaxed);
        if failed {
            self.failed_jobs.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_embedding(
        &self,
        inputs: usize,
        batch_size: Option<usize>,
        elapsed: Duration,
    ) {
        self.embedding_calls.fetch_add(1, Ordering::Relaxed);
        self.embedding_inputs.fetch_add(inputs, Ordering::Relaxed);
        let batches = batch_size
            .filter(|size| *size > 0)
            .map_or(1, |size| inputs.div_ceil(size));
        self.embedding_batches.fetch_add(batches, Ordering::Relaxed);
        self.embedding_latency.record(elapsed);
    }

    pub(crate) fn record_capacity_splits(&self, count: usize) {
        self.capacity_splits.fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn record_capacity_retry(&self, repacked: bool) {
        self.capacity_retries.fetch_add(1, Ordering::Relaxed);
        if repacked {
            self.batch_repacks.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_terminal_capacity_failure(&self) {
        self.terminal_capacity_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_workspace_resolution(
        &self,
        kind: WorkspaceResolutionKind,
        result: &crate::Result<crate::domain::Workspace>,
    ) {
        match result {
            Ok(_) => {
                match kind {
                    WorkspaceResolutionKind::Id => self
                        .workspace_resolutions_by_id
                        .fetch_add(1, Ordering::Relaxed),
                    WorkspaceResolutionKind::Name => self
                        .workspace_resolutions_by_name
                        .fetch_add(1, Ordering::Relaxed),
                    WorkspaceResolutionKind::Path => self
                        .workspace_resolutions_by_path
                        .fetch_add(1, Ordering::Relaxed),
                    WorkspaceResolutionKind::Default => self
                        .workspace_resolutions_by_default
                        .fetch_add(1, Ordering::Relaxed),
                    WorkspaceResolutionKind::Singleton => self
                        .workspace_resolutions_by_singleton
                        .fetch_add(1, Ordering::Relaxed),
                };
            }
            Err(CortexError::NotFound(_)) => {
                self.workspace_resolution_misses
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(CortexError::Analysis(_)) => {
                self.workspace_resolution_ambiguities
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.workspace_resolution_misses
                    .fetch_add(1, Ordering::Relaxed);
            }
        };
    }

    pub(crate) fn record_search(&self, elapsed: Duration) {
        self.search_latency.record(elapsed);
    }

    pub(crate) fn record_analyzer(&self, elapsed: Duration) {
        self.analyzer_latency.record(elapsed);
    }

    pub(crate) fn record_filesystem_events(&self, raw: usize, coalesced: usize) {
        self.filesystem_events.fetch_add(raw, Ordering::Relaxed);
        self.coalesced_events
            .fetch_add(coalesced, Ordering::Relaxed);
    }

    pub(crate) fn record_chunks(&self, added: usize, modified: usize, deleted: usize) {
        self.chunks_added.fetch_add(added, Ordering::Relaxed);
        self.chunks_modified.fetch_add(modified, Ordering::Relaxed);
        self.chunks_deleted.fetch_add(deleted, Ordering::Relaxed);
    }

    pub(crate) fn apply_to(&self, snapshot: &mut InstrumentationSnapshot) {
        snapshot.pending_jobs = self.pending_jobs.load(Ordering::Relaxed);
        snapshot.failed_jobs = self.failed_jobs.load(Ordering::Relaxed);
        snapshot.embedding_calls = self.embedding_calls.load(Ordering::Relaxed);
        snapshot.embedding_batches = self.embedding_batches.load(Ordering::Relaxed);
        snapshot.embedding_inputs = self.embedding_inputs.load(Ordering::Relaxed);
        snapshot.capacity_splits = self.capacity_splits.load(Ordering::Relaxed);
        snapshot.capacity_retries = self.capacity_retries.load(Ordering::Relaxed);
        snapshot.terminal_capacity_failures =
            self.terminal_capacity_failures.load(Ordering::Relaxed);
        snapshot.batch_repacks = self.batch_repacks.load(Ordering::Relaxed);
        snapshot.workspace_resolutions_by_id =
            self.workspace_resolutions_by_id.load(Ordering::Relaxed);
        snapshot.workspace_resolutions_by_name =
            self.workspace_resolutions_by_name.load(Ordering::Relaxed);
        snapshot.workspace_resolutions_by_path =
            self.workspace_resolutions_by_path.load(Ordering::Relaxed);
        snapshot.workspace_resolutions_by_default = self
            .workspace_resolutions_by_default
            .load(Ordering::Relaxed);
        snapshot.workspace_resolutions_by_singleton = self
            .workspace_resolutions_by_singleton
            .load(Ordering::Relaxed);
        snapshot.workspace_resolution_misses =
            self.workspace_resolution_misses.load(Ordering::Relaxed);
        snapshot.workspace_resolution_ambiguities = self
            .workspace_resolution_ambiguities
            .load(Ordering::Relaxed);
        snapshot.embedding_latency = self.embedding_latency.snapshot();
        snapshot.search_latency = self.search_latency.snapshot();
        snapshot.analyzer_latency = self.analyzer_latency.snapshot();
        snapshot.filesystem_events = self.filesystem_events.load(Ordering::Relaxed);
        snapshot.coalesced_events = self.coalesced_events.load(Ordering::Relaxed);
        snapshot.chunks_added = self.chunks_added.load(Ordering::Relaxed);
        snapshot.chunks_modified = self.chunks_modified.load(Ordering::Relaxed);
        snapshot.chunks_deleted = self.chunks_deleted.load(Ordering::Relaxed);
    }
}

#[derive(Default)]
struct LatencyAccumulator {
    samples: AtomicU64,
    total_micros: AtomicU64,
    max_micros: AtomicU64,
}

impl LatencyAccumulator {
    fn record(&self, elapsed: Duration) {
        let micros = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        self.samples.fetch_add(1, Ordering::Relaxed);
        self.total_micros.fetch_add(micros, Ordering::Relaxed);
        self.max_micros.fetch_max(micros, Ordering::Relaxed);
    }

    fn snapshot(&self) -> LatencySummary {
        let samples = self.samples.load(Ordering::Relaxed);
        let total_ms = self.total_micros.load(Ordering::Relaxed) as f64 / 1_000.0;
        LatencySummary {
            samples,
            total_ms,
            average_ms: if samples == 0 {
                0.0
            } else {
                total_ms / samples as f64
            },
            max_ms: self.max_micros.load(Ordering::Relaxed) as f64 / 1_000.0,
        }
    }
}

pub(crate) async fn snapshot(
    storage: &SqliteStorage,
    runtime: &RuntimeMetrics,
    workspace_id: Option<&str>,
    embeddings: &dyn EmbeddingProvider,
) -> Result<InstrumentationSnapshot> {
    let mut result = InstrumentationSnapshot {
        embedding_model: embeddings.request_model_name().to_owned(),
        embedding_space: embeddings.embedding_space_id().to_owned(),
        embedding_limits: Some(embeddings.limits()),
        token_counter: embeddings.token_counter_id().to_owned(),
        token_counter_accuracy: embeddings.token_counter_accuracy(),
        capacity_limit_source: "provider_contract".into(),
        runtime_metrics_scope: "process".into(),
        ..Default::default()
    };
    let filter = workspace_id.unwrap_or("");
    result.documents = count(
        storage,
        "SELECT COUNT(*) FROM documents WHERE (? = '' OR workspace_id = ?)",
        filter,
    )
    .await?;
    result.chunks = count(
        storage,
        "SELECT COUNT(*) FROM chunks c JOIN documents d ON d.id = c.document_id WHERE (? = '' OR d.workspace_id = ?)",
        filter,
    )
    .await?;
    result.memories = count(
        storage,
        "SELECT COUNT(*) FROM memories WHERE (? = '' OR workspace_id = ?)",
        filter,
    )
    .await?;
    result.events = count(
        storage,
        "SELECT COUNT(*) FROM events WHERE (? = '' OR workspace_id = ?)",
        filter,
    )
    .await?;
    result.chunks_by_language = grouped_counts(
        storage,
        "SELECT c.language, COUNT(*) FROM chunks c JOIN documents d ON d.id = c.document_id WHERE (? = '' OR d.workspace_id = ?) GROUP BY c.language ORDER BY c.language",
        filter,
    )
    .await?;
    result.analyzer_usage = grouped_counts(
        storage,
        "SELECT analyzer_id, COUNT(*) FROM documents WHERE (? = '' OR workspace_id = ?) GROUP BY analyzer_id ORDER BY analyzer_id",
        filter,
    )
    .await?;
    result.fallback_usage = result
        .analyzer_usage
        .get("generic")
        .copied()
        .unwrap_or_default();
    result.last_indexed_time = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT MAX(indexed_at) FROM documents WHERE (? = '' OR workspace_id = ?)",
    )
    .bind(filter)
    .bind(filter)
    .fetch_one(storage.pool())
    .await?;
    let stored_dimension = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(e.dimension) FROM embeddings e JOIN chunks c ON c.id = e.chunk_id JOIN documents d ON d.id = c.document_id WHERE e.model = ? AND (? = '' OR d.workspace_id = ?)",
    )
    .bind(embeddings.embedding_space_id())
    .bind(filter)
    .bind(filter)
    .fetch_one(storage.pool())
    .await?;
    result.embedding_dimension = embeddings
        .dimension()
        .or_else(|| stored_dimension.and_then(|dimension| usize::try_from(dimension).ok()));
    runtime.apply_to(&mut result);
    Ok(result)
}

async fn count(storage: &SqliteStorage, query: &str, filter: &str) -> Result<usize> {
    let value: i64 = sqlx::query_scalar(query)
        .bind(filter)
        .bind(filter)
        .fetch_one(storage.pool())
        .await?;
    usize::try_from(value)
        .map_err(|_| CortexError::Analysis("negative instrumentation count".into()))
}

async fn grouped_counts(
    storage: &SqliteStorage,
    query: &str,
    filter: &str,
) -> Result<BTreeMap<String, usize>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(query)
        .bind(filter)
        .bind(filter)
        .fetch_all(storage.pool())
        .await?;
    rows.into_iter()
        .map(|(key, value)| {
            usize::try_from(value)
                .map(|value| (key, value))
                .map_err(|_| CortexError::Analysis("negative instrumentation count".into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_metrics_track_jobs_batches_events_and_latencies() {
        let metrics = RuntimeMetrics::default();
        metrics.job_started();
        metrics.record_embedding(5, Some(2), Duration::from_millis(6));
        metrics.record_search(Duration::from_millis(2));
        metrics.record_analyzer(Duration::from_millis(3));
        metrics.record_filesystem_events(7, 3);
        metrics.record_chunks(2, 1, 4);
        metrics.record_terminal_capacity_failure();
        metrics.job_finished(true);
        let mut snapshot = InstrumentationSnapshot::default();
        metrics.apply_to(&mut snapshot);

        assert_eq!(snapshot.pending_jobs, 0);
        assert_eq!(snapshot.failed_jobs, 1);
        assert_eq!(snapshot.embedding_calls, 1);
        assert_eq!(snapshot.embedding_batches, 3);
        assert_eq!(snapshot.filesystem_events, 7);
        assert_eq!(snapshot.coalesced_events, 3);
        assert_eq!(snapshot.chunks_added, 2);
        assert_eq!(snapshot.chunks_modified, 1);
        assert_eq!(snapshot.chunks_deleted, 4);
        assert_eq!(snapshot.terminal_capacity_failures, 1);
        assert_eq!(snapshot.embedding_latency.samples, 1);
        assert_eq!(snapshot.search_latency.samples, 1);
        assert_eq!(snapshot.analyzer_latency.samples, 1);
    }
}
