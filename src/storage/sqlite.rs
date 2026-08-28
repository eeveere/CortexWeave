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
    use sqlx::migrate::Migrate;

    use crate::domain::{
        CortexEvent, Document, EmbeddingRecord, EventType, MemoryKind, MemoryRecord, Session,
        StoredChunk, Task, Workspace,
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
    async fn v0_1_database_upgrades_without_losing_existing_data() {
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
            for migration in migrations.iter().take(2) {
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
        legacy.insert_document(&document).await.unwrap();
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
            Some(workspace)
        );
        assert_eq!(
            upgraded.get_session(&session.id).await.unwrap(),
            Some(session)
        );
        assert_eq!(upgraded.get_task(&task.id).await.unwrap(), Some(task));
        assert_eq!(
            upgraded
                .find_document(&document.workspace_id, &document.relative_path)
                .await
                .unwrap(),
            Some(document.clone())
        );
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
    }
}
