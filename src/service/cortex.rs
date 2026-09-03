use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
    time::Instant,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    AppConfig, CortexError, Result,
    domain::{
        Checkpoint, ContextCandidatePool, ContextPacket, ContextPin, ContextRequest,
        ContextSourceType, CortexEvent, DecodedEvidence, Document, Episode, EpisodeEvent,
        EpisodeEventAssociationRequest, EpisodeListRequest, EpisodeStartRequest, EpisodeStatus,
        EpisodeTerminalRequest, EventType, EvidenceDecodeResult, ExperienceAssessment,
        ExperienceAssessmentReviewRequest, ExperienceDisputeProposal,
        ExperienceDisputeProposalRequest, ExperienceExplanation, ExperienceRecord,
        ExperienceSearchHit, ExperienceSearchRequest, FailureNormalizationResult, GraphRepairMode,
        GraphRepairOutcome, ImpactReport, MAX_EPISODE_EVENTS, MemoryOrigin, MemoryRecord,
        MemorySupersession, MemoryTrust, MemoryTrustReview, ResumeContext, ResumeContextRequest,
        Session, StructuralReadOptions, StructuralResult, Task, TaskStatus, TemporalContextItem,
        TemporalQuery, WorkingSetEntry, WorkingSetSnapshot, Workspace,
    },
    embedding::{
        EmbeddingLimits, EmbeddingProvider, OpenAiCompatibleEmbeddingProvider, TokenCount,
        TokenCountAccuracy, TokenCounter,
    },
    indexing::{IndexingService, WorkspaceReindexOutcome},
    instrumentation::{InstrumentationSnapshot, RuntimeMetrics, WorkspaceResolutionKind, snapshot},
    parsing::AnalyzerRegistry,
    retrieval::{RetrievalResult, RetrievalService},
    service::{
        ConsolidationService, ContextService, EvidenceService, ExperienceAssessmentService,
        ExperienceSearchService, FailureNormalizationService, HarnessContext,
        HarnessContextRequest, HarnessHydrationRequest, HarnessSelectedSource,
        HydratedContextSource, HydrationAuthorization, HydrationScoreProvenance,
        MemoryConsolidationReport, MemoryConsolidationRequest, MemorySupersessionReviewRequest,
        MemoryTrustReviewRequest, StructuralService,
    },
    storage::SqliteStorage,
    workspace::{PathIdentity, WorkspaceScanner, WorkspaceSelector},
};

const MAX_COLLECTION_LIMIT: usize = 100;
const MAX_EPISODE_TITLE_BYTES: usize = 512;
const MAX_EPISODE_REQUEST_KEY_BYTES: usize = 256;

struct EmbeddingTokenCounter {
    provider: Arc<dyn EmbeddingProvider>,
}

impl TokenCounter for EmbeddingTokenCounter {
    fn count(&self, text: &str) -> TokenCount {
        self.provider.count_tokens(text)
    }

    fn identity(&self) -> &str {
        self.provider.token_counter_id()
    }

