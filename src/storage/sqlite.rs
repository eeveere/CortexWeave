use std::{path::Path, time::Duration};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::Result;

const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct SqliteStorage {
    pool: SqlitePool,
}

impl SqliteStorage {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| crate::CortexError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(SQLITE_BUSY_TIMEOUT)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn in_memory() -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    pub async fn fts_health_check(&self) -> Result<()> {
        sqlx::query("SELECT COUNT(*) FROM chunk_fts")
            .execute(&self.pool)
            .await?;
        sqlx::query("SELECT COUNT(*) FROM memory_fts")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::Arc,
        time::Duration,
    };

    use chrono::Utc;
    use sqlx::migrate::Migrate;
    use tempfile::TempDir;

    use crate::{
        AppConfig, CortexWeaveService,
        domain::{
            Checkpoint, ContextPin, ContextSourceType, CortexEvent, Document, EmbeddingRecord,
            EventType, GraphAnalysisState, GraphRepairDisposition, GraphRepairGeneration,
            GraphRepairMode, GraphRepairReason, GraphRepairState, GraphState, MemoryKind,
            MemoryRecord, Session, StoredChunk, StructuralReadOptions, Task, WorkingSetEntry,
            Workspace,
        },
        embedding::provider::MockEmbeddingProvider,
        indexing::{IndexingService, WorkspaceWatcher},
        parsing::AnalyzerRegistry,
        service::StructuralService,
    };

    use super::*;

