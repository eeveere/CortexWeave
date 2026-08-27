use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use notify::{
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind,
};
use serde_json::json;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::Instant,
};

use crate::{
    CortexError, Result,
    domain::{CortexEvent, EventType, Workspace},
    storage::SqliteStorage,
};

use super::IndexingService;

#[derive(Debug)]
struct PathSignal {
    path: PathBuf,
    event_type: EventType,
}

pub struct WorkspaceWatcher;

pub struct WorkspaceWatcherHandle {
    _watcher: RecommendedWatcher,
    shutdown: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl WorkspaceWatcherHandle {
    pub async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
    }
}

impl Drop for WorkspaceWatcherHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = &self.worker {
            worker.abort();
        }
    }
}

impl WorkspaceWatcher {
    pub async fn start(
        workspace: Workspace,
        indexing: Arc<IndexingService>,
        storage: Arc<SqliteStorage>,
        debounce: Duration,
        queue_capacity: usize,
    ) -> Result<WorkspaceWatcherHandle> {
        let root = PathBuf::from(&workspace.root_path);
        let (sender, receiver) = mpsc::channel(queue_capacity.max(1));
        let overflowed = Arc::new(AtomicBool::new(false));
        let callback_overflowed = Arc::clone(&overflowed);
        let callback_indexing = Arc::clone(&indexing);
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| match result {
                Ok(event) => {
                    let Some(event_type) = normalize_event_type(&event.kind) else {
                        return;
                    };
                    for path in event.paths {
                        callback_indexing.record_filesystem_events(1, 0);
                        if sender
                            .try_send(PathSignal {
                                path,
                                event_type: event_type.clone(),
                            })
                            .is_err()
                        {
                            callback_overflowed.store(true, Ordering::Release);
                        }
                    }
                }
                Err(error) => tracing::warn!(%error, "filesystem watcher error"),
            },
            Config::default(),
        )
        .map_err(|error| CortexError::Analysis(error.to_string()))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|error| CortexError::Analysis(error.to_string()))?;
        // Arm notifications before scanning so changes made during startup are
        // queued and reconciled after the scan.
        indexing.reindex_workspace(&workspace).await?;
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let worker = tokio::spawn(run_worker(
            workspace,
            indexing,
            storage,
            receiver,
            overflowed,
            debounce,
            shutdown_receiver,
        ));
        Ok(WorkspaceWatcherHandle {
            _watcher: watcher,
            shutdown: Some(shutdown_sender),
            worker: Some(worker),
        })
    }
}

async fn run_worker(
    workspace: Workspace,
    indexing: Arc<IndexingService>,
    storage: Arc<SqliteStorage>,
    mut receiver: mpsc::Receiver<PathSignal>,
    overflowed: Arc<AtomicBool>,
    debounce: Duration,
    mut shutdown: oneshot::Receiver<()>,
) {
    loop {
        let first = tokio::select! {
            signal = receiver.recv() => signal,
            _ = &mut shutdown => break,
        };
        let Some(first) = first else { break };
        let mut batch = HashMap::new();
        batch.insert(first.path, first.event_type);
        let sleep = tokio::time::sleep(debounce);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                signal = receiver.recv() => {
                    let Some(signal) = signal else { break };
                    batch.insert(signal.path, signal.event_type);
                    sleep.as_mut().reset(Instant::now() + debounce);
                }
                _ = &mut sleep => break,
                _ = &mut shutdown => return,
            }
        }
        indexing.record_filesystem_events(0, batch.len());
        if overflowed.swap(false, Ordering::AcqRel) {
            match indexing.reindex_workspace(&workspace).await {
                Ok(result) => {
                    if let Err(error) =
                        persist_rescan_event(&workspace, &storage, "queue_overflow", &result).await
                    {
                        tracing::error!(%error, workspace_id = %workspace.id, "overflow recovery event failed");
                    }
                }
                Err(error) => {
                    tracing::error!(%error, workspace_id = %workspace.id, "overflow recovery scan failed");
                }
            }
            continue;
        }
        if let Err(error) = process_batch(&workspace, &indexing, &storage, batch).await {
            tracing::error!(%error, workspace_id = %workspace.id, "watcher batch failed");
        }
    }
}