    fn accuracy(&self) -> TokenCountAccuracy {
        self.provider.token_counter_accuracy()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    pub workspace: Workspace,
    pub documents_indexed: usize,
    pub chunks_indexed: usize,
    pub last_indexed_at: Option<DateTime<Utc>>,
    pub embedding_model: String,
    pub embedding_space: String,
    pub embedding_limits: EmbeddingLimits,
    pub token_counter: String,
    pub token_counter_accuracy: TokenCountAccuracy,
    pub graph: WorkspaceGraphStatus,
}

/// A deterministic summary of the graph projection for one workspace. It is
/// deliberately separate from retrieval results so callers can decide whether
/// a stale graph is acceptable before issuing a structural read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceGraphStatus {
    pub revision: Option<crate::domain::WorkspaceGraphRevision>,
    pub repair: Option<crate::domain::GraphRepairGeneration>,
    pub is_current: bool,
    pub nodes: usize,
    pub edges: usize,
    pub unresolved_relationships: usize,
    pub languages: Vec<GraphLanguageStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphLanguageStatus {
    pub language: String,
    pub documents: usize,
    pub nodes: usize,
    pub edges: usize,
    pub unresolved_relationships: usize,
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub using_generic_fallback: bool,
    pub capabilities: crate::domain::AnalyzerCapabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceCatalog {
    pub workspaces: Vec<Workspace>,
    pub default_hint_match: Option<Workspace>,
    pub default_hint_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebuildCost {
    pub documents: usize,
    pub chunks: usize,
    pub embeddings: usize,
}

impl RebuildCost {
    fn add(&mut self, other: Self) {
        self.documents += other.documents;
        self.chunks += other.chunks;
        self.embeddings += other.embeddings;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageReadiness {
    pub language: String,
    pub extensions: Vec<String>,
    pub files_discovered: usize,
    pub indexed_documents: usize,
    pub selected_analyzer_id: String,
    pub selected_analyzer_version: String,
    pub bundled_analyzer_available: bool,
    pub bundled_analyzer_configured: bool,
    pub using_generic_fallback: bool,
    pub configured_rebuild: RebuildCost,
    pub recommended_rebuild: RebuildCost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerRecommendation {
    pub language: String,
    pub config_key: String,
    pub analyzer_id: String,
    pub files_affected: usize,
    pub rebuild: RebuildCost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceReadiness {
    pub workspace: Workspace,
    pub ready: bool,
    pub read_only: bool,
    pub files_discovered: usize,
    pub scan_failures: usize,
    pub generic_fallback_files: usize,
    pub supported_fallback_files: usize,
    pub unsupported_fallback_files: usize,
    pub languages: Vec<LanguageReadiness>,
    pub recommendations: Vec<AnalyzerRecommendation>,
    pub configured_rebuild: RebuildCost,
    pub recommended_rebuild: RebuildCost,
    pub replacement_policy: String,
}

struct LanguageReadinessAccumulator {
    report: LanguageReadiness,
    recommended_analyzer_id: Option<String>,
}

pub struct CortexWeaveService {
    config: Arc<AppConfig>,
    storage: Arc<SqliteStorage>,
    analyzers: Arc<AnalyzerRegistry>,
    embeddings: Arc<dyn EmbeddingProvider>,
    indexing: Arc<IndexingService>,
    retrieval: Arc<RetrievalService>,
    structural: Arc<StructuralService>,
    context: Arc<ContextService>,
    evidence: Arc<EvidenceService>,
    failure_normalization: Arc<FailureNormalizationService>,
    consolidation: Arc<ConsolidationService>,
    experience_search: Arc<ExperienceSearchService>,
    experience_assessment: Arc<ExperienceAssessmentService>,
    metrics: Arc<RuntimeMetrics>,
}

impl CortexWeaveService {
    pub async fn open(config: AppConfig) -> Result<Self> {
        let storage = SqliteStorage::open(&config.database.path).await?;
        Self::from_parts(config, storage)
    }

    pub fn from_parts(config: AppConfig, storage: SqliteStorage) -> Result<Self> {
        let embeddings = Arc::new(OpenAiCompatibleEmbeddingProvider::new(&config.embedding)?);
        Self::from_parts_with_embeddings(config, storage, embeddings)
    }

    pub fn from_parts_with_embeddings(
        config: AppConfig,
        storage: SqliteStorage,
        embeddings: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self> {
        config.validate()?;
        let storage = Arc::new(storage);
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &config.languages,
            &config.indexing.generic_chunks,
        ));
        let metrics = Arc::new(RuntimeMetrics::default());
        let indexing = Arc::new(IndexingService::with_config(
            Arc::clone(&storage),
            Arc::clone(&embeddings),
            Arc::clone(&analyzers),
            &config.indexing,
            Arc::clone(&metrics),
        ));
        let structural = Arc::new(StructuralService::new(
            Arc::clone(&storage),
            Arc::clone(&analyzers),
        ));
        let retrieval = Arc::new(RetrievalService::with_structural_metrics(
            Arc::clone(&storage),
            Arc::clone(&embeddings),
            config.retrieval.semantic_weight,
            config.retrieval.lexical_weight,
            config.retrieval.structural.clone(),
            Arc::clone(&structural),
            Arc::clone(&metrics),
        )?);
        let context = Arc::new(ContextService::new_with_token_counter(
            Arc::clone(&storage),
            Arc::clone(&retrieval),
            config.working_set.clone(),
            config.temporal.clone(),
            config.context.clone(),
            Arc::new(EmbeddingTokenCounter {
                provider: Arc::clone(&embeddings),
            }),
        )?);
        let evidence = Arc::new(EvidenceService::standard()?);
        let failure_normalization = Arc::new(FailureNormalizationService::standard()?);
        let consolidation = Arc::new(ConsolidationService::new(
            Arc::clone(&storage),
            Arc::clone(&evidence),
            Arc::clone(&failure_normalization),
        ));
        let experience_search = Arc::new(ExperienceSearchService::new(Arc::clone(&storage)));
        let experience_assessment = Arc::new(ExperienceAssessmentService::new(
            Arc::clone(&storage),
            Arc::clone(&evidence),
            Arc::clone(&failure_normalization),
            Arc::clone(&experience_search),
        ));
        Ok(Self {
            config: Arc::new(config),
            storage,
            analyzers,
            embeddings,
            indexing,
            retrieval,
            structural,
            context,
            evidence,
            failure_normalization,
            consolidation,
            experience_search,
            experience_assessment,
            metrics,
        })
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn storage(&self) -> &SqliteStorage {
        &self.storage
    }

    pub fn analyzers(&self) -> &AnalyzerRegistry {
        &self.analyzers
    }

    pub fn embeddings(&self) -> &dyn EmbeddingProvider {
        self.embeddings.as_ref()
    }

    pub fn indexing(&self) -> &IndexingService {
        &self.indexing
    }

    pub fn indexing_handle(&self) -> Arc<IndexingService> {
        Arc::clone(&self.indexing)
    }

    pub fn retrieval(&self) -> &RetrievalService {
        &self.retrieval
    }

    pub fn structural(&self) -> &StructuralService {
        &self.structural
    }

    pub fn context(&self) -> &ContextService {
        &self.context
    }

    pub fn evidence(&self) -> &EvidenceService {
        &self.evidence
    }

    pub fn failure_normalization(&self) -> &FailureNormalizationService {
        &self.failure_normalization
    }

    pub async fn preview_experience(
        &self,
        request: &crate::domain::ConsolidationRequest,
    ) -> Result<crate::domain::ConsolidationPreview> {
        let started = Instant::now();
        let preview = self.consolidation.preview(request).await?;
        self.metrics
            .record_consolidation_preview(&preview, started.elapsed());
        Ok(preview)
    }

    pub async fn accept_experience(
        &self,
        request: &crate::domain::ConsolidationAcceptanceRequest,
    ) -> Result<crate::domain::ConsolidationAcceptance> {
        let started = Instant::now();
        let acceptance = self.consolidation.accept(request).await?;
        self.metrics
            .record_consolidation_acceptance(&acceptance, started.elapsed());
        Ok(acceptance)
    }

    /// Normalizes already-decoded evidence without mutating events or durable state.
    pub fn normalize_decoded_failure(
        &self,
        evidence: &DecodedEvidence,
    ) -> FailureNormalizationResult {
        let result = self.failure_normalization.normalize(evidence);
        self.metrics.record_failure_normalization(&result);
        result
    }

    /// Returns a typed evidence diagnostic for an event without changing the
    /// event or any persisted state. Legacy event payloads remain raw events
    /// and are reported as unsupported rather than silently inferred.
    pub fn diagnose_event_evidence(&self, event: &CortexEvent) -> EvidenceDecodeResult {
        self.evidence.diagnose(event)
    }

    pub fn storage_handle(&self) -> Arc<SqliteStorage> {
        Arc::clone(&self.storage)
    }

    pub async fn register_workspace(
        &self,
        root_path: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Workspace> {
        let root_path = root_path.into();
        let name = name.into();
        if root_path.trim().is_empty() || name.trim().is_empty() {
            return Err(CortexError::Analysis(
                "workspace root path and name cannot be empty".into(),
            ));
        }
        let root_identity = PathIdentity::existing_directory(Path::new(&root_path))?;
        for existing in self.storage.list_workspaces().await? {
            let existing_identity = PathIdentity::from_path(Path::new(&existing.root_path))?;
            if existing_identity.comparison_key() == root_identity.comparison_key() {
                return Ok(existing);
            }
        }
        let workspace = Workspace::new(root_identity.display_path().to_string_lossy(), name);
        self.storage.insert_workspace_if_absent(&workspace).await
    }

    pub async fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        self.storage.list_workspaces().await
    }

    pub async fn resolve_workspace(
        &self,
        selector: WorkspaceSelector,
        default_hint: Option<WorkspaceSelector>,
    ) -> Result<Workspace> {
        let workspaces = self.storage.list_workspaces().await?;
        let kind = workspace_resolution_kind(&selector, default_hint.as_ref(), workspaces.len());
        let result = resolve_workspace_with_default(&workspaces, selector, default_hint);
        self.metrics.record_workspace_resolution(kind, &result);
        result
    }

    pub async fn resolve_workspace_selection(
        &self,
        workspace_id: Option<String>,
        workspace: Option<WorkspaceSelector>,
        default_hint: Option<WorkspaceSelector>,
    ) -> Result<Workspace> {
        let workspaces = self.storage.list_workspaces().await?;
        let kind = workspace_selection_kind(
            workspace_id.as_deref(),
            workspace.as_ref(),
            default_hint.as_ref(),
            workspaces.len(),
        );
        let result = match (workspace_id, workspace) {
            (Some(id), Some(selector)) => {
                let by_id = resolve_explicit_workspace(&workspaces, WorkspaceSelector::Id(id))?;
                let by_selector = resolve_explicit_workspace(&workspaces, selector)?;
                if by_id.id != by_selector.id {
                    return Err(workspace_ambiguity(
                        "workspace_id and workspace select different registrations",
                        &[by_id, by_selector],
                    ));
                }
                Ok(by_id)
            }
            (Some(id), None) => resolve_explicit_workspace(&workspaces, WorkspaceSelector::Id(id)),
            (None, Some(selector)) => resolve_explicit_workspace(&workspaces, selector),
            (None, None) => resolve_workspace_with_default(
                &workspaces,
                WorkspaceSelector::Default,
                default_hint,
            ),
        };
        self.metrics.record_workspace_resolution(kind, &result);
        result
    }

    pub async fn workspace_catalog(
        &self,
        default_hint: Option<WorkspaceSelector>,
    ) -> Result<WorkspaceCatalog> {
        let workspaces = self.storage.list_workspaces().await?;
        let (default_hint_match, default_hint_error) = match default_hint {
            Some(WorkspaceSelector::Default) | None => (None, None),
            Some(hint) => match resolve_explicit_workspace(&workspaces, hint) {
                Ok(workspace) => (Some(workspace), None),
                Err(error) => (None, Some(error.to_string())),
            },
        };
        Ok(WorkspaceCatalog {
            workspaces,
            default_hint_match,
            default_hint_error,
        })
    }

    pub async fn workspace_status(&self, workspace_id: &str) -> Result<WorkspaceStatus> {
        let workspace = self.require_workspace(workspace_id).await?;
        let documents = self.storage.list_documents(workspace_id).await?;
        let chunks_indexed = self.storage.workspace_chunk_count(workspace_id).await?;
        Ok(WorkspaceStatus {
            workspace,
            documents_indexed: documents.len(),
            chunks_indexed,
            last_indexed_at: documents.iter().map(|document| document.indexed_at).max(),
            embedding_model: self.embeddings.request_model_name().to_owned(),
            embedding_space: self.embeddings.embedding_space_id().to_owned(),
            embedding_limits: self.embeddings.limits(),
            token_counter: self.embeddings.token_counter_id().to_owned(),
            token_counter_accuracy: self.embeddings.token_counter_accuracy(),
            graph: self
                .workspace_graph_status_from_documents(workspace_id, &documents)
                .await?,
        })
    }

    pub async fn workspace_graph_status(&self, workspace_id: &str) -> Result<WorkspaceGraphStatus> {
        self.require_workspace(workspace_id).await?;
        let documents = self.storage.list_documents(workspace_id).await?;
        self.workspace_graph_status_from_documents(workspace_id, &documents)
            .await
    }

    async fn workspace_graph_status_from_documents(
        &self,
        workspace_id: &str,
        documents: &[Document],
    ) -> Result<WorkspaceGraphStatus> {
        let mut revision = self.storage.workspace_graph_revision(workspace_id).await?;
        let repair = self.storage.workspace_graph_repair(workspace_id).await?;
        if let Some(snapshot) = &mut revision
            && snapshot.is_current()
        {
            for document in documents {
                let analyzer = self.analyzers.for_path(Path::new(&document.relative_path));
                let state_is_current = self
                    .storage
                    .graph_analysis_state(&document.id)
                    .await?
                    .is_some_and(|state| {
                        state.content_revision == document.content_revision
                            && state.analyzer_id == analyzer.analyzer_id()
                            && state.analyzer_version == analyzer.analyzer_version()
                            && state.structure_version == analyzer.structure_version()
                            && state.last_error.is_none()
                    });
                if !state_is_current {
                    snapshot.graph_state = crate::domain::GraphState::Stale;
                    break;
                }
            }
        }
        let (nodes, edges, unresolved_relationships) =
            self.storage.workspace_graph_counts(workspace_id).await?;
        let language_counts: BTreeMap<_, _> = self
            .storage
            .workspace_graph_counts_by_language(workspace_id)
            .await?
            .into_iter()
            .map(|(language, nodes, edges, unresolved)| (language, (nodes, edges, unresolved)))
            .collect();
        let mut languages: BTreeMap<String, GraphLanguageStatus> = BTreeMap::new();

        for document in documents {
            let analyzer = self.analyzers.for_path(Path::new(&document.relative_path));
            let entry = languages
                .entry(document.language.clone())
                .or_insert_with(|| GraphLanguageStatus {
                    language: document.language.clone(),
                    documents: 0,
                    nodes: 0,
                    edges: 0,
                    unresolved_relationships: 0,
                    analyzer_id: analyzer.analyzer_id().to_owned(),
                    analyzer_version: analyzer.analyzer_version(),
                    using_generic_fallback: analyzer.analyzer_id() == "generic",
                    capabilities: analyzer.capabilities(),
                });
            entry.documents += 1;
        }
        for (language, entry) in &mut languages {
            if let Some((nodes, edges, unresolved)) = language_counts.get(language) {
                entry.nodes = *nodes;
                entry.edges = *edges;
                entry.unresolved_relationships = *unresolved;
            }
        }

        Ok(WorkspaceGraphStatus {
            is_current: revision
                .as_ref()
                .is_some_and(|revision| revision.is_current())
                && !repair
                    .as_ref()
                    .is_some_and(|repair| repair.state.blocks_structural_reads()),
            revision,
            repair,
            nodes,
            edges,
            unresolved_relationships,
            languages: languages.into_values().collect(),
        })
    }

    pub async fn workspace_readiness(&self, workspace_id: &str) -> Result<WorkspaceReadiness> {
        let workspace = self.require_workspace(workspace_id).await?;
        let scanner = WorkspaceScanner::with_patterns(
            Arc::clone(&self.analyzers),
            self.config.indexing.max_file_bytes,
            self.config.indexing.include_patterns.clone(),
            self.config.indexing.exclude_patterns.clone(),
        );
        let scan = scanner.scan(Path::new(&workspace.root_path))?;
        let persisted: BTreeMap<_, _> = self
            .storage
            .list_documents(workspace_id)
            .await?
            .into_iter()
            .map(|document| (document.relative_path.clone(), document))
            .collect();
        let mut by_language: BTreeMap<String, LanguageReadinessAccumulator> = BTreeMap::new();
        let mut generic_fallback_files = 0;
        let mut supported_fallback_files = 0;
        let mut unsupported_fallback_files = 0;

        for file in &scan.files {
            let selected = self.analyzers.for_path(&file.relative_path);
            let available = self.analyzers.available_for_path(&file.relative_path);
            let language = available
                .as_ref()
                .map(|analyzer| analyzer.language_id())
                .unwrap_or_else(|| selected.language_id())
                .to_owned();
            let using_generic_fallback = selected.analyzer_id() == "generic";
            let bundled_analyzer_configured = available.as_ref().is_some_and(|analyzer| {
                selected.analyzer_id() == analyzer.analyzer_id()
                    && selected.analyzer_version() == analyzer.analyzer_version()
            });
            let recommendation = available
                .as_ref()
                .filter(|_| using_generic_fallback && !bundled_analyzer_configured);
            if using_generic_fallback {
                generic_fallback_files += 1;
                if recommendation.is_some() {
                    supported_fallback_files += 1;
                } else {
                    unsupported_fallback_files += 1;
                }
            }

            let entry = by_language.entry(language.clone()).or_insert_with(|| {
                LanguageReadinessAccumulator {
                    report: LanguageReadiness {
                        language: language.clone(),
                        extensions: Vec::new(),
                        files_discovered: 0,
                        indexed_documents: 0,
                        selected_analyzer_id: selected.analyzer_id().to_owned(),
                        selected_analyzer_version: selected.analyzer_version(),
                        bundled_analyzer_available: available.is_some(),
                        bundled_analyzer_configured,
                        using_generic_fallback,
                        configured_rebuild: RebuildCost::default(),
                        recommended_rebuild: RebuildCost::default(),
                    },
                    recommended_analyzer_id: recommendation
                        .map(|analyzer| analyzer.analyzer_id().to_owned()),
                }
            });
            entry.report.files_discovered += 1;
            if let Some(extension) = file
                .relative_path
                .extension()
                .and_then(|extension| extension.to_str())
            {
                entry.report.extensions.push(extension.to_ascii_lowercase());
            }

            let relative_path = file.relative_path.to_string_lossy().replace('\\', "/");
            let Some(document) = persisted.get(&relative_path) else {
                continue;
            };
            entry.report.indexed_documents += 1;
            let configured_mismatch = !document_matches_analyzer(document, selected.as_ref());
            let recommended_mismatch = recommendation
                .is_some_and(|analyzer| !document_matches_analyzer(document, analyzer.as_ref()));
            if configured_mismatch || recommended_mismatch {
                let cost = self.document_rebuild_cost(document).await?;
                if configured_mismatch {
                    entry.report.configured_rebuild.add(cost);
                }
                if recommended_mismatch {
                    entry.report.recommended_rebuild.add(cost);
                }
            }
        }

        let mut configured_rebuild = RebuildCost::default();
        let mut recommended_rebuild = RebuildCost::default();
        let mut recommendations = Vec::new();
        let languages = by_language
            .into_values()
            .map(|mut entry| {
                entry.report.extensions.sort();
                entry.report.extensions.dedup();
                configured_rebuild.add(entry.report.configured_rebuild);
                recommended_rebuild.add(entry.report.recommended_rebuild);
                if let Some(analyzer_id) = entry.recommended_analyzer_id {
                    recommendations.push(AnalyzerRecommendation {
                        language: entry.report.language.clone(),
                        config_key: format!("languages.{}", entry.report.language),
                        analyzer_id,
                        files_affected: entry.report.files_discovered,
                        rebuild: entry.report.recommended_rebuild,
                    });
                }
                entry.report
            })
            .collect();
        let scan_failures = scan.failed_relative_paths.len();

        Ok(WorkspaceReadiness {
            workspace,
            ready: scan_failures == 0
                && supported_fallback_files == 0
                && configured_rebuild.documents == 0,
            read_only: true,
            files_discovered: scan.files.len(),
            scan_failures,
            generic_fallback_files,
            supported_fallback_files,
            unsupported_fallback_files,
            languages,
            recommendations,
            configured_rebuild,
            recommended_rebuild,
            replacement_policy: "A configured analyzer identity or version change requires explicit reindexing and replaces the affected document chunks and embeddings. Enabling a recommended analyzer has the same replacement cost. This report does not change configuration or start reindexing.".into(),
        })
    }

    async fn document_rebuild_cost(&self, document: &Document) -> Result<RebuildCost> {
        let chunks = self.storage.list_chunks(&document.id).await?;
        let mut embeddings = 0;
        for chunk in &chunks {
            embeddings += usize::from(self.storage.get_embedding(&chunk.id).await?.is_some());
        }
        Ok(RebuildCost {
            documents: 1,
            chunks: chunks.len(),
            embeddings,
        })
    }

    pub async fn instrumentation(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<InstrumentationSnapshot> {
        if let Some(workspace_id) = workspace_id {
            self.require_workspace(workspace_id).await?;
        }
        snapshot(
            &self.storage,
            &self.metrics,
            workspace_id,
            self.embeddings.as_ref(),
        )
        .await
    }

    pub async fn workspace_reindex(&self, workspace_id: &str) -> Result<WorkspaceReindexOutcome> {
        let workspace = self.require_workspace(workspace_id).await?;
        self.indexing.reindex_workspace(&workspace).await
    }

    pub async fn workspace_graph_repair(
        &self,
        workspace_id: &str,
        mode: GraphRepairMode,
    ) -> Result<GraphRepairOutcome> {
        let workspace = self.require_workspace(workspace_id).await?;
        self.indexing.repair_graph(&workspace, mode).await
    }

    pub async fn semantic_search(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        self.retrieval
            .semantic_search(workspace_id, query, limit)
            .await
    }

    pub async fn lexical_search(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        self.retrieval
            .lexical_search(workspace_id, query, limit)
            .await
    }

    pub async fn hybrid_search(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        self.retrieval
            .hybrid_search(workspace_id, query, limit)
            .await
    }

    pub async fn graph_find_symbol(
        &self,
        workspace_id: &str,
        symbol_or_path: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.structural
            .find_symbol(workspace_id, symbol_or_path, options)
            .await
    }

    pub async fn graph_neighbors(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.structural
            .neighbors(workspace_id, node_id, options)
            .await
    }

    pub async fn graph_callers(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.structural
            .callers(workspace_id, node_id, options)
            .await
    }

    pub async fn graph_callees(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.structural
            .callees(workspace_id, node_id, options)
            .await
    }

    pub async fn graph_references(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.structural
            .references(workspace_id, node_id, options)
            .await
    }

    pub async fn graph_implementations(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.structural
            .implementations(workspace_id, node_id, options)
            .await
    }

    pub async fn graph_tests(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.structural.tests(workspace_id, node_id, options).await
    }

    pub async fn graph_dependencies(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.structural
            .dependencies(workspace_id, node_id, options)
            .await
    }

    pub async fn graph_dependents(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.structural
            .dependents(workspace_id, node_id, options)
            .await
    }

    pub async fn graph_impact_symbol(
        &self,
        workspace_id: &str,
        symbol: &str,
        options: &StructuralReadOptions,
    ) -> Result<ImpactReport> {
        self.structural
            .impact_from_symbol(workspace_id, symbol, options)
            .await
    }

    pub async fn graph_impact_path(
        &self,
        workspace_id: &str,
        path: &str,
        options: &StructuralReadOptions,
    ) -> Result<ImpactReport> {
        self.structural
            .impact_from_path(workspace_id, path, options)
            .await
    }

    pub async fn get_item(
        &self,
        workspace_id: &str,
        chunk_id: &str,
    ) -> Result<Option<RetrievalResult>> {
        self.retrieval.get_item(workspace_id, chunk_id).await
    }

    pub async fn temporal_retrieval(
        &self,
        query: TemporalQuery,
    ) -> Result<Vec<TemporalContextItem>> {
        self.context.temporal_retrieval(query).await
    }

    pub async fn build_candidate_pool(
        &self,
        request: ContextRequest,
    ) -> Result<ContextCandidatePool> {
        self.context.build_candidate_pool(request).await
    }

    pub async fn semantic_context(&self, request: ContextRequest) -> Result<ContextPacket> {
        let started = Instant::now();
        let packet = self.context.assemble_context_packet(request).await?;
        self.metrics
            .record_context_packet(&packet, started.elapsed());
        Ok(packet)
    }

    pub async fn prepare_harness_context(
        &self,
        request: HarnessContextRequest,
    ) -> Result<HarnessContext> {
        if request.query.trim().is_empty() {
            return Err(CortexError::Analysis(
                "harness context query cannot be empty".into(),
            ));
        }
        if request.token_budget == 0 {
            return Err(CortexError::Analysis(
                "harness context token budget must be greater than zero".into(),
            ));
        }
        self.validate_provenance(
            &request.workspace_id,
            &Some(request.session_id.clone()),
            &Some(request.task_id.clone()),
        )
        .await?;
        let workspace = self.require_workspace(&request.workspace_id).await?;
        let session = self
            .storage
            .get_session(&request.session_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("session {}", request.session_id)))?;
        if session.ended_at.is_some() {
            return Err(CortexError::Analysis(
                "harness context requires an active session".into(),
            ));
        }
        let task = self
            .storage
            .get_task(&request.task_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("task {}", request.task_id)))?;
        if task.status != TaskStatus::Active {
            return Err(CortexError::Analysis(
                "harness context requires an active task".into(),
            ));
        }

        let mut context_request = ContextRequest::new(&request.workspace_id);
        context_request.session_id = Some(request.session_id);
        context_request.task_id = Some(request.task_id);
        context_request.query = Some(request.query);
        context_request.token_budget = request.token_budget;
        context_request.include_explanation = true;
        let packet = self.semantic_context(context_request).await?;
        let selected_sources = packet
            .items
            .iter()
            .map(|item| HarnessSelectedSource {
                workspace_id: packet.workspace_id.clone(),
                source_id: item.source_id.clone(),
                source_type: item.source_type.clone(),
                path: item.path.clone(),
                symbol: item.symbol.clone(),
                source_segments: item.source_segments.clone(),
                scores: item.scores.clone(),
            })
            .collect();
        Ok(HarnessContext {
            workspace,
            session,
            task,
            packet,
            selected_sources,
        })
    }

    pub async fn hydrate_harness_context(
        &self,
        request: HarnessHydrationRequest,
    ) -> Result<Vec<HydratedContextSource>> {
        self.validate_provenance(
            &request.workspace_id,
            &Some(request.session_id.clone()),
            &Some(request.task_id.clone()),
        )
        .await?;
        let selected: BTreeMap<_, _> = request
            .selected_sources
            .iter()
            .map(|source| (source.source_id.as_str(), source))
            .collect();
        if request
            .selected_sources
            .iter()
            .any(|source| source.workspace_id != request.workspace_id)
        {
            return Err(CortexError::Analysis(
                "selected source provenance belongs to a different workspace".into(),
            ));
        }
        let mut unique = BTreeSet::new();
        if request
            .chunk_ids
            .iter()
            .any(|chunk_id| !unique.insert(chunk_id.as_str()))
        {
            return Err(CortexError::Analysis(
                "harness hydration chunk IDs must be unique".into(),
            ));
        }
        let out_of_packet: Vec<_> = request
            .chunk_ids
            .iter()
            .filter(|chunk_id| !selected.contains_key(chunk_id.as_str()))
            .cloned()
            .collect();
        let override_reason = if out_of_packet.is_empty() {
            None
        } else {
            Some(
                request
                    .override_reason
                    .as_deref()
                    .filter(|reason| !reason.trim().is_empty())
                    .ok_or_else(|| {
                        CortexError::Analysis(format!(
                            "out-of-packet hydration is not authorized for chunk IDs: {}",
                            out_of_packet.join(", ")
                        ))
                    })?
                    .to_owned(),
            )
        };

        let mut hydrated = Vec::with_capacity(request.chunk_ids.len());
        for chunk_id in &request.chunk_ids {
            let source = self
                .get_item(&request.workspace_id, chunk_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("code item {chunk_id}")))?;
            if let Some(selected_source) = selected.get(chunk_id.as_str()) {
                let hydrated_symbol = source
                    .qualified_symbol
                    .as_deref()
                    .or(source.symbol.as_deref());
                if selected_source.source_type != ContextSourceType::Code
                    || selected_source.path.as_deref() != Some(source.path.as_str())
                    || selected_source.symbol.as_deref() != hydrated_symbol
                {
                    return Err(CortexError::Analysis(format!(
                        "selected source provenance no longer matches chunk {chunk_id}"
                    )));
                }
            }
            hydrated.push(source);
        }

        let audit_event = if let Some(reason) = &override_reason {
            let sources = hydrated
                .iter()
                .filter(|source| !selected.contains_key(source.chunk_id.as_str()))
                .map(|source| {
                    json!({
                        "chunk_id": source.chunk_id,
                        "path": source.path,
                        "symbol": source.qualified_symbol.as_ref().or(source.symbol.as_ref()),
                        "score_provenance": "out_of_packet_not_scored",
                    })
                })
                .collect::<Vec<_>>();
            let mut event = CortexEvent::new(
                &request.workspace_id,
                EventType::ContextHydrationOverride,
                json!({
                    "reason": reason,
                    "chunk_ids": out_of_packet,
                    "sources": sources,
                    "packet_source_ids": request
                        .selected_sources
                        .iter()
                        .map(|source| source.source_id.as_str())
                        .collect::<Vec<_>>(),
                }),
            );
            event.session_id = Some(request.session_id.clone());
            event.task_id = Some(request.task_id.clone());
            Some(self.record_event(event).await?)
        } else {
            None
        };

        Ok(hydrated
            .into_iter()
            .map(|source| {
                if let Some(selected_source) = selected.get(source.chunk_id.as_str()) {
                    HydratedContextSource {
                        source,
                        authorization: HydrationAuthorization::PacketSelection,
                        score_provenance: HydrationScoreProvenance::PacketSelection(
                            selected_source.scores.clone(),
                        ),
                    }
                } else {
                    let event = audit_event
                        .as_ref()
                        .expect("out-of-packet hydration must have an audit event");
                    HydratedContextSource {
                        source,
                        authorization: HydrationAuthorization::HarnessOverride {
                            reason: override_reason
                                .clone()
                                .expect("out-of-packet hydration must have a reason"),
                            audit_event_id: event.id.clone(),
                        },
                        score_provenance: HydrationScoreProvenance::OutOfPacketNotScored,
                    }
                }
            })
            .collect())
    }

    pub async fn resume_context(&self, request: ResumeContextRequest) -> Result<ResumeContext> {
        self.context.resume_context(request).await
    }

    pub async fn activate_context_source(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: ContextSourceType,
    ) -> Result<WorkingSetEntry> {
        self.context
            .activate_source(workspace_id, session_id, task_id, source_id, source_type)
            .await
    }

    pub async fn inspect_working_set(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
    ) -> Result<WorkingSetSnapshot> {
        self.context
            .inspect_working_set(workspace_id, session_id, task_id)
            .await
    }

    pub async fn pin_context(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: ContextSourceType,
    ) -> Result<ContextPin> {
        self.context
            .pin_context(workspace_id, session_id, task_id, source_id, source_type)
            .await
    }

    pub async fn unpin_context(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: ContextSourceType,
    ) -> Result<bool> {
        self.context
            .unpin_context(workspace_id, session_id, task_id, source_id, source_type)
            .await
    }

    pub async fn create_checkpoint(&self, checkpoint: Checkpoint) -> Result<Checkpoint> {
        self.context.create_checkpoint(checkpoint).await
    }

    pub async fn latest_checkpoint(&self, workspace_id: &str) -> Result<Option<Checkpoint>> {
        self.context.latest_checkpoint(workspace_id).await
    }

    pub async fn latest_checkpoint_for_session(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Option<Checkpoint>> {
        self.context
            .latest_checkpoint_for_session(workspace_id, session_id)
            .await
    }

    pub async fn latest_checkpoint_for_task(
        &self,
        workspace_id: &str,
        task_id: &str,
    ) -> Result<Option<Checkpoint>> {
        self.context
            .latest_checkpoint_for_task(workspace_id, task_id)
            .await
    }

    pub async fn start_session(&self, workspace_id: &str, metadata: Value) -> Result<Session> {
        self.require_workspace(workspace_id).await?;
        let session = Session::new(workspace_id, metadata);
        self.storage.insert_session(&session).await?;
        Ok(session)
    }

    pub async fn end_session(&self, session_id: &str) -> Result<Session> {
        self.end_session_scoped(None, session_id).await
    }

    pub async fn end_session_in_workspace(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Session> {
        self.end_session_scoped(Some(workspace_id), session_id)
            .await
    }

    async fn end_session_scoped(
        &self,
        expected_workspace_id: Option<&str>,
        session_id: &str,
    ) -> Result<Session> {
        if let Some(workspace_id) = expected_workspace_id {
            self.require_workspace(workspace_id).await?;
        }
        let session = self
            .storage
            .get_session(session_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))?;
        if expected_workspace_id.is_some_and(|workspace_id| session.workspace_id != workspace_id) {
            return Err(CortexError::Analysis(
                "session belongs to a different workspace".into(),
            ));
        }
        if session.ended_at.is_some() {
            return Err(CortexError::Analysis(format!(
                "session {session_id} is already ended"
            )));
        }
        self.storage.end_session(session_id, Utc::now()).await?;
        self.storage
            .get_session(session_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))
    }

    pub async fn start_task(
        &self,
        workspace_id: &str,
        session_id: Option<String>,
        title: impl Into<String>,
        details: Value,
    ) -> Result<Task> {
        self.require_workspace(workspace_id).await?;
        let title = title.into();
        if title.trim().is_empty() {
            return Err(CortexError::Analysis("task title cannot be empty".into()));
        }
        if let Some(session_id) = &session_id {
            let session = self
                .storage
                .get_session(session_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))?;
            if session.workspace_id != workspace_id {
                return Err(CortexError::Analysis(
                    "task session belongs to a different workspace".into(),
                ));
            }
            if session.ended_at.is_some() {
                return Err(CortexError::Analysis(
                    "cannot start a task on an ended session".into(),
                ));
            }
        }
        let mut task = Task::new(workspace_id, session_id, title, details);
        task.status = TaskStatus::Active;
        self.storage.insert_task(&task).await?;
        Ok(task)
    }

    pub async fn update_task(
        &self,
        task_id: &str,
        status: TaskStatus,
        details: Value,
    ) -> Result<Task> {
        let current = self
            .storage
            .get_task(task_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?;
        if matches!(
            current.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        ) {
            return Err(CortexError::Analysis(format!(
                "cannot transition terminal task {task_id}"
            )));
        }
        self.storage
            .update_task_status(task_id, status, &details)
            .await?;
        self.storage
            .get_task(task_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))
    }

    pub async fn complete_task(&self, task_id: &str, details: Value) -> Result<Task> {
        self.update_task(task_id, TaskStatus::Completed, details)
            .await
    }

    pub async fn record_memory(&self, mut memory: MemoryRecord) -> Result<MemoryRecord> {
        validate_memory_integrity(&memory)?;
        self.validate_provenance(&memory.workspace_id, &memory.session_id, &memory.task_id)
            .await?;
        memory.metadata = memory.metadata_for_storage();
        self.storage.insert_memory(&memory).await?;
        Ok(memory)
    }

    pub async fn search_memories(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let limit = limit.min(MAX_COLLECTION_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(match_query) = memory_fts_query(query) else {
            return Ok(Vec::new());
        };
        self.storage
            .search_memories(workspace_id, &match_query, limit)
            .await
    }

    pub async fn recent_memories(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let limit = limit.min(MAX_COLLECTION_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.storage.recent_memories(workspace_id, limit).await
    }

    pub async fn review_memory_trust(
        &self,
        request: MemoryTrustReviewRequest,
    ) -> Result<MemoryTrustReview> {
        validate_review_fields(&request.reviewed_by, &request.reason)?;
        if request.new_trust == MemoryTrust::Unreviewed {
            return Err(CortexError::Analysis(
                "a memory trust review must resolve to trusted or rejected".into(),
            ));
        }
        let memory = self
            .storage
            .memory(&request.workspace_id, &request.memory_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("memory {}", request.memory_id)))?;
        if memory.origin != MemoryOrigin::Imported {
            return Err(CortexError::Analysis(
                "human-authorized memory does not use the imported-memory review flow".into(),
            ));
        }
        if memory.trust == request.new_trust {
            return Err(CortexError::Analysis(
                "memory already has the requested trust state".into(),
            ));
        }
        let review = MemoryTrustReview::new(
            &request.workspace_id,
            &request.memory_id,
            memory.trust,
            request.new_trust,
            request.reviewed_by,
            request.reason,
        );
        self.storage.review_memory_trust(&review).await?;
        Ok(review)
    }

    pub async fn memory_trust_reviews(
        &self,
        workspace_id: &str,
        memory_id: &str,
    ) -> Result<Vec<MemoryTrustReview>> {
        self.storage
            .memory(workspace_id, memory_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("memory {memory_id}")))?;
        self.storage
            .memory_trust_reviews(workspace_id, memory_id)
            .await
    }

    pub async fn consolidate_memories(
        &self,
        request: MemoryConsolidationRequest,
    ) -> Result<MemoryConsolidationReport> {
        self.require_workspace(&request.workspace_id).await?;
        if !(2..=MAX_COLLECTION_LIMIT).contains(&request.memory_ids.len()) {
            return Err(CortexError::Analysis(format!(
                "memory consolidation requires between 2 and {MAX_COLLECTION_LIMIT} memory IDs"
            )));
        }
        let unique_ids = request.memory_ids.iter().collect::<BTreeSet<_>>();
        if unique_ids.len() != request.memory_ids.len() {
            return Err(CortexError::Analysis(
                "memory consolidation IDs must be unique".into(),
            ));
        }
        let mut memories = Vec::with_capacity(request.memory_ids.len());
        for memory_id in &request.memory_ids {
            memories.push(
                self.storage
                    .memory(&request.workspace_id, memory_id)
                    .await?
                    .ok_or_else(|| CortexError::NotFound(format!("memory {memory_id}")))?,
            );
        }
        Ok(super::memory::analyze_memory_consolidation(
            &request.workspace_id,
            memories,
        ))
    }

    pub async fn apply_memory_supersession(
        &self,
        request: MemorySupersessionReviewRequest,
    ) -> Result<MemorySupersession> {
        validate_review_fields(&request.reviewed_by, &request.reason)?;
        if request.superseded_memory_id == request.superseding_memory_id {
            return Err(CortexError::Analysis(
                "a memory cannot supersede itself".into(),
            ));
        }
        for memory_id in [
            &request.superseded_memory_id,
            &request.superseding_memory_id,
        ] {
            let memory = self
                .storage
                .memory(&request.workspace_id, memory_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("memory {memory_id}")))?;
            if !memory.trust.is_context_eligible() {
                return Err(CortexError::Analysis(format!(
                    "memory {memory_id} must be trusted before supersession"
                )));
            }
        }
        let mut supersession = MemorySupersession::new(
            &request.workspace_id,
            request.superseded_memory_id,
            request.superseding_memory_id,
        );
        supersession.reviewed_by = Some(request.reviewed_by);
        supersession.reason = Some(request.reason);
        self.storage
            .insert_memory_supersession(&supersession)
            .await?;
        Ok(supersession)
    }

    pub async fn record_event(&self, event: CortexEvent) -> Result<CortexEvent> {
        self.validate_provenance(&event.workspace_id, &event.session_id, &event.task_id)
            .await?;
        self.storage.insert_event(&event).await?;
        Ok(event)
    }

    pub async fn recent_events(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<CortexEvent>> {
        let limit = limit.min(MAX_COLLECTION_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.storage.recent_events(workspace_id, limit).await
    }

    pub async fn start_episode(&self, request: EpisodeStartRequest) -> Result<Episode> {
        let started = Instant::now();
        validate_episode_title(request.title.as_deref())?;
        self.require_workspace(&request.workspace_id).await?;
        let session = self
            .storage
            .get_session(&request.session_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("session {}", request.session_id)))?;
        if session.workspace_id != request.workspace_id {
            return Err(CortexError::Analysis(
                "episode session belongs to a different workspace".into(),
            ));
        }
        if session.ended_at.is_some() {
            return Err(CortexError::Analysis(
                "cannot start an episode on an ended session".into(),
            ));
        }
        if let Some(task_id) = &request.task_id {
            let task = self
                .storage
                .get_task(task_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?;
            if task.workspace_id != request.workspace_id
                || task.session_id.as_deref() != Some(&request.session_id)
            {
                return Err(CortexError::Analysis(
                    "episode task must belong to the episode workspace and session".into(),
                ));
            }
        }
        let episode = Episode::new(
            request.workspace_id,
            request.session_id,
            request.task_id,
            request.episode_type,
            request.title,
            request.created_by,
        );
        self.storage.insert_episode(&episode).await?;
        self.metrics.record_episode_mutation(started.elapsed());
        Ok(episode)
    }

    pub async fn add_episode_events(
        &self,
        request: EpisodeEventAssociationRequest,
    ) -> Result<Episode> {
        let started = Instant::now();
        validate_episode_event_association_request(&request)?;
        let episode = self
            .require_episode(&request.workspace_id, &request.episode_id)
            .await?;
        for event_id in &request.event_ids {
            let event = self
                .storage
                .event(&request.workspace_id, event_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("event {event_id}")))?;
            if event.session_id.as_deref() != Some(&episode.session_id)
                || event.task_id != episode.task_id
            {
                return Err(CortexError::Analysis(format!(
                    "event {event_id} does not exactly match episode session/task provenance"
                )));
            }
        }
        let request_hash = episode_mutation_hash(
            "add_events",
            &request.workspace_id,
            &request.episode_id,
            request.expected_version,
            &request.event_ids,
        );
        let episode = self
            .storage
            .associate_episode_events(&request, &request_hash, Utc::now())
            .await?;
        self.metrics.record_episode_mutation(started.elapsed());
        Ok(episode)
    }

    pub async fn close_episode(&self, request: EpisodeTerminalRequest) -> Result<Episode> {
        let started = Instant::now();
        let episode = self
            .transition_episode(request, EpisodeStatus::Closed)
            .await?;
        self.metrics.record_episode_mutation(started.elapsed());
        Ok(episode)
    }

    pub async fn abandon_episode(&self, request: EpisodeTerminalRequest) -> Result<Episode> {
        let started = Instant::now();
        let episode = self
            .transition_episode(request, EpisodeStatus::Abandoned)
            .await?;
        self.metrics.record_episode_mutation(started.elapsed());
        Ok(episode)
    }

    pub async fn get_episode(
        &self,
        workspace_id: &str,
        episode_id: &str,
    ) -> Result<Option<Episode>> {
        self.require_workspace(workspace_id).await?;
        self.storage.episode(workspace_id, episode_id).await
    }

    pub async fn list_episodes(&self, request: EpisodeListRequest) -> Result<Vec<Episode>> {
        if request.limit > MAX_COLLECTION_LIMIT {
            return Err(CortexError::Analysis(format!(
                "episode list limit cannot exceed {MAX_COLLECTION_LIMIT}"
            )));
        }
        if request.limit == 0 {
            return Ok(Vec::new());
        }
        self.require_workspace(&request.workspace_id).await?;
        if let Some(session_id) = &request.session_id {
            let session = self
                .storage
                .get_session(session_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))?;
            if session.workspace_id != request.workspace_id {
                return Err(CortexError::Analysis(
                    "episode list session belongs to a different workspace".into(),
                ));
            }
        }
        if let Some(task_id) = &request.task_id {
            let task = self
                .storage
                .get_task(task_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?;
            if task.workspace_id != request.workspace_id {
                return Err(CortexError::Analysis(
                    "episode list task belongs to a different workspace".into(),
                ));
            }
            if request
                .session_id
                .as_deref()
                .is_some_and(|session_id| task.session_id.as_deref() != Some(session_id))
            {
                return Err(CortexError::Analysis(
                    "episode list task and session provenance do not match".into(),
                ));
            }
        }
        self.storage.list_episodes(&request).await
    }

    pub async fn episode_events(
        &self,
        workspace_id: &str,
        episode_id: &str,
        limit: usize,
    ) -> Result<Vec<EpisodeEvent>> {
        if limit > MAX_COLLECTION_LIMIT {
            return Err(CortexError::Analysis(format!(
                "episode event limit cannot exceed {MAX_COLLECTION_LIMIT}"
            )));
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.require_episode(workspace_id, episode_id).await?;
        self.storage
            .episode_events(workspace_id, episode_id, limit)
            .await
    }

    pub async fn get_experience(
        &self,
        workspace_id: &str,
        experience_id: &str,
    ) -> Result<Option<ExperienceRecord>> {
        self.require_workspace(workspace_id).await?;
        self.storage.experience(workspace_id, experience_id).await
    }

    pub async fn search_experiences(
        &self,
        request: &ExperienceSearchRequest,
    ) -> Result<Vec<ExperienceSearchHit>> {
        self.require_workspace(&request.workspace_id).await?;
        let started = Instant::now();
        let hits = self.experience_search.search(request).await?;
        self.metrics
            .record_experience_search(request, started.elapsed());
        Ok(hits)
    }

    pub async fn experience_get(
        &self,
        workspace_id: &str,
        experience_id: &str,
    ) -> Result<Option<ExperienceExplanation>> {
        self.require_workspace(workspace_id).await?;
        self.experience_search
            .get(workspace_id, experience_id)
            .await
    }

    pub async fn review_experience_assessment(
        &self,
        request: ExperienceAssessmentReviewRequest,
    ) -> Result<ExperienceAssessment> {
        self.require_workspace(&request.workspace_id).await?;
        self.experience_assessment.review(request).await
    }

    pub async fn propose_experience_disputes(
        &self,
        request: &ExperienceDisputeProposalRequest,
    ) -> Result<Vec<ExperienceDisputeProposal>> {
        self.require_workspace(&request.workspace_id).await?;
        self.experience_assessment.propose_disputes(request).await
    }

    pub async fn experience_assessments(
        &self,
        workspace_id: &str,
        experience_id: &str,
    ) -> Result<Vec<ExperienceAssessment>> {
        self.require_workspace(workspace_id).await?;
        self.storage
            .experience_assessments(workspace_id, experience_id)
            .await
    }

    pub async fn experience_assessment_history(
        &self,
        workspace_id: &str,
        experience_id: &str,
        after: Option<&crate::domain::ExperienceAssessmentCursor>,
        limit: usize,
    ) -> Result<crate::domain::ExperienceAssessmentPage> {
        self.require_workspace(workspace_id).await?;
        self.storage
            .experience_assessment_page(workspace_id, experience_id, after, limit)
            .await
    }

    async fn transition_episode(
        &self,
        request: EpisodeTerminalRequest,
        status: EpisodeStatus,
    ) -> Result<Episode> {
        validate_episode_terminal_request(&request)?;
        self.require_episode(&request.workspace_id, &request.episode_id)
            .await?;
        let operation = match status {
            EpisodeStatus::Closed => "close",
            EpisodeStatus::Abandoned => "abandon",
            EpisodeStatus::Open | EpisodeStatus::Invalid => {
                unreachable!("terminal status required")
            }
        };
        let request_hash = episode_mutation_hash(
            operation,
            &request.workspace_id,
            &request.episode_id,
            request.expected_version,
            &[],
        );
        self.storage
            .transition_episode(&request, status, &request_hash, Utc::now())
            .await
    }

    async fn require_episode(&self, workspace_id: &str, episode_id: &str) -> Result<Episode> {
        self.require_workspace(workspace_id).await?;
        self.storage
            .episode(workspace_id, episode_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("episode {episode_id}")))
    }

    async fn require_workspace(&self, workspace_id: &str) -> Result<Workspace> {
        self.storage
            .get_workspace(workspace_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("workspace {workspace_id}")))
    }

    async fn validate_provenance(
        &self,
        workspace_id: &str,
        session_id: &Option<String>,
        task_id: &Option<String>,
    ) -> Result<()> {
        self.require_workspace(workspace_id).await?;
        let session = match session_id {
            Some(session_id) => Some(
                self.storage
                    .get_session(session_id)
                    .await?
                    .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))?,
            ),
            None => None,
        };
        let task = match task_id {
            Some(task_id) => Some(
                self.storage
                    .get_task(task_id)
                    .await?
                    .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?,
            ),
            None => None,
        };
        if session
            .as_ref()
            .is_some_and(|session| session.workspace_id != workspace_id)
            || task
                .as_ref()
                .is_some_and(|task| task.workspace_id != workspace_id)
        {
            return Err(CortexError::Analysis(
                "provenance belongs to a different workspace".into(),
            ));
        }
        if let (Some(session), Some(task)) = (session, task)
            && task.session_id.as_deref() != Some(&session.id)
        {
            return Err(CortexError::Analysis(
                "task and session provenance do not match".into(),
            ));
        }
        Ok(())
    }
}

fn validate_episode_title(title: Option<&str>) -> Result<()> {
    if title.is_some_and(|title| {
        title.trim().is_empty() || title.len() > MAX_EPISODE_TITLE_BYTES || title.contains('\0')
    }) {
        return Err(CortexError::Analysis(format!(
            "episode title must be non-empty, contain no NUL, and be at most {MAX_EPISODE_TITLE_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_episode_event_association_request(
    request: &EpisodeEventAssociationRequest,
) -> Result<()> {
    validate_episode_request_key(&request.request_key)?;
    if request.event_ids.is_empty() || request.event_ids.len() > MAX_EPISODE_EVENTS {
        return Err(CortexError::Analysis(format!(
            "episode event association requires between 1 and {MAX_EPISODE_EVENTS} event IDs"
        )));
    }
    let unique_ids = request.event_ids.iter().collect::<BTreeSet<_>>();
    if unique_ids.len() != request.event_ids.len() {
        return Err(CortexError::Analysis(
            "episode event association IDs must be unique and preserve caller order".into(),
        ));
    }
    if request.event_ids.iter().any(|event_id| {
        event_id.trim().is_empty()
            || event_id.len() > MAX_EPISODE_REQUEST_KEY_BYTES
            || event_id.contains('\0')
    }) {
        return Err(CortexError::Analysis(
            "episode event IDs must be bounded non-empty identifiers".into(),
        ));
    }
    Ok(())
}

fn validate_episode_terminal_request(request: &EpisodeTerminalRequest) -> Result<()> {
    validate_episode_request_key(&request.request_key)
}

fn validate_episode_request_key(request_key: &str) -> Result<()> {
    if request_key.trim().is_empty()
        || request_key.len() > MAX_EPISODE_REQUEST_KEY_BYTES
        || request_key.contains('\0')
    {
        return Err(CortexError::Analysis(format!(
            "episode request key must be non-empty, contain no NUL, and be at most {MAX_EPISODE_REQUEST_KEY_BYTES} bytes"
        )));
    }
    Ok(())
}

fn episode_mutation_hash(
    operation: &str,
    workspace_id: &str,
    episode_id: &str,
    expected_version: u64,
    event_ids: &[String],
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cortexweave.episode_mutation.v1\0");
    for value in [operation, workspace_id, episode_id] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(&expected_version.to_le_bytes());
    hasher.update(&(event_ids.len() as u64).to_le_bytes());
    for event_id in event_ids {
        hasher.update(&(event_id.len() as u64).to_le_bytes());
        hasher.update(event_id.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn validate_memory_integrity(memory: &MemoryRecord) -> Result<()> {
    if memory.content.trim().is_empty() {
        return Err(CortexError::Analysis(
            "memory content cannot be empty".into(),
        ));
    }
    match memory.origin {
        MemoryOrigin::HumanAuthorized if memory.trust != MemoryTrust::Trusted => {
            return Err(CortexError::Analysis(
                "human-authorized memory must be trusted when recorded".into(),
            ));
        }
        MemoryOrigin::Imported if memory.trust != MemoryTrust::Unreviewed => {
            return Err(CortexError::Analysis(
                "imported memory must be unreviewed when recorded".into(),
            ));
        }
        MemoryOrigin::Imported if memory.source_segments.is_empty() => {
            return Err(CortexError::Analysis(
                "imported memory requires at least one source segment".into(),
            ));
        }
        _ => {}
    }
    let mut segments_by_source = BTreeMap::<&str, Vec<(u64, u64)>>::new();
    for segment in &memory.source_segments {
        if segment.source.trim().is_empty() || segment.start_byte >= segment.end_byte {
            return Err(CortexError::Analysis(
                "memory source segments require a source and a non-empty byte range".into(),
            ));
        }
        segments_by_source
            .entry(segment.source.as_str())
            .or_default()
            .push((segment.start_byte, segment.end_byte));
    }
    for segments in segments_by_source.values_mut() {
        segments.sort_unstable();
        if segments.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(CortexError::Analysis(
                "memory source segments cannot overlap within one source".into(),
            ));
        }
    }
    if memory
        .claim
        .as_ref()
        .is_some_and(|claim| claim.key.trim().is_empty())
    {
        return Err(CortexError::Analysis(
            "memory claim key cannot be empty".into(),
        ));
    }
    Ok(())
}

fn validate_review_fields(reviewed_by: &str, reason: &str) -> Result<()> {
    if reviewed_by.trim().is_empty() || reason.trim().is_empty() {
        return Err(CortexError::Analysis(
            "memory reviews require a reviewer and reason".into(),
        ));
    }
    Ok(())
}

fn workspace_resolution_kind(
    selector: &WorkspaceSelector,
    default_hint: Option<&WorkspaceSelector>,
    workspace_count: usize,
) -> WorkspaceResolutionKind {
    match selector {
        WorkspaceSelector::Id(_) => WorkspaceResolutionKind::Id,
        WorkspaceSelector::Name(_) => WorkspaceResolutionKind::Name,
        WorkspaceSelector::RootPath(_) | WorkspaceSelector::FileUri(_) => {
            WorkspaceResolutionKind::Path
        }
        WorkspaceSelector::Default if default_hint.is_none() && workspace_count == 1 => {
            WorkspaceResolutionKind::Singleton
        }
        WorkspaceSelector::Default => WorkspaceResolutionKind::Default,
    }
}

fn workspace_selection_kind(
    workspace_id: Option<&str>,
    workspace: Option<&WorkspaceSelector>,
    default_hint: Option<&WorkspaceSelector>,
    workspace_count: usize,
) -> WorkspaceResolutionKind {
    if workspace_id.is_some() {
        WorkspaceResolutionKind::Id
    } else if let Some(selector) = workspace {
        workspace_resolution_kind(selector, None, workspace_count)
    } else if default_hint.is_none() && workspace_count == 1 {
        WorkspaceResolutionKind::Singleton
    } else {
        WorkspaceResolutionKind::Default
    }
}

fn resolve_workspace_with_default(
    workspaces: &[Workspace],
    selector: WorkspaceSelector,
    default_hint: Option<WorkspaceSelector>,
) -> Result<Workspace> {
    match selector {
        WorkspaceSelector::Default => match default_hint {
            Some(WorkspaceSelector::Default) | None => resolve_default_workspace(workspaces),
            Some(hint) => resolve_explicit_workspace(workspaces, hint),
        },
        explicit => resolve_explicit_workspace(workspaces, explicit),
    }
}

fn resolve_default_workspace(workspaces: &[Workspace]) -> Result<Workspace> {
    match workspaces {
        [workspace] => Ok(workspace.clone()),
        [] => Err(workspace_not_found(
            "no workspace selector was supplied and none are registered",
        )),
        candidates => Err(workspace_ambiguity(
            "no workspace selector was supplied",
            candidates,
        )),
    }
}

fn resolve_explicit_workspace(
    workspaces: &[Workspace],
    selector: WorkspaceSelector,
) -> Result<Workspace> {
    match selector {
        WorkspaceSelector::Id(id) => workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .cloned()
            .ok_or_else(|| workspace_not_found(&format!("workspace ID {id:?} was not found"))),
        WorkspaceSelector::Name(name) => resolve_workspace_name(workspaces, &name),
        WorkspaceSelector::RootPath(path) => {
            let identity = PathIdentity::from_path(&path)?;
            resolve_workspace_path(workspaces, &identity, &path.display().to_string())
        }
        WorkspaceSelector::FileUri(uri) => {
            let identity = PathIdentity::from_file_uri(&uri)?;
            resolve_workspace_path(workspaces, &identity, &uri)
        }
        WorkspaceSelector::Default => resolve_default_workspace(workspaces),
    }
}

fn resolve_workspace_name(workspaces: &[Workspace], name: &str) -> Result<Workspace> {
    let exact: Vec<_> = workspaces
        .iter()
        .filter(|workspace| workspace.name == name)
        .cloned()
        .collect();
    match exact.as_slice() {
        [workspace] => return Ok(workspace.clone()),
        [] => {}
        candidates => {
            return Err(workspace_ambiguity(
                &format!("workspace name {name:?} matches multiple registrations"),
                candidates,
            ));
        }
    }

    let insensitive: Vec<_> = workspaces
        .iter()
        .filter(|workspace| workspace.name.eq_ignore_ascii_case(name))
        .cloned()
        .collect();
    match insensitive.as_slice() {
        [workspace] => Ok(workspace.clone()),
        [] => Err(workspace_not_found(&format!(
            "workspace name {name:?} was not found"
        ))),
        candidates => Err(workspace_ambiguity(
            &format!("workspace name {name:?} is ambiguous ignoring case"),
            candidates,
        )),
    }
}

fn resolve_workspace_path(
    workspaces: &[Workspace],
    selector: &PathIdentity,
    selector_display: &str,
) -> Result<Workspace> {
    let mut matches = Vec::new();
    for workspace in workspaces {
        let root = PathIdentity::from_path(Path::new(&workspace.root_path))?;
        if root.contains(selector) {
            matches.push((root.comparison_key().len(), workspace.clone()));
        }
    }
    let Some(longest) = matches.iter().map(|(length, _)| *length).max() else {
        return Err(workspace_not_found(&format!(
            "workspace path {selector_display:?} is not inside a registered root"
        )));
    };
    let candidates: Vec<_> = matches
        .into_iter()
        .filter(|(length, _)| *length == longest)
        .map(|(_, workspace)| workspace)
        .collect();
    match candidates.as_slice() {
        [workspace] => Ok(workspace.clone()),
        _ => Err(workspace_ambiguity(
            &format!("workspace path {selector_display:?} matches duplicate roots"),
            &candidates,
        )),
    }
}

fn workspace_not_found(reason: &str) -> CortexError {
    CortexError::NotFound(format!(
        "{reason}; list registered workspaces or register the intended root, then select it by UUID, exact name, root path, or file URI"
    ))
}

fn workspace_ambiguity(reason: &str, candidates: &[Workspace]) -> CortexError {
    let candidates = candidates
        .iter()
        .map(|workspace| {
            format!(
                "id={} name={:?} root={:?}",
                workspace.id, workspace.name, workspace.root_path
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    CortexError::Ambiguous(format!(
        "{reason}; choose one explicitly. candidates: {candidates}"
    ))
}

fn memory_fts_query(query: &str) -> Option<String> {
    let terms: Vec<_> = query
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != ':'
        })
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.replace('"', "")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

fn document_matches_analyzer(
    document: &Document,
    analyzer: &dyn crate::parsing::LanguageAnalyzer,
) -> bool {
    document.language == analyzer.language_id()
        && document.analyzer_id == analyzer.analyzer_id()
        && document.analyzer_version == analyzer.analyzer_version()
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, sync::Arc};

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        domain::{
            Checkpoint, EpisodeCreator, EpisodeEventAssociationRequest, EpisodeListRequest,
            EpisodeStartRequest, EpisodeStatus, EpisodeTerminalRequest, EpisodeType, EventType,
            EvidenceDecodeResult, MemoryKind, Workspace,
        },
        embedding::{TokenCountAccuracy, provider::MockEmbeddingProvider},
    };

    fn resolver_service(storage: SqliteStorage) -> CortexWeaveService {
        CortexWeaveService::from_parts_with_embeddings(
            AppConfig::default(),
            storage,
            Arc::new(MockEmbeddingProvider::new("resolver", 4)),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn service_can_start_without_an_adapter() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        service.storage().health_check().await.unwrap();
        assert_eq!(
            service
                .analyzers()
                .for_path(std::path::Path::new("x.unknown"))
                .analyzer_id(),
            "generic"
        );
    }

    #[tokio::test]
    async fn readiness_guides_explicit_analyzer_enablement_and_rebuild() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("workspace");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("main.py"),
            "def ensure_ready():\n    return 'ready'\n",
        )
        .unwrap();
        fs::write(root.join("notes.md"), "# Operational notes\n").unwrap();
        let storage = SqliteStorage::in_memory().await.unwrap();
        let mut disabled_config = AppConfig::default();
        disabled_config.languages.python = false;
        let disabled = CortexWeaveService::from_parts_with_embeddings(
            disabled_config,
            storage.clone(),
            Arc::new(MockEmbeddingProvider::new("readiness", 4)),
        )
        .unwrap();
        let workspace = disabled
            .register_workspace(root.to_string_lossy(), "readiness")
            .await
            .unwrap();

        let before_index = disabled.workspace_readiness(&workspace.id).await.unwrap();
        assert!(!before_index.ready);
        assert!(before_index.read_only);
        assert_eq!(before_index.files_discovered, 2);
        assert_eq!(before_index.generic_fallback_files, 2);
        assert_eq!(before_index.supported_fallback_files, 1);
        assert_eq!(before_index.unsupported_fallback_files, 1);
        assert_eq!(before_index.recommendations.len(), 1);
        assert_eq!(before_index.recommendations[0].language, "python");
        assert_eq!(
            before_index.recommendations[0].config_key,
            "languages.python"
        );
        assert_eq!(
            before_index.recommendations[0].analyzer_id,
            "tree-sitter-python"
        );
        assert_eq!(
            before_index.recommendations[0].rebuild,
            RebuildCost::default()
        );
        assert!(!disabled.config().languages.python);
        assert!(
            storage
                .find_document(&workspace.id, "main.py")
                .await
                .unwrap()
                .is_none()
        );

        disabled.workspace_reindex(&workspace.id).await.unwrap();
        let generic_document = storage
            .find_document(&workspace.id, "main.py")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(generic_document.analyzer_id, "generic");
        let generic_chunks = storage.list_chunks(&generic_document.id).await.unwrap();
        let generic_chunk_ids: BTreeSet<_> = generic_chunks
            .iter()
            .map(|chunk| chunk.id.clone())
            .collect();
        let generic_readiness = disabled.workspace_readiness(&workspace.id).await.unwrap();
        assert_eq!(generic_readiness.recommended_rebuild.documents, 1);
        assert_eq!(
            generic_readiness.recommended_rebuild.chunks,
            generic_chunks.len()
        );
        assert_eq!(
            generic_readiness.recommended_rebuild.embeddings,
            generic_chunks.len()
        );

        let enabled = CortexWeaveService::from_parts_with_embeddings(
            AppConfig::default(),
            storage.clone(),
            Arc::new(MockEmbeddingProvider::new("readiness", 4)),
        )
        .unwrap();
        let pending = enabled.workspace_readiness(&workspace.id).await.unwrap();
        assert!(!pending.ready);
        assert!(pending.recommendations.is_empty());
        assert_eq!(pending.configured_rebuild.documents, 1);
        assert_eq!(pending.configured_rebuild.chunks, generic_chunks.len());
        assert_eq!(pending.configured_rebuild.embeddings, generic_chunks.len());
        assert_eq!(pending.recommended_rebuild, RebuildCost::default());

        let rebuilt = enabled.workspace_reindex(&workspace.id).await.unwrap();
        assert_eq!(rebuilt.files_updated, 1);
        let structured_document = storage
            .find_document(&workspace.id, "main.py")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(structured_document.language, "python");
        assert_eq!(structured_document.analyzer_id, "tree-sitter-python");
        let structured_chunks = storage.list_chunks(&structured_document.id).await.unwrap();
        assert!(structured_chunks.iter().all(|chunk| chunk.symbol.is_some()));
        assert!(
            structured_chunks
                .iter()
                .all(|chunk| !generic_chunk_ids.contains(&chunk.id))
        );

        let second = enabled.workspace_reindex(&workspace.id).await.unwrap();
        assert_eq!(second.files_updated, 0);
        assert_eq!(second.chunks_embedded, 0);
        assert_eq!(
            storage.list_chunks(&structured_document.id).await.unwrap(),
            structured_chunks
        );
        let ready = enabled.workspace_readiness(&workspace.id).await.unwrap();
        assert!(ready.ready);
        assert_eq!(ready.generic_fallback_files, 1);
        assert_eq!(ready.unsupported_fallback_files, 1);
        assert_eq!(ready.configured_rebuild, RebuildCost::default());
        assert_eq!(ready.recommended_rebuild, RebuildCost::default());
    }

    #[tokio::test]
    async fn facade_exposes_structured_checkpoint_operations() {
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/checkpoint-facade", "checkpoint-facade");
        service
            .storage()
            .insert_workspace(&workspace)
            .await
            .unwrap();
        let session = service
            .start_session(&workspace.id, json!({}))
            .await
            .unwrap();
        let task = service
            .start_task(
                &workspace.id,
                Some(session.id.clone()),
                "checkpoint facade",
                json!({}),
            )
            .await
            .unwrap();
        let mut checkpoint = Checkpoint::new(&workspace.id, &session.id, "checkpoint state");
        checkpoint.task_id = Some(task.id.clone());
        checkpoint.next_action = Some("continue work".into());

        let checkpoint = service.create_checkpoint(checkpoint).await.unwrap();
        assert_eq!(
            service.latest_checkpoint(&workspace.id).await.unwrap(),
            Some(checkpoint.clone())
        );
        assert_eq!(
            service
                .latest_checkpoint_for_session(&workspace.id, &session.id)
                .await
                .unwrap(),
            Some(checkpoint.clone())
        );
        assert_eq!(
            service
                .latest_checkpoint_for_task(&workspace.id, &task.id)
                .await
                .unwrap(),
            Some(checkpoint)
        );
    }

    #[tokio::test]
    async fn workspace_resolution_reports_an_actionable_empty_registry() {
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());

        let error = service
            .resolve_workspace(WorkspaceSelector::Default, None)
            .await
            .unwrap_err();

        assert!(matches!(error, CortexError::NotFound(_)));
        let message = error.to_string();
        assert!(message.contains("none are registered"));
        assert!(message.contains("register the intended root"));
    }

    #[tokio::test]
    async fn workspace_registration_is_idempotent_by_canonical_root() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());

        let first = service
            .register_workspace(root.to_string_lossy(), "original")
            .await
            .unwrap();
        let alternate_spelling = root.to_string_lossy().replace('\\', "/");
        let second = service
            .register_workspace(alternate_spelling, "replacement")
            .await
            .unwrap();

        assert_eq!(second.id, first.id);
        assert_eq!(second.name, "original");
        assert_eq!(
            service.list_workspaces().await.unwrap(),
            vec![first.clone()]
        );
        assert_eq!(
            service
                .resolve_workspace(WorkspaceSelector::Default, None)
                .await
                .unwrap(),
            first
        );
    }

    #[tokio::test]
    async fn explicit_workspace_selectors_outrank_default_hints() {
        let directory = tempdir().unwrap();
        let first_root = directory.path().join("first");
        let second_root = directory.path().join("second");
        fs::create_dir_all(&first_root).unwrap();
        fs::create_dir_all(&second_root).unwrap();
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());
        let first = service
            .register_workspace(first_root.to_string_lossy(), "first")
            .await
            .unwrap();
        let second = service
            .register_workspace(second_root.to_string_lossy(), "second")
            .await
            .unwrap();

        let explicit = service
            .resolve_workspace(
                WorkspaceSelector::Id(first.id.clone()),
                Some(WorkspaceSelector::Name(second.name.clone())),
            )
            .await
            .unwrap();
        let hinted = service
            .resolve_workspace(
                WorkspaceSelector::Default,
                Some(WorkspaceSelector::Name(second.name.clone())),
            )
            .await
            .unwrap();

        assert_eq!(explicit, first);
        assert_eq!(hinted, second);
        let error = service
            .resolve_workspace(WorkspaceSelector::Default, None)
            .await
            .unwrap_err();
        assert!(matches!(error, CortexError::Ambiguous(_)));
        assert!(error.to_string().contains(&explicit.id));
        assert!(error.to_string().contains(&hinted.id));

        let consistent = service
            .resolve_workspace_selection(
                Some(explicit.id.clone()),
                Some(WorkspaceSelector::Name(explicit.name.clone())),
                Some(WorkspaceSelector::Name(hinted.name.clone())),
            )
            .await
            .unwrap();
        assert_eq!(consistent, explicit);

        let conflict = service
            .resolve_workspace_selection(
                Some(explicit.id.clone()),
                Some(WorkspaceSelector::Name(hinted.name.clone())),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(conflict, CortexError::Ambiguous(_)));
        assert!(conflict.to_string().contains(&explicit.id));
        assert!(conflict.to_string().contains(&hinted.id));

        let catalog = service
            .workspace_catalog(Some(WorkspaceSelector::RootPath(second_root)))
            .await
            .unwrap();
        assert_eq!(catalog.workspaces.len(), 2);
        assert_eq!(catalog.default_hint_match, Some(hinted));
        assert_eq!(catalog.default_hint_error, None);
    }

    #[tokio::test]
    async fn workspace_names_require_a_unique_exact_or_case_insensitive_match() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let first = Workspace::new("C:/resolver/one", "Alpha");
        let second = Workspace::new("C:/resolver/two", "ALPHA");
        storage.insert_workspace(&first).await.unwrap();
        storage.insert_workspace(&second).await.unwrap();
        let service = resolver_service(storage);

        assert_eq!(
            service
                .resolve_workspace(WorkspaceSelector::Name("Alpha".into()), None)
                .await
                .unwrap(),
            first
        );
        let case_error = service
            .resolve_workspace(WorkspaceSelector::Name("alpha".into()), None)
            .await
            .unwrap_err();
        assert!(matches!(case_error, CortexError::Ambiguous(_)));
        assert!(case_error.to_string().contains(&first.id));
        assert!(case_error.to_string().contains(&second.id));

        let third = Workspace::new("C:/resolver/three", "duplicate");
        let fourth = Workspace::new("C:/resolver/four", "duplicate");
        service.storage().insert_workspace(&third).await.unwrap();
        service.storage().insert_workspace(&fourth).await.unwrap();
        let duplicate_error = service
            .resolve_workspace(WorkspaceSelector::Name("duplicate".into()), None)
            .await
            .unwrap_err();
        assert!(matches!(duplicate_error, CortexError::Ambiguous(_)));
        assert!(duplicate_error.to_string().contains(&third.root_path));
        assert!(duplicate_error.to_string().contains(&fourth.root_path));
    }

    #[tokio::test]
    async fn path_resolution_uses_the_deepest_registered_root_and_file_uris() {
        let directory = tempdir().unwrap();
        let outer_root = directory.path().join("outer workspace");
        let inner_root = outer_root.join("packages").join("inner workspace");
        let source = inner_root.join("src");
        fs::create_dir_all(&source).unwrap();
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());
        let outer = service
            .register_workspace(outer_root.to_string_lossy(), "outer")
            .await
            .unwrap();
        let inner = service
            .register_workspace(inner_root.to_string_lossy(), "inner")
            .await
            .unwrap();

        let from_root = service
            .resolve_workspace(WorkspaceSelector::RootPath(outer_root.clone()), None)
            .await
            .unwrap();
        let from_child = service
            .resolve_workspace(
                WorkspaceSelector::RootPath(source.join("not-created-yet.rs")),
                None,
            )
            .await
            .unwrap();
        let source_uri = url::Url::from_directory_path(&source).unwrap().to_string();
        let from_uri = service
            .resolve_workspace(WorkspaceSelector::FileUri(source_uri), None)
            .await
            .unwrap();

        assert_eq!(from_root, outer);
        assert_eq!(from_child, inner);
        assert_eq!(from_uri, inner);

        let outside = directory.path().join("outside").join("missing.rs");
        let error = service
            .resolve_workspace(WorkspaceSelector::RootPath(outside), None)
            .await
            .unwrap_err();
        assert!(matches!(error, CortexError::NotFound(_)));
        assert!(error.to_string().contains("not inside a registered root"));
    }