    #[tokio::test]
    async fn migrations_create_foundation_tables() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        storage.health_check().await.unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'workspaces'",
        )
        .fetch_one(storage.pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn file_backed_storage_uses_wal() {
        let directory = tempfile::tempdir().unwrap();
        let storage = SqliteStorage::open(directory.path().join("cortexweave.sqlite"))
            .await
            .unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(storage.pool())
            .await
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[tokio::test]
    async fn migration_0001_database_upgrades_without_losing_existing_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cortexweave.sqlite");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();

        {
            let mut connection = pool.acquire().await.unwrap();
            connection.ensure_migrations_table().await.unwrap();
            let migrations = sqlx::migrate!();
            for migration in migrations.iter().take(1) {
                connection.apply(migration).await.unwrap();
            }
        }

        let legacy = SqliteStorage { pool };
        let workspace = Workspace::new("C:/legacy", "legacy");
        legacy.insert_workspace(&workspace).await.unwrap();
        let session = Session::new(&workspace.id, serde_json::json!({"version": "v0.1"}));
        legacy.insert_session(&session).await.unwrap();
        let task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "preserve legacy state",
            serde_json::json!({}),
        );
        legacy.insert_task(&task).await.unwrap();
        let document = Document::new(&workspace.id, "src/legacy.rs");
        sqlx::query(
            "INSERT INTO documents(id, workspace_id, relative_path, language, analyzer_id, analyzer_version, content_hash, size_bytes, modified_at_ns, indexed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&document.id)
        .bind(&document.workspace_id)
        .bind(&document.relative_path)
        .bind(&document.language)
        .bind(&document.analyzer_id)
        .bind(&document.analyzer_version)
        .bind(&document.content_hash)
        .bind(document.size_bytes)
        .bind(document.modified_at_ns)
        .bind(document.indexed_at)
        .execute(legacy.pool())
        .await
        .unwrap();
        let chunk = StoredChunk::new(&document.id, "legacy::run", "fn run() {}");
        legacy.insert_chunk(&chunk).await.unwrap();
        let embedding = EmbeddingRecord::new(&chunk.id, "legacy-model", vec![0.1, 0.2]);
        legacy.insert_embedding(&embedding).await.unwrap();
        let mut memory = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Keep v0.1 data");
        memory.session_id = Some(session.id.clone());
        memory.task_id = Some(task.id.clone());
        sqlx::query(
            "INSERT INTO memories(id, workspace_id, session_id, task_id, kind, content, metadata_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&memory.id)
        .bind(&memory.workspace_id)
        .bind(&memory.session_id)
        .bind(&memory.task_id)
        .bind(memory.kind.as_str())
        .bind(&memory.content)
        .bind(serde_json::to_string(&memory.metadata_for_storage()).unwrap())
        .bind(memory.created_at)
        .execute(legacy.pool())
        .await
        .unwrap();
        let mut event = CortexEvent::new(
            &workspace.id,
            EventType::TaskStarted,
            serde_json::json!({"task_id": task.id}),
        );
        event.session_id = Some(session.id.clone());
        event.task_id = Some(task.id.clone());
        legacy.insert_event(&event).await.unwrap();
        legacy.pool.close().await;

        let upgraded = SqliteStorage::open(&path).await.unwrap();

        assert_eq!(
            upgraded.get_workspace(&workspace.id).await.unwrap(),
            Some(workspace.clone())
        );
        assert_eq!(
            upgraded.get_session(&session.id).await.unwrap(),
            Some(session)
        );
        assert_eq!(upgraded.get_task(&task.id).await.unwrap(), Some(task));
        let mut migrated_document = document.clone();
        migrated_document.content_revision = 1;
        assert_eq!(
            upgraded
                .find_document(&document.workspace_id, &document.relative_path)
                .await
                .unwrap(),
            Some(migrated_document)
        );
        let graph_revision = upgraded
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(graph_revision.content_revision, 1);
        assert_eq!(graph_revision.graph_content_revision, 0);
        assert_eq!(
            upgraded.list_chunks(&document.id).await.unwrap(),
            vec![chunk]
        );
        assert_eq!(
            upgraded.get_embedding(&embedding.chunk_id).await.unwrap(),
            Some(embedding)
        );
        assert_eq!(
            upgraded
                .recent_memories(&document.workspace_id, 10)
                .await
                .unwrap(),
            vec![memory]
        );
        assert_eq!(
            upgraded
                .recent_events(&document.workspace_id, 10)
                .await
                .unwrap(),
            vec![event]
        );

        let v0_2_tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('working_set_entries', 'context_pins', 'checkpoints', 'memory_supersession')",
        )
        .fetch_one(upgraded.pool())
        .await
        .unwrap();
        assert_eq!(v0_2_tables, 4);

        let applied: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(upgraded.pool())
                .await
                .unwrap();
        assert_eq!(applied, vec![1, 2, 3, 4, 5, 6]);
    }

    #[tokio::test]
    async fn migration_0003_applies_all_later_migrations_in_one_current_binary_open() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cortexweave.sqlite");
        let legacy = storage_at_migration(&path, 3).await;
        let workspace = Workspace::new("C:/skipped-version", "skipped-version");
        legacy.insert_workspace(&workspace).await.unwrap();
        let document = Document::new(&workspace.id, "src/skipped.rs");
        legacy.insert_document(&document).await.unwrap();
        let memory = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Decision,
            "Preserve the migration-0003 memory",
        );
        sqlx::query(
            "INSERT INTO memories(id, workspace_id, session_id, task_id, kind, content, metadata_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&memory.id)
        .bind(&memory.workspace_id)
        .bind(&memory.session_id)
        .bind(&memory.task_id)
        .bind(memory.kind.as_str())
        .bind(&memory.content)
        .bind(serde_json::to_string(&memory.metadata_for_storage()).unwrap())
        .bind(memory.created_at)
        .execute(legacy.pool())
        .await
        .unwrap();
        legacy.pool.close().await;

        let upgraded = SqliteStorage::open(&path).await.unwrap();
        let applied: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(upgraded.pool())
                .await
                .unwrap();
        assert_eq!(applied, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(
            upgraded.recent_memories(&workspace.id, 10).await.unwrap(),
            vec![memory]
        );
        let revision = upgraded
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(revision.content_revision, 1);
        assert_eq!(revision.graph_content_revision, 0);
        assert_eq!(revision.graph_state, GraphState::Stale);
    }

    #[tokio::test]
    async fn forensic_populated_0004_manual_reindex_builds_graph_without_source_or_embedding_changes()
     {
        let fixture = populated_pre_graph_fixture().await;
        let storage = Arc::new(SqliteStorage::open(&fixture.database_path).await.unwrap());
        let provider = Arc::new(MockEmbeddingProvider::new("v0.4.1-forensic", 8));
        let indexer = forensic_indexer(Arc::clone(&storage), provider.clone());

        let before = graph_snapshot(&storage, &fixture.workspace).await;
        println!("forensic manual before: {before:?}");
        assert_eq!(before.graph_state, GraphState::Stale);
        assert_eq!(before.content_revision, 1);
        assert_eq!(before.graph_content_revision, 0);
        assert_eq!(before.analysis_states, 0);
        assert_eq!(before.nodes, 0);
        assert_eq!(before.edges, 0);

        let first = &fixture.documents[0];
        let first_result = indexer
            .reconcile_file(
                &fixture.workspace,
                &fixture.root.join(&first.relative_path),
                Path::new(&first.relative_path),
            )
            .await
            .unwrap();
        assert_eq!(first_result.embedded, 0);
        let during = graph_snapshot(&storage, &fixture.workspace).await;
        println!("forensic manual during: {during:?}");
        assert_eq!(during.graph_state, GraphState::Stale);
        assert_eq!(during.graph_content_revision, 0);
        assert_eq!(during.analysis_states, 1);
        assert!(during.nodes > 0);
        let structural = StructuralService::new(Arc::clone(&storage), configured_analyzers());
        let current_only = structural
            .find_symbol(
                &fixture.workspace.id,
                &first.relative_path,
                &StructuralReadOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(current_only.to_string().contains("is stale"));
        let partial = structural
            .find_symbol(
                &fixture.workspace.id,
                &first.relative_path,
                &StructuralReadOptions {
                    allow_stale: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(partial.snapshot.graph_state, GraphState::Stale);
        assert!(!partial.seeds.is_empty());

        for document in fixture.documents.iter().skip(1) {
            let result = indexer
                .reconcile_file(
                    &fixture.workspace,
                    &fixture.root.join(&document.relative_path),
                    Path::new(&document.relative_path),
                )
                .await
                .unwrap();
            assert_eq!(result.embedded, 0);
        }
        let after = graph_snapshot(&storage, &fixture.workspace).await;
        println!("forensic manual after: {after:?}");
        assert_eq!(after.graph_state, GraphState::Current);
        assert_eq!(after.content_revision, 1);
        assert_eq!(after.graph_content_revision, 1);
        assert_eq!(after.analysis_states, fixture.documents.len());
        assert!(after.nodes >= fixture.documents.len());
        assert!(after.edges > 0);
        assert_eq!(provider.embedded_text_count(), 0);
        assert_preserved_legacy_state(&storage, &fixture).await;

        let second = indexer.reindex_workspace(&fixture.workspace).await.unwrap();
        assert_eq!(second.files_updated, 0);
        assert_eq!(second.chunks_embedded, 0);
        let repair = second.graph_repair.as_ref().unwrap();
        assert_eq!(repair.reason, GraphRepairReason::Current);
        assert_eq!(repair.documents_analyzed, 0);
        assert!(repair.generation_id.is_none());
        assert_eq!(graph_snapshot(&storage, &fixture.workspace).await, after);
    }

    #[tokio::test]
    async fn workspace_reindex_reports_graph_repair_separately_from_source_updates() {
        let fixture = populated_pre_graph_fixture().await;
        let storage = Arc::new(SqliteStorage::open(&fixture.database_path).await.unwrap());
        let provider = Arc::new(MockEmbeddingProvider::new("v0.4.1-forensic", 8));
        let indexer = forensic_indexer(Arc::clone(&storage), provider.clone());

        let result = indexer.reindex_workspace(&fixture.workspace).await.unwrap();
        assert_eq!(result.files_seen, fixture.documents.len());
        assert_eq!(result.files_updated, 0);
        assert_eq!(result.files_failed, 0);
        assert_eq!(result.files_removed, 0);
        assert_eq!(result.chunks_embedded, 0);
        let repair = result.graph_repair.as_ref().unwrap();
        assert_eq!(repair.documents_analyzed, fixture.documents.len());
        assert_eq!(repair.embeddings_computed, 0);
        assert!(!repair.source_revision_changed);
        assert_eq!(provider.embedded_text_count(), 0);
        assert_eq!(
            graph_snapshot(&storage, &fixture.workspace)
                .await
                .graph_state,
            GraphState::Current
        );
        assert_preserved_legacy_state(&storage, &fixture).await;
    }

    #[tokio::test]
    async fn forensic_watcher_startup_can_complete_upgrade_before_manual_reindex() {
        let fixture = populated_pre_graph_fixture().await;
        let storage = Arc::new(SqliteStorage::open(&fixture.database_path).await.unwrap());
        let provider = Arc::new(MockEmbeddingProvider::new("v0.4.1-forensic", 8));
        let indexer = forensic_indexer(Arc::clone(&storage), provider.clone());

        let watcher = WorkspaceWatcher::start(
            fixture.workspace.clone(),
            Arc::clone(&indexer),
            Arc::clone(&storage),
            Duration::from_millis(10),
            32,
        )
        .await
        .unwrap();
        let after_startup = graph_snapshot(&storage, &fixture.workspace).await;
        println!("forensic watcher after startup: {after_startup:?}");
        assert_eq!(after_startup.graph_state, GraphState::Current);
        assert_eq!(provider.embedded_text_count(), 0);

        let manual = indexer.reindex_workspace(&fixture.workspace).await.unwrap();
        assert_eq!(manual.files_seen, fixture.documents.len());
        assert_eq!(manual.files_updated, 0);
        assert_eq!(manual.files_failed, 0);
        assert_eq!(manual.files_removed, 0);
        assert_eq!(manual.chunks_embedded, 0);
        assert_eq!(
            graph_snapshot(&storage, &fixture.workspace).await,
            after_startup
        );
        assert_preserved_legacy_state(&storage, &fixture).await;
        watcher.shutdown().await;
    }

    #[tokio::test]
    async fn graph_repair_reprojects_missing_projections_without_embedding_or_source_changes() {
        let fixture = populated_pre_graph_fixture().await;
        let storage = Arc::new(SqliteStorage::open(&fixture.database_path).await.unwrap());
        let provider = Arc::new(MockEmbeddingProvider::new("v0.4.1-forensic", 8));
        let analyzers = configured_analyzers();
        for document in storage.list_documents(&fixture.workspace.id).await.unwrap() {
            let analyzer = analyzers.for_path(Path::new(&document.relative_path));
            storage
                .upsert_graph_analysis_state(&GraphAnalysisState {
                    document_id: document.id,
                    workspace_id: document.workspace_id,
                    content_revision: document.content_revision,
                    analyzer_id: analyzer.analyzer_id().into(),
                    analyzer_version: analyzer.analyzer_version(),
                    structure_version: analyzer.structure_version(),
                    last_error: None,
                    analyzed_at: Utc::now(),
                })
                .await
                .unwrap();
        }
        let indexer = Arc::new(IndexingService::new(
            Arc::clone(&storage),
            provider.clone(),
            analyzers,
            1,
            1024 * 1024,
        ));

        let plan = indexer
            .graph_repair_plan(&fixture.workspace, GraphRepairMode::IfNeeded)
            .await
            .unwrap();
        assert_eq!(plan.documents_requiring_repair(), fixture.documents.len());
        assert!(
            plan.documents
                .iter()
                .all(|document| document.disposition == GraphRepairDisposition::Repair)
        );
        let result = indexer
            .repair_graph(&fixture.workspace, GraphRepairMode::IfNeeded)
            .await
            .unwrap();
        let after = graph_snapshot(&storage, &fixture.workspace).await;
        assert!(result.error.is_none(), "{result:?}");
        assert_eq!(result.documents_analyzed, fixture.documents.len());
        assert_eq!(result.embeddings_computed, 0);
        assert!(!result.source_revision_changed);
        assert_eq!(
            result.final_graph_state,
            crate::domain::GraphRepairState::Completed
        );
        assert_eq!(provider.embedded_text_count(), 0);
        assert_eq!(after.graph_state, GraphState::Current);
        assert_eq!(after.graph_content_revision, after.content_revision);
        assert_eq!(after.analysis_states, fixture.documents.len());
        assert!(after.nodes > 0);
        assert!(after.facts > 0);
        assert!(after.edges > 0);
        assert_preserved_legacy_state(&storage, &fixture).await;
    }

    #[tokio::test]
    async fn graph_repair_records_source_drift_and_blocks_stale_structural_reads() {
        let fixture = populated_pre_graph_fixture().await;
        let storage = Arc::new(SqliteStorage::open(&fixture.database_path).await.unwrap());
        let provider = Arc::new(MockEmbeddingProvider::new("v0.4.1-forensic", 8));
        let indexer = forensic_indexer(Arc::clone(&storage), provider.clone());
        let initial = indexer
            .repair_graph(&fixture.workspace, GraphRepairMode::IfNeeded)
            .await
            .unwrap();
        assert_eq!(initial.final_graph_state, GraphRepairState::Completed);
        assert_eq!(initial.embeddings_computed, 0);
        fs::write(
            fixture.root.join("src/lib.rs"),
            "pub fn source_changed_after_indexing() {}\n",
        )
        .unwrap();

        let plan = indexer
            .graph_repair_plan(&fixture.workspace, GraphRepairMode::IfNeeded)
            .await
            .unwrap();
        assert!(plan.documents.iter().any(|document| {
            document.disposition == GraphRepairDisposition::RequiresSourceReconciliation
                && document.reason == GraphRepairReason::SourceDrift
        }));
        let outcome = indexer
            .repair_graph(&fixture.workspace, GraphRepairMode::IfNeeded)
            .await
            .unwrap();
        assert_eq!(outcome.final_graph_state, GraphRepairState::Failed);
        assert_eq!(outcome.embeddings_computed, 0);
        assert_eq!(provider.embedded_text_count(), 0);
        let repair = storage
            .workspace_graph_repair(&fixture.workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(repair.state, GraphRepairState::Failed);

        let service = CortexWeaveService::from_parts_with_embeddings(
            AppConfig::default(),
            (*storage).clone(),
            provider,
        )
        .unwrap();
        let status = service
            .workspace_graph_status(&fixture.workspace.id)
            .await
            .unwrap();
        assert!(!status.is_current);
        assert_eq!(status.repair.unwrap().state, GraphRepairState::Failed);

        let structural = StructuralService::new(Arc::clone(&storage), configured_analyzers());
        let error = structural
            .find_symbol(
                &fixture.workspace.id,
                "orchestrate",
                &StructuralReadOptions {
                    allow_stale: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("graph repair"));

        fs::write(
            fixture.root.join("src/lib.rs"),
            "mod worker;\npub fn orchestrate() -> i32 { run() }\n",
        )
        .unwrap();
        let recovery_plan = indexer
            .graph_repair_plan(&fixture.workspace, GraphRepairMode::IfNeeded)
            .await
            .unwrap();
        assert_eq!(recovery_plan.reason, GraphRepairReason::PriorFailure);
        assert!(
            recovery_plan
                .documents
                .iter()
                .all(|document| document.disposition == GraphRepairDisposition::Current)
        );
        let recovered = indexer
            .repair_graph(&fixture.workspace, GraphRepairMode::IfNeeded)
            .await
            .unwrap();
        assert_eq!(recovered.reason, GraphRepairReason::PriorFailure);
        assert_eq!(recovered.final_graph_state, GraphRepairState::Completed);
        assert_eq!(recovered.documents_analyzed, 0);
        assert_eq!(recovered.embeddings_computed, 0);
        let recovered_status = service
            .workspace_graph_status(&fixture.workspace.id)
            .await
            .unwrap();
        assert!(recovered_status.is_current);
    }

    #[tokio::test]
    async fn forced_graph_repair_reprojects_a_current_workspace_without_embeddings() {
        let fixture = populated_pre_graph_fixture().await;
        let storage = Arc::new(SqliteStorage::open(&fixture.database_path).await.unwrap());
        let provider = Arc::new(MockEmbeddingProvider::new("v0.4.1-forensic", 8));
        let indexer = forensic_indexer(Arc::clone(&storage), provider.clone());
        indexer.reindex_workspace(&fixture.workspace).await.unwrap();
        assert_eq!(provider.embedded_text_count(), 0);

        let plan = indexer
            .graph_repair_plan(&fixture.workspace, GraphRepairMode::Force)
            .await
            .unwrap();
        assert_eq!(plan.documents_requiring_repair(), fixture.documents.len());
        assert!(plan.documents.iter().all(|document| {
            document.disposition == GraphRepairDisposition::Repair
                && document.reason == GraphRepairReason::ForceRequested
        }));
        let outcome = indexer
            .repair_graph(&fixture.workspace, GraphRepairMode::Force)
            .await
            .unwrap();
        assert_eq!(outcome.final_graph_state, GraphRepairState::Completed);
        assert_eq!(outcome.documents_analyzed, fixture.documents.len());
        assert_eq!(outcome.embeddings_computed, 0);
        assert!(!outcome.source_revision_changed);
        assert_eq!(provider.embedded_text_count(), 0);
        assert_eq!(
            graph_snapshot(&storage, &fixture.workspace)
                .await
                .graph_state,
            GraphState::Current
        );
        assert_preserved_legacy_state(&storage, &fixture).await;
    }

    #[tokio::test]
    async fn active_repair_blocks_even_explicit_stale_structural_reads() {
        let fixture = populated_pre_graph_fixture().await;
        let storage = Arc::new(SqliteStorage::open(&fixture.database_path).await.unwrap());
        let provider = Arc::new(MockEmbeddingProvider::new("v0.4.1-forensic", 8));
        let indexer = forensic_indexer(Arc::clone(&storage), provider);
        indexer.reindex_workspace(&fixture.workspace).await.unwrap();
        let revision = storage
            .workspace_graph_revision(&fixture.workspace.id)
            .await
            .unwrap()
            .unwrap();
        let now = Utc::now();
        let generation = GraphRepairGeneration {
            workspace_id: fixture.workspace.id.clone(),
            generation_id: uuid::Uuid::new_v4().to_string(),
            mode: GraphRepairMode::Force,
            target_content_revision: revision.content_revision,
            state: GraphRepairState::Active,
            started_at: now,
            lease_expires_at: now + chrono::Duration::seconds(60),
            updated_at: now,
            completed_at: None,
            documents_considered: fixture.documents.len(),
            documents_repaired: 0,
            documents_failed: 0,
            last_error: None,
        };
        assert!(matches!(
            storage
                .acquire_graph_repair(&generation, now)
                .await
                .unwrap(),
            crate::storage::GraphRepairAcquire::Acquired(_)
        ));

        let structural = StructuralService::new(Arc::clone(&storage), configured_analyzers());
        let error = structural
            .find_symbol(
                &fixture.workspace.id,
                "orchestrate",
                &StructuralReadOptions {
                    allow_stale: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("graph repair"));
    }

    #[tokio::test]
    async fn workspace_reindex_recovers_an_abandoned_graph_update_marker() {
        let fixture = populated_pre_graph_fixture().await;
        let storage = Arc::new(SqliteStorage::open(&fixture.database_path).await.unwrap());
        let provider = Arc::new(MockEmbeddingProvider::new("v0.4.1-forensic", 8));
        let indexer = forensic_indexer(Arc::clone(&storage), provider.clone());
        indexer.reindex_workspace(&fixture.workspace).await.unwrap();
        storage
            .mark_graph_updating(&fixture.workspace.id, Utc::now())
            .await
            .unwrap();

        let result = indexer.reindex_workspace(&fixture.workspace).await.unwrap();
        let after = graph_snapshot(&storage, &fixture.workspace).await;
        println!("forensic abandoned update after reindex: {after:?}");
        assert_eq!(result.files_updated, 0);
        assert_eq!(result.chunks_embedded, 0);
        assert_eq!(after.graph_state, GraphState::Current);
        assert_eq!(after.graph_content_revision, after.content_revision);
        assert!(after.nodes > 0);
        assert_eq!(provider.embedded_text_count(), 0);
        let repair = result.graph_repair.as_ref().unwrap();
        assert_eq!(repair.final_graph_state, GraphRepairState::Completed);
    }

    struct PopulatedPreGraphFixture {
        _directory: TempDir,
        root: PathBuf,
        database_path: PathBuf,
        workspace: Workspace,
        documents: Vec<Document>,
        chunks: Vec<StoredChunk>,
        embeddings: Vec<EmbeddingRecord>,
        session: Session,
        task: Task,
        memory: MemoryRecord,
        event: CortexEvent,
        working_set_entry: WorkingSetEntry,
        context_pin: ContextPin,
        checkpoint: Checkpoint,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct GraphForensicSnapshot {
        content_revision: i64,
        graph_content_revision: i64,
        graph_state: GraphState,
        analysis_states: usize,
        nodes: usize,
        facts: usize,
        edges: usize,
        unresolved: usize,
    }

    async fn storage_at_migration(path: &Path, migration_count: usize) -> SqliteStorage {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        {
            let mut connection = pool.acquire().await.unwrap();
            connection.ensure_migrations_table().await.unwrap();
            let migrations = sqlx::migrate!();
            for migration in migrations.iter().take(migration_count) {
                connection.apply(migration).await.unwrap();
            }
        }
        SqliteStorage { pool }
    }

    fn configured_analyzers() -> Arc<AnalyzerRegistry> {
        let config = AppConfig::default();
        Arc::new(AnalyzerRegistry::configured(
            &config.languages,
            &config.indexing.generic_chunks,
        ))
    }

    fn forensic_indexer(
        storage: Arc<SqliteStorage>,
        provider: Arc<MockEmbeddingProvider>,
    ) -> Arc<IndexingService> {
        Arc::new(IndexingService::new(
            storage,
            provider,
            configured_analyzers(),
            1,
            1024 * 1024,
        ))
    }

    async fn populated_pre_graph_fixture() -> PopulatedPreGraphFixture {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("workspace");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tools")).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "mod worker;\npub fn orchestrate() -> i32 { run() }\n",
        )
        .unwrap();
        fs::write(root.join("src/worker.rs"), "pub fn run() -> i32 { 42 }\n").unwrap();
        fs::write(
            root.join("tools/helper.py"),
            "def helper():\n    return 42\n",
        )
        .unwrap();

        let seed_storage = SqliteStorage::in_memory().await.unwrap();
        let seed_provider = Arc::new(MockEmbeddingProvider::new("v0.4.1-forensic", 8));
        let seed_service = CortexWeaveService::from_parts_with_embeddings(
            AppConfig::default(),
            seed_storage.clone(),
            seed_provider,
        )
        .unwrap();
        let workspace = seed_service
            .register_workspace(root.to_string_lossy(), "populated-0004")
            .await
            .unwrap();
        let seeded = seed_service.workspace_reindex(&workspace.id).await.unwrap();
        assert_eq!(seeded.files_updated, 3);

        let mut documents = seed_storage.list_documents(&workspace.id).await.unwrap();
        documents.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let mut chunks = Vec::new();
        let mut embeddings = Vec::new();
        for document in &documents {
            for chunk in seed_storage.list_chunks(&document.id).await.unwrap() {
                embeddings.push(
                    seed_storage
                        .get_embedding(&chunk.id)
                        .await
                        .unwrap()
                        .expect("seeded chunk should have an embedding"),
                );
                chunks.push(chunk);
            }
        }

        let database_path = directory.path().join("populated-0004.sqlite");
        let legacy = storage_at_migration(&database_path, 4).await;
        legacy.insert_workspace(&workspace).await.unwrap();
        for document in &documents {
            legacy.insert_document(document).await.unwrap();
        }
        for chunk in &chunks {
            legacy.insert_chunk(chunk).await.unwrap();
        }
        for embedding in &embeddings {
            legacy.insert_embedding(embedding).await.unwrap();
        }

        let session = Session::new(&workspace.id, serde_json::json!({"fixture": "0004"}));
        legacy.insert_session(&session).await.unwrap();
        let task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "preserve populated upgrade state",
            serde_json::json!({}),
        );
        legacy.insert_task(&task).await.unwrap();
        let mut memory = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Decision,
            "Graph repair must preserve legacy domains",
        );
        memory.session_id = Some(session.id.clone());
        memory.task_id = Some(task.id.clone());
        legacy.insert_memory(&memory).await.unwrap();
        let mut event = CortexEvent::new(
            &workspace.id,
            EventType::TaskStarted,
            serde_json::json!({"task_id": task.id}),
        );
        event.session_id = Some(session.id.clone());
        event.task_id = Some(task.id.clone());
        legacy.insert_event(&event).await.unwrap();

        let source_id = chunks.first().unwrap().id.clone();
        let mut working_set_entry = WorkingSetEntry::new(
            &workspace.id,
            &session.id,
            &source_id,
            ContextSourceType::Code,
            0.75,
        );
        working_set_entry.task_id = Some(task.id.clone());
        legacy
            .insert_working_set_entry(&working_set_entry)
            .await
            .unwrap();
        let mut context_pin = ContextPin::new(
            &workspace.id,
            &session.id,
            &source_id,
            ContextSourceType::Code,
        );
        context_pin.task_id = Some(task.id.clone());
        legacy.insert_context_pin(&context_pin).await.unwrap();
        let mut checkpoint = Checkpoint::new(
            &workspace.id,
            &session.id,
            "Continue from the populated pre-graph state",
        );
        checkpoint.task_id = Some(task.id.clone());
        checkpoint.related_paths = documents
            .iter()
            .map(|document| document.relative_path.clone())
            .collect();
        legacy.insert_checkpoint(&checkpoint).await.unwrap();
        legacy.pool.close().await;

        PopulatedPreGraphFixture {
            _directory: directory,
            root,
            database_path,
            workspace,
            documents,
            chunks,
            embeddings,
            session,
            task,
            memory,
            event,
            working_set_entry,
            context_pin,
            checkpoint,
        }
    }

    async fn graph_snapshot(
        storage: &SqliteStorage,
        workspace: &Workspace,
    ) -> GraphForensicSnapshot {
        let revision = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        let documents = storage.list_documents(&workspace.id).await.unwrap();
        let mut analysis_states = 0;
        for document in documents {
            analysis_states += usize::from(
                storage
                    .graph_analysis_state(&document.id)
                    .await
                    .unwrap()
                    .is_some(),
            );
        }
        GraphForensicSnapshot {
            content_revision: revision.content_revision,
            graph_content_revision: revision.graph_content_revision,
            graph_state: revision.graph_state,
            analysis_states,
            nodes: storage.graph_nodes(&workspace.id).await.unwrap().len(),
            facts: storage
                .graph_relationship_facts(&workspace.id)
                .await
                .unwrap()
                .len(),
            edges: storage.graph_edges(&workspace.id).await.unwrap().len(),
            unresolved: storage
                .unresolved_relationships(&workspace.id)
                .await
                .unwrap()
                .len(),
        }
    }

    async fn assert_preserved_legacy_state(
        storage: &SqliteStorage,
        fixture: &PopulatedPreGraphFixture,
    ) {
        for expected in &fixture.documents {
            let actual = storage
                .find_document(&fixture.workspace.id, &expected.relative_path)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(actual.id, expected.id);
            assert_eq!(actual.content_revision, 1);
            assert_eq!(actual.content_hash, expected.content_hash);
            assert_eq!(actual.segmentation_id, expected.segmentation_id);
        }
        for expected in &fixture.chunks {
            let actual = storage
                .list_chunks(&expected.document_id)
                .await
                .unwrap()
                .into_iter()
                .find(|chunk| chunk.id == expected.id)
                .expect("legacy chunk ID should survive graph-only work");
            assert_eq!(actual.stable_key, expected.stable_key);
            assert_eq!(actual.content_hash, expected.content_hash);
        }
        for expected in &fixture.embeddings {
            assert_eq!(
                storage.get_embedding(&expected.chunk_id).await.unwrap(),
                Some(expected.clone())
            );
        }
        assert_eq!(
            storage.get_session(&fixture.session.id).await.unwrap(),
            Some(fixture.session.clone())
        );
        assert_eq!(
            storage.get_task(&fixture.task.id).await.unwrap(),
            Some(fixture.task.clone())
        );
        assert_eq!(
            storage
                .recent_memories(&fixture.workspace.id, 10)
                .await
                .unwrap(),
            vec![fixture.memory.clone()]
        );
        assert_eq!(
            storage
                .recent_events(&fixture.workspace.id, 10)
                .await
                .unwrap(),
            vec![fixture.event.clone()]
        );
        assert_eq!(
            storage
                .working_set_entries(&fixture.session.id)
                .await
                .unwrap(),
            vec![fixture.working_set_entry.clone()]
        );
        assert_eq!(
            storage.context_pins(&fixture.session.id).await.unwrap(),
            vec![fixture.context_pin.clone()]
        );
        assert_eq!(
            storage.checkpoints(&fixture.workspace.id).await.unwrap(),
            vec![fixture.checkpoint.clone()]
        );
    }
}