async fn process_batch(
    workspace: &Workspace,
    indexing: &IndexingService,
    storage: &SqliteStorage,
    batch: HashMap<PathBuf, EventType>,
) -> Result<()> {
    let root = Path::new(&workspace.root_path);
    let mut full_rescan = false;
    for (path, event_type) in batch {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative_string = relative.to_string_lossy().replace('\\', "/");
        if is_ignore_control_file(relative) || path.is_dir() {
            full_rescan = true;
            continue;
        }
        let known_document = storage
            .find_document(&workspace.id, &relative_string)
            .await?
            .is_some();
        if !known_document {
            // Missing unknown paths may be deleted directories, including names
            // with extensions. Existing unknown files must pass ignore-aware
            // discovery before becoming indexable.
            full_rescan = true;
            continue;
        }
        let result = match indexing.reconcile_file(workspace, &path, relative).await {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "file reconciliation failed");
                continue;
            }
        };
        let mut event = CortexEvent::new(
            &workspace.id,
            event_type,
            json!({
                "path": relative_string,
                "status": format!("{:?}", result.status).to_ascii_lowercase(),
                "added": result.added,
                "modified": result.modified,
                "removed": result.removed,
                "embedded": result.embedded,
            }),
        );
        event.created_at = chrono::Utc::now();
        if let Err(error) = storage.insert_event(&event).await {
            tracing::error!(%error, path = %path.display(), "watcher event persistence failed after reconciliation");
        }
    }
    if full_rescan {
        let result = indexing.reindex_workspace(workspace).await?;
        if let Err(error) =
            persist_rescan_event(workspace, storage, "ambiguous_path", &result).await
        {
            tracing::error!(%error, workspace_id = %workspace.id, "watcher rescan event persistence failed");
        }
    }
    Ok(())
}

async fn persist_rescan_event(
    workspace: &Workspace,
    storage: &SqliteStorage,
    reason: &str,
    result: &super::WorkspaceReindexOutcome,
) -> Result<()> {
    let event = CortexEvent::new(
        &workspace.id,
        EventType::FileModified,
        json!({
            "rescan": true,
            "reason": reason,
            "files_seen": result.files_seen,
            "files_updated": result.files_updated,
            "files_failed": result.files_failed,
            "files_removed": result.files_removed,
            "chunks_embedded": result.chunks_embedded,
        }),
    );
    storage.insert_event(&event).await
}

fn is_ignore_control_file(relative: &Path) -> bool {
    relative
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".gitignore" | ".ignore" | "exclude"))
}