    #[tokio::test]
    async fn records_searches_and_lists_explicit_memory() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/memory", "memory");
        storage.insert_workspace(&workspace).await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        let mut decision = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Decision,
            "Use BLAKE3 for deterministic change detection.",
        );
        decision.related_paths = vec!["src/indexing/reconciler.rs".into()];
        decision.metadata = json!({ "author": "agent" });
        let saved = service.record_memory(decision).await.unwrap();

        let search = service
            .search_memories(&workspace.id, "Why are we using BLAKE3?", 5)
            .await
            .unwrap();
        assert_eq!(search, vec![saved.clone()]);
        assert_eq!(search[0].related_paths, ["src/indexing/reconciler.rs"]);
        assert_eq!(search[0].metadata["author"], "agent");
        assert_eq!(
            service.recent_memories(&workspace.id, 5).await.unwrap(),
            vec![saved]
        );
        assert!(
            service
                .search_memories(&workspace.id, "", 5)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            service
                .recent_memories(&workspace.id, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn memory_search_survives_a_service_restart() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("cortexweave.sqlite");
        let workspace = Workspace::new("C:/restart-memory", "restart-memory");

        {
            let storage = SqliteStorage::open(&database).await.unwrap();
            storage.insert_workspace(&workspace).await.unwrap();
            let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
            let memory = MemoryRecord::new(
                &workspace.id,
                MemoryKind::Decision,
                "Use BLAKE3 for deterministic change detection.",
            );
            service.record_memory(memory).await.unwrap();
        }

        let restarted = CortexWeaveService::from_parts(
            AppConfig::default(),
            SqliteStorage::open(&database).await.unwrap(),
        )
        .unwrap();
        let results = restarted
            .search_memories(&workspace.id, "Why are we using BLAKE3?", 5)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, MemoryKind::Decision);
        assert_eq!(
            results[0].content,
            "Use BLAKE3 for deterministic change detection."
        );
    }

    #[tokio::test]
    async fn ending_a_session_checks_its_workspace_when_one_is_resolved() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let first = Workspace::new("C:/sessions/first", "first");
        let second = Workspace::new("C:/sessions/second", "second");
        storage.insert_workspace(&first).await.unwrap();
        storage.insert_workspace(&second).await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        let session = service.start_session(&first.id, json!({})).await.unwrap();

        assert!(
            service
                .end_session_in_workspace(&second.id, &session.id)
                .await
                .is_err()
        );
        assert!(
            service
                .end_session_in_workspace(&first.id, &session.id)
                .await
                .unwrap()
                .ended_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn records_harness_events_without_automatic_interpretation() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/events", "events");
        storage.insert_workspace(&workspace).await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        let types = [
            EventType::ExternalToolStarted,
            EventType::ExternalToolFinished,
            EventType::CompilerResult,
            EventType::TestResult,
            EventType::TaskUpdated,
        ];

        for event_type in types {
            service
                .record_event(CortexEvent::new(
                    &workspace.id,
                    event_type,
                    json!({ "ok": true }),
                ))
                .await
                .unwrap();
        }

        let events = service.recent_events(&workspace.id, 10).await.unwrap();
        assert_eq!(events.len(), 5);
        assert!(events.iter().all(|event| event.payload["ok"] == true));
        assert!(
            service
                .recent_events(&workspace.id, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn facade_diagnoses_typed_evidence_without_mutating_raw_events() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let service = resolver_service(storage);
        let directory = tempdir().unwrap();
        let workspace = service
            .register_workspace(directory.path().to_string_lossy(), "evidence")
            .await
            .unwrap();
        let session = service
            .start_session(&workspace.id, json!({}))
            .await
            .unwrap();
        let task = service
            .start_task(
                &workspace.id,
                Some(session.id.clone()),
                "diagnose",
                json!({}),
            )
            .await
            .unwrap();

        let legacy = CortexEvent::new(
            &workspace.id,
            EventType::CompilerResult,
            json!({ "ok": false }),
        );
        assert!(matches!(
            service.diagnose_event_evidence(&legacy),
            EvidenceDecodeResult::Unsupported { .. }
        ));

        let mut typed = CortexEvent::new(
            &workspace.id,
            EventType::CompilerResult,
            json!({
                "contract": "cortexweave.rust_compiler_result",
                "version": 1,
                "subject": { "kind": "target", "value": "cortexweave" },
                "exit_code": 1,
                "diagnostics": [{
                    "level": "error",
                    "code": "E0308",
                    "message": "mismatched types",
                    "path": "src/lib.rs",
                    "start_line": 1,
                    "start_column": 1
                }]
            }),
        );
        typed.session_id = Some(session.id.clone());
        typed.task_id = Some(task.id.clone());
        let before = typed.clone();
        assert!(matches!(
            service.diagnose_event_evidence(&typed),
            EvidenceDecodeResult::Decoded { .. }
        ));
        assert_eq!(typed, before);

        service.record_event(typed).await.unwrap();
        let stored = service.recent_events(&workspace.id, 1).await.unwrap();
        assert_eq!(stored[0].payload, before.payload);
    }

    #[tokio::test]
    async fn episodes_preserve_exact_scope_order_lifecycle_and_idempotency() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let service = resolver_service(storage);
        let directory = tempdir().unwrap();
        let workspace = service
            .register_workspace(directory.path().to_string_lossy(), "episodes")
            .await
            .unwrap();
        let session = service
            .start_session(&workspace.id, json!({}))
            .await
            .unwrap();
        let task = service
            .start_task(
                &workspace.id,
                Some(session.id.clone()),
                "episode scope",
                json!({}),
            )
            .await
            .unwrap();

        let episode = service
            .start_episode(EpisodeStartRequest {
                workspace_id: workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(task.id.clone()),
                episode_type: EpisodeType::Debugging,
                title: Some("repair a failing check".into()),
                created_by: EpisodeCreator::User,
            })
            .await
            .unwrap();

        let mut first_event = CortexEvent::new(&workspace.id, EventType::CompilerResult, json!({}));
        first_event.session_id = Some(session.id.clone());
        first_event.task_id = Some(task.id.clone());
        service.record_event(first_event.clone()).await.unwrap();
        let mut second_event = CortexEvent::new(&workspace.id, EventType::TestResult, json!({}));
        second_event.session_id = Some(session.id.clone());
        second_event.task_id = Some(task.id.clone());
        service.record_event(second_event.clone()).await.unwrap();

        let add = EpisodeEventAssociationRequest {
            workspace_id: workspace.id.clone(),
            episode_id: episode.id.clone(),
            expected_version: 0,
            request_key: "episode-add-1".into(),
            event_ids: vec![second_event.id.clone(), first_event.id.clone()],
        };
        let associated = service.add_episode_events(add.clone()).await.unwrap();
        assert_eq!(associated.version, 1);
        assert_eq!(
            service
                .episode_events(&workspace.id, &episode.id, 10)
                .await
                .unwrap()
                .into_iter()
                .map(|member| member.event_id)
                .collect::<Vec<_>>(),
            vec![second_event.id.clone(), first_event.id.clone()]
        );
        assert_eq!(
            service.add_episode_events(add).await.unwrap().version,
            1,
            "the same durable request is idempotent"
        );

        let reuse_error = service
            .add_episode_events(EpisodeEventAssociationRequest {
                workspace_id: workspace.id.clone(),
                episode_id: episode.id.clone(),
                expected_version: 1,
                request_key: "episode-add-1".into(),
                event_ids: vec![first_event.id.clone()],
            })
            .await
            .unwrap_err();
        assert!(matches!(reuse_error, CortexError::Conflict(_)));

        let closed = service
            .close_episode(EpisodeTerminalRequest {
                workspace_id: workspace.id.clone(),
                episode_id: episode.id.clone(),
                expected_version: 1,
                request_key: "episode-close-1".into(),
            })
            .await
            .unwrap();
        assert_eq!(closed.status, EpisodeStatus::Closed);
        assert_eq!(closed.version, 2);
        assert!(closed.ended_at.is_some());
        assert_eq!(
            service
                .close_episode(EpisodeTerminalRequest {
                    workspace_id: workspace.id.clone(),
                    episode_id: episode.id.clone(),
                    expected_version: 1,
                    request_key: "episode-close-1".into(),
                })
                .await
                .unwrap()
                .version,
            2
        );
        assert!(matches!(
            service
                .add_episode_events(EpisodeEventAssociationRequest {
                    workspace_id: workspace.id.clone(),
                    episode_id: episode.id.clone(),
                    expected_version: 2,
                    request_key: "episode-add-after-close".into(),
                    event_ids: vec![first_event.id.clone()],
                })
                .await,
            Err(CortexError::Conflict(_))
        ));

        let listed = service
            .list_episodes(EpisodeListRequest {
                workspace_id: workspace.id.clone(),
                session_id: Some(session.id),
                task_id: Some(task.id),
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(listed, vec![closed]);
    }

    #[tokio::test]
    async fn episodes_reject_scope_mismatch_and_concurrent_stale_membership() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let service = Arc::new(resolver_service(storage));
        let directory = tempdir().unwrap();
        let workspace = service
            .register_workspace(directory.path().to_string_lossy(), "episode-concurrency")
            .await
            .unwrap();
        let session = service
            .start_session(&workspace.id, json!({}))
            .await
            .unwrap();
        let task = service
            .start_task(
                &workspace.id,
                Some(session.id.clone()),
                "concurrent episode",
                json!({}),
            )
            .await
            .unwrap();
        let episode = service
            .start_episode(EpisodeStartRequest {
                workspace_id: workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(task.id.clone()),
                episode_type: EpisodeType::Implementation,
                title: None,
                created_by: EpisodeCreator::NativeHarness,
            })
            .await
            .unwrap();

        let mut scoped = CortexEvent::new(&workspace.id, EventType::TaskUpdated, json!({}));
        scoped.session_id = Some(session.id.clone());
        scoped.task_id = Some(task.id.clone());
        service.record_event(scoped.clone()).await.unwrap();
        let mut second = CortexEvent::new(&workspace.id, EventType::TaskUpdated, json!({}));
        second.session_id = Some(session.id.clone());
        second.task_id = Some(task.id.clone());
        service.record_event(second.clone()).await.unwrap();
        let unscoped = CortexEvent::new(&workspace.id, EventType::FileModified, json!({}));
        service.record_event(unscoped.clone()).await.unwrap();

        assert!(matches!(
            service
                .add_episode_events(EpisodeEventAssociationRequest {
                    workspace_id: workspace.id.clone(),
                    episode_id: episode.id.clone(),
                    expected_version: 0,
                    request_key: "reject-unscoped".into(),
                    event_ids: vec![unscoped.id],
                })
                .await,
            Err(CortexError::Analysis(_))
        ));

        let first_request = EpisodeEventAssociationRequest {
            workspace_id: workspace.id.clone(),
            episode_id: episode.id.clone(),
            expected_version: 0,
            request_key: "concurrent-first".into(),
            event_ids: vec![scoped.id.clone()],
        };
        let second_request = EpisodeEventAssociationRequest {
            workspace_id: workspace.id.clone(),
            episode_id: episode.id.clone(),
            expected_version: 0,
            request_key: "concurrent-second".into(),
            event_ids: vec![second.id.clone()],
        };
        let (first, second) = tokio::join!(
            service.add_episode_events(first_request),
            service.add_episode_events(second_request)
        );
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert!(matches!(
            first.as_ref().err().or(second.as_ref().err()),
            Some(CortexError::Conflict(_))
        ));
        let members = service
            .episode_events(&workspace.id, &episode.id, 10)
            .await
            .unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].ordinal, 0);
        assert_eq!(
            service
                .get_episode(&workspace.id, &episode.id)
                .await
                .unwrap()
                .unwrap()
                .version,
            1
        );

        let close_race = service
            .start_episode(EpisodeStartRequest {
                workspace_id: workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(task.id.clone()),
                episode_type: EpisodeType::Verification,
                title: None,
                created_by: EpisodeCreator::NativeHarness,
            })
            .await
            .unwrap();
        let mut close_race_event =
            CortexEvent::new(&workspace.id, EventType::TestResult, json!({}));
        close_race_event.session_id = Some(session.id);
        close_race_event.task_id = Some(task.id);
        service
            .record_event(close_race_event.clone())
            .await
            .unwrap();
        let add = EpisodeEventAssociationRequest {
            workspace_id: workspace.id.clone(),
            episode_id: close_race.id.clone(),
            expected_version: 0,
            request_key: "race-add".into(),
            event_ids: vec![close_race_event.id],
        };
        let close = EpisodeTerminalRequest {
            workspace_id: workspace.id.clone(),
            episode_id: close_race.id.clone(),
            expected_version: 0,
            request_key: "race-close".into(),
        };
        let (add, close) = tokio::join!(
            service.add_episode_events(add),
            service.close_episode(close)
        );
        assert_eq!(usize::from(add.is_ok()) + usize::from(close.is_ok()), 1);
        let raced = service
            .get_episode(&workspace.id, &close_race.id)
            .await
            .unwrap()
            .unwrap();
        let raced_members = service
            .episode_events(&workspace.id, &close_race.id, 10)
            .await
            .unwrap();
        match raced.status {
            EpisodeStatus::Closed => {
                assert!(add.is_err());
                assert!(raced_members.is_empty());
            }
            EpisodeStatus::Open => {
                assert!(close.is_err());
                assert_eq!(raced_members.len(), 1);
            }
            EpisodeStatus::Abandoned | EpisodeStatus::Invalid => unreachable!("race only closes"),
        }
    }

    #[tokio::test]
    async fn episode_torture_cases_reject_ambiguity_without_guessing() {
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());
        let directory = tempdir().unwrap();
        let first_root = directory.path().join("first");
        let second_root = directory.path().join("second");
        fs::create_dir_all(&first_root).unwrap();
        fs::create_dir_all(&second_root).unwrap();
        let first_workspace = service
            .register_workspace(first_root.to_string_lossy(), "first")
            .await
            .unwrap();
        let second_workspace = service
            .register_workspace(second_root.to_string_lossy(), "second")
            .await
            .unwrap();
        let session = service
            .start_session(&first_workspace.id, json!({}))
            .await
            .unwrap();
        let alpha = service
            .start_task(
                &first_workspace.id,
                Some(session.id.clone()),
                "alpha",
                json!({}),
            )
            .await
            .unwrap();
        let beta = service
            .start_task(
                &first_workspace.id,
                Some(session.id.clone()),
                "beta",
                json!({}),
            )
            .await
            .unwrap();
        let alpha_episode = service
            .start_episode(EpisodeStartRequest {
                workspace_id: first_workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(alpha.id.clone()),
                episode_type: EpisodeType::Debugging,
                title: Some("alpha failure and attempts".into()),
                created_by: EpisodeCreator::User,
            })
            .await
            .unwrap();
        let duplicate_candidate = service
            .start_episode(EpisodeStartRequest {
                workspace_id: first_workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(alpha.id.clone()),
                episode_type: EpisodeType::Investigation,
                title: None,
                created_by: EpisodeCreator::User,
            })
            .await
            .unwrap();
        let beta_episode = service
            .start_episode(EpisodeStartRequest {
                workspace_id: first_workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(beta.id.clone()),
                episode_type: EpisodeType::Debugging,
                title: Some("unrelated beta failure".into()),
                created_by: EpisodeCreator::User,
            })
            .await
            .unwrap();

        let mut alpha_events = Vec::new();
        for event_type in [
            EventType::CompilerResult,
            EventType::FileModified,
            EventType::CompilerResult,
            EventType::FileModified,
            EventType::TestResult,
        ] {
            let mut event = CortexEvent::new(&first_workspace.id, event_type, json!({}));
            event.session_id = Some(session.id.clone());
            event.task_id = Some(alpha.id.clone());
            alpha_events.push(service.record_event(event).await.unwrap());
        }
        let mut beta_failure =
            CortexEvent::new(&first_workspace.id, EventType::CompilerResult, json!({}));
        beta_failure.session_id = Some(session.id.clone());
        beta_failure.task_id = Some(beta.id.clone());
        service.record_event(beta_failure.clone()).await.unwrap();

        service
            .add_episode_events(EpisodeEventAssociationRequest {
                workspace_id: first_workspace.id.clone(),
                episode_id: alpha_episode.id.clone(),
                expected_version: 0,
                request_key: "alpha-sequence".into(),
                event_ids: alpha_events.iter().map(|event| event.id.clone()).collect(),
            })
            .await
            .unwrap();
        service
            .add_episode_events(EpisodeEventAssociationRequest {
                workspace_id: first_workspace.id.clone(),
                episode_id: beta_episode.id.clone(),
                expected_version: 0,
                request_key: "beta-failure".into(),
                event_ids: vec![beta_failure.id.clone()],
            })
            .await
            .unwrap();

        assert!(matches!(
            service
                .add_episode_events(EpisodeEventAssociationRequest {
                    workspace_id: first_workspace.id.clone(),
                    episode_id: alpha_episode.id.clone(),
                    expected_version: 1,
                    request_key: "task-switch".into(),
                    event_ids: vec![beta_failure.id],
                })
                .await,
            Err(CortexError::Analysis(_))
        ));
        assert!(matches!(
            service
                .add_episode_events(EpisodeEventAssociationRequest {
                    workspace_id: first_workspace.id.clone(),
                    episode_id: duplicate_candidate.id.clone(),
                    expected_version: 0,
                    request_key: "duplicate-primary".into(),
                    event_ids: vec![alpha_events[0].id.clone()],
                })
                .await,
            Err(CortexError::Conflict(_))
        ));

        let abandoned = service
            .start_episode(EpisodeStartRequest {
                workspace_id: first_workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(alpha.id.clone()),
                episode_type: EpisodeType::Investigation,
                title: None,
                created_by: EpisodeCreator::User,
            })
            .await
            .unwrap();
        assert_eq!(
            service
                .abandon_episode(EpisodeTerminalRequest {
                    workspace_id: first_workspace.id.clone(),
                    episode_id: abandoned.id,
                    expected_version: 0,
                    request_key: "abandon".into(),
                })
                .await
                .unwrap()
                .status,
            EpisodeStatus::Abandoned
        );
        let replacement = service
            .start_episode(EpisodeStartRequest {
                workspace_id: first_workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(alpha.id.clone()),
                episode_type: EpisodeType::Investigation,
                title: Some("replacement episode".into()),
                created_by: EpisodeCreator::User,
            })
            .await
            .unwrap();

        service
            .close_episode(EpisodeTerminalRequest {
                workspace_id: first_workspace.id.clone(),
                episode_id: alpha_episode.id.clone(),
                expected_version: 1,
                request_key: "close-alpha".into(),
            })
            .await
            .unwrap();
        let mut late_verifier =
            CortexEvent::new(&first_workspace.id, EventType::TestResult, json!({}));
        late_verifier.session_id = Some(session.id.clone());
        late_verifier.task_id = Some(alpha.id.clone());
        service.record_event(late_verifier.clone()).await.unwrap();
        assert!(matches!(
            service
                .add_episode_events(EpisodeEventAssociationRequest {
                    workspace_id: first_workspace.id.clone(),
                    episode_id: alpha_episode.id,
                    expected_version: 2,
                    request_key: "late-verifier".into(),
                    event_ids: vec![late_verifier.id],
                })
                .await,
            Err(CortexError::Conflict(_))
        ));

        let second_session = service
            .start_session(&second_workspace.id, json!({}))
            .await
            .unwrap();
        let mut foreign_event =
            CortexEvent::new(&second_workspace.id, EventType::CompilerResult, json!({}));
        foreign_event.session_id = Some(second_session.id);
        service.record_event(foreign_event.clone()).await.unwrap();
        assert!(matches!(
            service
                .add_episode_events(EpisodeEventAssociationRequest {
                    workspace_id: first_workspace.id.clone(),
                    episode_id: replacement.id.clone(),
                    expected_version: 0,
                    request_key: "cross-workspace".into(),
                    event_ids: vec![foreign_event.id],
                })
                .await,
            Err(CortexError::NotFound(_))
        ));

        service.end_session(&session.id).await.unwrap();
        assert_eq!(
            service
                .close_episode(EpisodeTerminalRequest {
                    workspace_id: first_workspace.id.clone(),
                    episode_id: beta_episode.id,
                    expected_version: 1,
                    request_key: "close-after-session".into(),
                })
                .await
                .unwrap()
                .status,
            EpisodeStatus::Closed
        );
        assert_eq!(
            service
                .close_episode(EpisodeTerminalRequest {
                    workspace_id: first_workspace.id,
                    episode_id: replacement.id,
                    expected_version: 0,
                    request_key: "close-replacement".into(),
                })
                .await
                .unwrap()
                .status,
            EpisodeStatus::Closed
        );
    }

    #[tokio::test]
    async fn service_rejects_incoherent_provenance_and_terminal_lifecycle_changes() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let first = Workspace::new("C:/first", "first");
        let second = Workspace::new("C:/second", "second");
        storage.insert_workspace(&first).await.unwrap();
        storage.insert_workspace(&second).await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        let first_session = service.start_session(&first.id, json!({})).await.unwrap();
        let second_session = service.start_session(&second.id, json!({})).await.unwrap();
        let task = service
            .start_task(&first.id, Some(first_session.id.clone()), "work", json!({}))
            .await
            .unwrap();

        let mut memory = MemoryRecord::new(&first.id, MemoryKind::Note, "mismatch");
        memory.session_id = Some(second_session.id.clone());
        memory.task_id = Some(task.id.clone());
        assert!(service.record_memory(memory).await.is_err());

        service.complete_task(&task.id, json!({})).await.unwrap();
        assert!(
            service
                .update_task(&task.id, TaskStatus::Active, json!({}))
                .await
                .is_err()
        );
        service.end_session(&first_session.id).await.unwrap();
        assert!(service.end_session(&first_session.id).await.is_err());
        assert!(
            service
                .start_task(&first.id, Some(first_session.id), "late", json!({}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn dedicated_harness_can_use_the_facade_without_an_adapter() {
        let directory = tempdir().unwrap();
        let source_root = directory.path().join("workspace");
        fs::create_dir_all(source_root.join("src")).unwrap();
        fs::write(
            source_root.join("src/lib.rs"),
            "pub trait EmbeddingProvider { fn embed(&self); }\n",
        )
        .unwrap();
        let provider: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new("harness-test", 8));
        let service = CortexWeaveService::from_parts_with_embeddings(
            AppConfig::default(),
            SqliteStorage::in_memory().await.unwrap(),
            provider,
        )
        .unwrap();

        let workspace = service
            .register_workspace(source_root.to_string_lossy(), "harness")
            .await
            .unwrap();
        assert_eq!(
            service.list_workspaces().await.unwrap(),
            std::slice::from_ref(&workspace)
        );

        let session = service
            .start_session(&workspace.id, json!({ "client": "native-harness" }))
            .await
            .unwrap();
        let task = service
            .start_task(
                &workspace.id,
                Some(session.id.clone()),
                "inspect embeddings",
                json!({}),
            )
            .await
            .unwrap();
        assert_eq!(task.status, TaskStatus::Active);

        let reindex = service.workspace_reindex(&workspace.id).await.unwrap();
        assert_eq!(reindex.files_seen, 1);
        let status = service.workspace_status(&workspace.id).await.unwrap();
        assert_eq!(status.documents_indexed, 1);
        assert!(status.chunks_indexed >= 1);
        assert_eq!(status.embedding_model, "harness-test");
        assert!(status.graph.is_current);
        assert!(status.graph.nodes >= 2);
        assert_eq!(status.graph.languages[0].language, "rust");
        assert!(status.graph.languages[0].capabilities.calls);

        let semantic = service
            .semantic_search(&workspace.id, "embedding interface", 5)
            .await
            .unwrap();
        let lexical = service
            .lexical_search(&workspace.id, "EmbeddingProvider", 5)
            .await
            .unwrap();
        assert!(!semantic.is_empty());
        let item = service
            .get_item(&workspace.id, &lexical[0].chunk_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.path, "src/lib.rs");
        assert_eq!(item.scores.semantic, None);

        service
            .activate_context_source(
                &workspace.id,
                &session.id,
                Some(&task.id),
                &item.chunk_id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();
        service
            .pin_context(
                &workspace.id,
                &session.id,
                Some(&task.id),
                &item.chunk_id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();
        let working_set = service
            .inspect_working_set(&workspace.id, &session.id, Some(&task.id))
            .await
            .unwrap();
        assert_eq!(working_set.entries.len(), 1);
        assert_eq!(working_set.pins.len(), 1);

        let mut memory = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Observation,
            "EmbeddingProvider is defined in src/lib.rs.",
        );
        memory.session_id = Some(session.id.clone());
        memory.task_id = Some(task.id.clone());
        memory.related_paths = vec!["src/lib.rs".into()];
        service.record_memory(memory).await.unwrap();
        assert_eq!(
            service
                .search_memories(&workspace.id, "Where is EmbeddingProvider?", 5)
                .await
                .unwrap()[0]
                .related_paths,
            ["src/lib.rs"]
        );

        let mut checkpoint = Checkpoint::new(
            &workspace.id,
            &session.id,
            "The embedding interface was located and verified.",
        );
        checkpoint.task_id = Some(task.id.clone());
        checkpoint.related_paths = vec!["src/lib.rs".into()];
        checkpoint.next_action = Some("Resume from the verified interface.".into());
        let checkpoint = service.create_checkpoint(checkpoint).await.unwrap();

        let mut context_request = ContextRequest::new(&workspace.id);
        context_request.query = Some("EmbeddingProvider".into());
        context_request.session_id = Some(session.id.clone());
        context_request.task_id = Some(task.id.clone());
        context_request.token_budget = 1_024;
        let packet = service.semantic_context(context_request).await.unwrap();
        assert_eq!(packet.workspace_id, workspace.id);
        assert!(packet.estimated_tokens <= packet.token_budget);
        assert!(packet.items.iter().any(|item| item.source_id == task.id));
        assert!(
            packet
                .items
                .iter()
                .any(|item| item.path.as_deref() == Some("src/lib.rs"))
        );

        let mut event = CortexEvent::new(
            &workspace.id,
            EventType::TaskUpdated,
            json!({ "phase": "verified" }),
        );
        event.session_id = Some(session.id.clone());
        event.task_id = Some(task.id.clone());
        service.record_event(event).await.unwrap();
        assert_eq!(
            service.recent_events(&workspace.id, 5).await.unwrap().len(),
            1
        );

        let completed = service
            .complete_task(&task.id, json!({ "result": "found" }))
            .await
            .unwrap();
        assert_eq!(completed.status, TaskStatus::Completed);
        assert!(completed.completed_at.is_some());
        assert!(
            service
                .end_session(&session.id)
                .await
                .unwrap()
                .ended_at
                .is_some()
        );

        let mut resume_request = ResumeContextRequest::new(&workspace.id);
        resume_request.task_id = Some(task.id.clone());
        resume_request.token_budget = 1_024;
        let resume = service.resume_context(resume_request).await.unwrap();
        assert_eq!(resume.selected_task.unwrap().id, task.id);
        assert_eq!(resume.checkpoint.unwrap().id, checkpoint.id);
        assert!(resume.packet.items.iter().any(|item| {
            item.reasons
                .contains(&crate::domain::ContextSelectionReason::CurrentCheckpoint)
        }));
    }

    #[tokio::test]
    async fn instrumentation_combines_persisted_and_runtime_activity() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("lib.rs"),
            "fn indexed() -> i32 { 1 }\n",
        )
        .unwrap();
        fs::write(directory.path().join("notes.unknown"), "fallback text\n").unwrap();
        let provider: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new("metrics-model", 8));
        let service = CortexWeaveService::from_parts_with_embeddings(
            AppConfig::default(),
            SqliteStorage::in_memory().await.unwrap(),
            provider,
        )
        .unwrap();
        let workspace = service
            .register_workspace(directory.path().to_string_lossy(), "metrics")
            .await
            .unwrap();
        service.workspace_reindex(&workspace.id).await.unwrap();
        service
            .semantic_search(&workspace.id, "indexed", 5)
            .await
            .unwrap();
        service
            .record_memory(MemoryRecord::new(
                &workspace.id,
                MemoryKind::Note,
                "instrumentation memory",
            ))
            .await
            .unwrap();
        service
            .record_event(CortexEvent::new(
                &workspace.id,
                EventType::TaskUpdated,
                json!({}),
            ))
            .await
            .unwrap();
        service
            .resolve_workspace(WorkspaceSelector::Id(workspace.id.clone()), None)
            .await
            .unwrap();
        assert!(
            service
                .resolve_workspace(WorkspaceSelector::Name("not-registered".into()), None)
                .await
                .is_err()
        );

        let snapshot = service.instrumentation(Some(&workspace.id)).await.unwrap();
        assert_eq!(snapshot.documents, 2);
        assert!(snapshot.chunks >= 2);
        assert!(snapshot.chunks_by_language.contains_key("rust"));
        assert!(snapshot.chunks_by_language.contains_key("text"));
        assert_eq!(snapshot.analyzer_usage["tree-sitter-rust"], 1);
        assert_eq!(snapshot.fallback_usage, 1);
        assert_eq!(snapshot.memories, 1);
        assert_eq!(snapshot.events, 1);
        assert_eq!(snapshot.pending_jobs, 0);
        assert_eq!(snapshot.failed_jobs, 0);
        assert!(snapshot.embedding_calls >= 3);
        assert!(snapshot.embedding_batches >= 3);
        assert!(snapshot.embedding_latency.samples >= 3);
        assert_eq!(snapshot.search_latency.samples, 1);
        assert_eq!(snapshot.analyzer_latency.samples, 2);
        assert!(snapshot.chunks_added >= 2);
        assert!(snapshot.last_indexed_time.is_some());
        assert_eq!(snapshot.embedding_model, "metrics-model");
        assert_eq!(snapshot.embedding_dimension, Some(8));
        assert_eq!(
            snapshot.token_counter_accuracy,
            TokenCountAccuracy::ConservativeEstimate
        );
        assert_eq!(snapshot.workspace_resolutions_by_id, 1);
        assert_eq!(snapshot.workspace_resolution_misses, 1);
        assert_eq!(snapshot.runtime_metrics_scope, "process");
    }
}