fn normalize_event_type(kind: &EventKind) -> Option<EventType> {
    match kind {
        EventKind::Create(_) => Some(EventType::FileCreated),
        EventKind::Modify(ModifyKind::Name(_)) => Some(EventType::FileRenamed),
        EventKind::Modify(_) => Some(EventType::FileModified),
        EventKind::Remove(_) => Some(EventType::FileRemoved),
        EventKind::Any | EventKind::Other => Some(EventType::FileModified),
        EventKind::Access(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::{
        config::{GenericChunkConfig, LanguageConfig},
        embedding::provider::MockEmbeddingProvider,
        parsing::AnalyzerRegistry,
    };

    async fn wait_until(mut predicate: impl FnMut() -> bool) {
        for _ in 0..100 {
            if predicate() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("condition was not reached before timeout");
    }

    #[tokio::test]
    async fn rapid_save_rename_and_delete_converge_to_disk_state() {
        let directory = tempdir().unwrap();
        let original = directory.path().join("module.py");
        let renamed = directory.path().join("module.txt");
        fs::write(&original, "def value():\n    return 0\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "watch");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let indexer = Arc::new(IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("watch", 4)),
            Arc::new(AnalyzerRegistry::configured(
                &LanguageConfig::default(),
                &GenericChunkConfig::default(),
            )),
            1,
            1_024,
        ));
        let handle = WorkspaceWatcher::start(
            workspace.clone(),
            indexer,
            Arc::clone(&storage),
            Duration::from_millis(50),
            16,
        )
        .await
        .unwrap();

        for value in 1..10 {
            fs::write(&original, format!("def value():\n    return {value}\n")).unwrap();
        }
        fs::rename(&original, &renamed).unwrap();
        wait_until(|| renamed.exists()).await;
        for _ in 0..100 {
            let old = storage
                .find_document(&workspace.id, "module.py")
                .await
                .unwrap();
            let new = storage
                .find_document(&workspace.id, "module.txt")
                .await
                .unwrap();
            if old.is_none()
                && new
                    .as_ref()
                    .is_some_and(|document| document.language == "text")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            storage
                .find_document(&workspace.id, "module.py")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            storage
                .find_document(&workspace.id, "module.txt")
                .await
                .unwrap()
                .unwrap()
                .language,
            "text"
        );

        fs::remove_file(&renamed).unwrap();
        for _ in 0..100 {
            if storage
                .find_document(&workspace.id, "module.txt")
                .await
                .unwrap()
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            storage
                .find_document(&workspace.id, "module.txt")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !storage
                .recent_events(&workspace.id, 20)
                .await
                .unwrap()
                .is_empty()
        );
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn startup_scan_recovers_offline_changes_and_directory_deletion() {
        let directory = tempdir().unwrap();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let file = nested.join("lib.rs");
        fs::write(&file, "fn old() {}\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "offline");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));
        let first = Arc::new(IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("watch", 4)),
            Arc::clone(&analyzers),
            1,
            1_024,
        ));
        first.reindex_workspace(&workspace).await.unwrap();
        fs::remove_dir_all(&nested).unwrap();

        let restarted = Arc::new(IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("watch", 4)),
            analyzers,
            1,
            1_024,
        ));
        let handle = WorkspaceWatcher::start(
            workspace.clone(),
            restarted,
            Arc::clone(&storage),
            Duration::from_millis(20),
            4,
        )
        .await
        .unwrap();
        assert!(
            storage
                .list_documents(&workspace.id)
                .await
                .unwrap()
                .is_empty()
        );
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn analyzer_changing_renames_and_collision_match_final_state() {
        let directory = tempdir().unwrap();
        let python = directory.path().join("module.py");
        let text = directory.path().join("module.txt");
        let typescript = directory.path().join("view.ts");
        let tsx = directory.path().join("view.tsx");
        let rust = directory.path().join("old.rs");
        let moved_rust = directory.path().join("moved.rs");
        let collision_source = directory.path().join("source.rs");
        let collision_target = directory.path().join("collision.rs");
        fs::write(&python, "def value():\n    return 1\n").unwrap();
        fs::write(&typescript, "export function view() { return 1; }\n").unwrap();
        fs::write(&rust, "fn old_name() {}\n").unwrap();
        fs::write(&collision_source, "fn source_wins() {}\n").unwrap();
        fs::write(&collision_target, "fn replaced() {}\n").unwrap();

        let workspace = Workspace::new(directory.path().to_string_lossy(), "renames");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let indexer = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("watch", 4)),
            Arc::new(AnalyzerRegistry::configured(
                &LanguageConfig::default(),
                &GenericChunkConfig::default(),
            )),
            1,
            4_096,
        );
        indexer.reindex_workspace(&workspace).await.unwrap();

        fs::rename(&python, &text).unwrap();
        fs::rename(&typescript, &tsx).unwrap();
        fs::rename(&rust, &moved_rust).unwrap();
        let collision_content = fs::read(&collision_source).unwrap();
        fs::remove_file(&collision_source).unwrap();
        fs::write(&collision_target, collision_content).unwrap();
        let batch = [
            (&python, EventType::FileRenamed),
            (&text, EventType::FileRenamed),
            (&typescript, EventType::FileRenamed),
            (&tsx, EventType::FileRenamed),
            (&rust, EventType::FileRenamed),
            (&moved_rust, EventType::FileRenamed),
            (&collision_source, EventType::FileRenamed),
            (&collision_target, EventType::FileRenamed),
        ]
        .into_iter()
        .map(|(path, event_type)| (path.clone(), event_type))
        .collect();
        process_batch(&workspace, &indexer, &storage, batch)
            .await
            .unwrap();

        assert!(
            storage
                .find_document(&workspace.id, "module.py")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            storage
                .find_document(&workspace.id, "module.txt")
                .await
                .unwrap()
                .unwrap()
                .language,
            "text"
        );
        assert!(
            storage
                .find_document(&workspace.id, "view.ts")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            storage
                .find_document(&workspace.id, "view.tsx")
                .await
                .unwrap()
                .unwrap()
                .language,
            "typescript"
        );
        assert!(
            storage
                .find_document(&workspace.id, "old.rs")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            storage
                .find_document(&workspace.id, "moved.rs")
                .await
                .unwrap()
                .unwrap()
                .language,
            "rust"
        );
        assert!(
            storage
                .find_document(&workspace.id, "source.rs")
                .await
                .unwrap()
                .is_none()
        );
        let collision = storage
            .find_document(&workspace.id, "collision.rs")
            .await
            .unwrap()
            .unwrap();
        assert!(
            storage
                .list_chunks(&collision.id)
                .await
                .unwrap()
                .iter()
                .any(|chunk| chunk.symbol.as_deref() == Some("source_wins"))
        );
    }

    #[tokio::test]
    async fn dotted_directory_ignore_and_non_text_transitions_remove_stale_rows() {
        let directory = tempdir().unwrap();
        let dotted = directory.path().join("generated.v2");
        let ignored = directory.path().join("ignored");
        fs::create_dir(&dotted).unwrap();
        fs::create_dir(&ignored).unwrap();
        fs::write(directory.path().join(".gitignore"), "ignored/\n").unwrap();
        fs::write(dotted.join("nested.rs"), "fn nested() {}\n").unwrap();
        let binary = directory.path().join("binary.rs");
        let oversized = directory.path().join("large.rs");
        fs::write(&binary, "fn initially_text() {}\n").unwrap();
        fs::write(&oversized, "fn small() {}\n").unwrap();

        let workspace = Workspace::new(directory.path().to_string_lossy(), "filters");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let indexer = IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("watch", 4)),
            Arc::new(AnalyzerRegistry::configured(
                &LanguageConfig::default(),
                &GenericChunkConfig::default(),
            )),
            1,
            64,
        );
        indexer.reindex_workspace(&workspace).await.unwrap();

        fs::remove_dir_all(&dotted).unwrap();
        let ignored_file = ignored.join("secret.rs");
        fs::write(&ignored_file, "fn secret() {}\n").unwrap();
        fs::write(&binary, [1_u8, 0, 2]).unwrap();
        fs::write(&oversized, "x".repeat(128)).unwrap();
        let batch = [
            (dotted, EventType::FileRemoved),
            (ignored_file, EventType::FileModified),
            (binary, EventType::FileModified),
            (oversized, EventType::FileModified),
        ]
        .into_iter()
        .collect();
        process_batch(&workspace, &indexer, &storage, batch)
            .await
            .unwrap();

        for relative in [
            "generated.v2/nested.rs",
            "ignored/secret.rs",
            "binary.rs",
            "large.rs",
        ] {
            assert!(
                storage
                    .find_document(&workspace.id, relative)
                    .await
                    .unwrap()
                    .is_none(),
                "stale document remained for {relative}"
            );
        }
    }

    #[tokio::test]
    async fn formatter_replacement_under_queue_pressure_converges() {
        let directory = tempdir().unwrap();
        let target = directory.path().join("format.rs");
        fs::write(&target, "fn value() -> i32 { 0 }\n").unwrap();
        let workspace = Workspace::new(directory.path().to_string_lossy(), "formatter");
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        storage.insert_workspace(&workspace).await.unwrap();
        let indexer = Arc::new(IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("watch", 4)),
            Arc::new(AnalyzerRegistry::configured(
                &LanguageConfig::default(),
                &GenericChunkConfig::default(),
            )),
            1,
            1_024,
        ));
        let handle = WorkspaceWatcher::start(
            workspace.clone(),
            indexer,
            Arc::clone(&storage),
            Duration::from_millis(30),
            1,
        )
        .await
        .unwrap();

        for value in 1..20 {
            let temporary = directory.path().join(format!("format.rs.{value}.tmp"));
            fs::write(&temporary, format!("fn value() -> i32 {{ {value} }}\n")).unwrap();
            fs::remove_file(&target).unwrap();
            fs::rename(&temporary, &target).unwrap();
        }
        let final_hash = blake3::hash(&fs::read(&target).unwrap())
            .to_hex()
            .to_string();
        for _ in 0..120 {
            let converged = storage
                .find_document(&workspace.id, "format.rs")
                .await
                .unwrap()
                .is_some_and(|document| document.content_hash == final_hash);
            let no_temporary_rows = storage
                .list_documents(&workspace.id)
                .await
                .unwrap()
                .iter()
                .all(|document| !document.relative_path.ends_with(".tmp"));
            if converged && no_temporary_rows {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            storage
                .find_document(&workspace.id, "format.rs")
                .await
                .unwrap()
                .unwrap()
                .content_hash,
            final_hash
        );
        assert!(
            storage
                .list_documents(&workspace.id)
                .await
                .unwrap()
                .iter()
                .all(|document| !document.relative_path.ends_with(".tmp"))
        );
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn database_restart_recovers_offline_extension_change() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let database = directory.path().join("cortex.db");
        let original = root.join("state.ts");
        let renamed = root.join("state.tsx");
        fs::write(&original, "export function state() { return 1; }\n").unwrap();
        let workspace = Workspace::new(root.to_string_lossy(), "restart");
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &LanguageConfig::default(),
            &GenericChunkConfig::default(),
        ));

        {
            let storage = Arc::new(SqliteStorage::open(&database).await.unwrap());
            storage.insert_workspace(&workspace).await.unwrap();
            let indexer = Arc::new(IndexingService::new(
                Arc::clone(&storage),
                Arc::new(MockEmbeddingProvider::new("watch", 4)),
                Arc::clone(&analyzers),
                1,
                1_024,
            ));
            let handle = WorkspaceWatcher::start(
                workspace.clone(),
                indexer,
                storage,
                Duration::from_millis(20),
                8,
            )
            .await
            .unwrap();
            handle.shutdown().await;
        }

        fs::rename(&original, &renamed).unwrap();
        fs::write(&renamed, "export function state() { return 2; }\n").unwrap();

        let storage = Arc::new(SqliteStorage::open(&database).await.unwrap());
        let indexer = Arc::new(IndexingService::new(
            Arc::clone(&storage),
            Arc::new(MockEmbeddingProvider::new("watch", 4)),
            analyzers,
            1,
            1_024,
        ));
        let handle = WorkspaceWatcher::start(
            workspace.clone(),
            indexer,
            Arc::clone(&storage),
            Duration::from_millis(20),
            8,
        )
        .await
        .unwrap();
        assert!(
            storage
                .find_document(&workspace.id, "state.ts")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            storage
                .find_document(&workspace.id, "state.tsx")
                .await
                .unwrap()
                .unwrap()
                .language,
            "typescript"
        );
        handle.shutdown().await;
    }
}
