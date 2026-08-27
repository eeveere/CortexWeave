PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS workspaces (
    id TEXT PRIMARY KEY NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(id, workspace_id)
);

CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    session_id TEXT,
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    details_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(id, workspace_id),
    FOREIGN KEY(session_id, workspace_id) REFERENCES sessions(id, workspace_id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS documents (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    language TEXT NOT NULL,
    analyzer_id TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    modified_at_ns INTEGER,
    indexed_at TEXT NOT NULL,
    UNIQUE(workspace_id, relative_path)
);

CREATE TABLE IF NOT EXISTS chunks (
    id TEXT PRIMARY KEY NOT NULL,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    stable_key TEXT NOT NULL,
    language TEXT NOT NULL,
    symbol TEXT,
    qualified_symbol TEXT,
    symbol_kind TEXT,
    start_byte INTEGER NOT NULL,
    end_byte INTEGER NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(document_id, stable_key)
);

CREATE TABLE IF NOT EXISTS embeddings (
    chunk_id TEXT PRIMARY KEY NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    dimension INTEGER NOT NULL,
    vector BLOB NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    session_id TEXT,
    task_id TEXT,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    FOREIGN KEY(session_id, workspace_id) REFERENCES sessions(id, workspace_id) ON DELETE RESTRICT,
    FOREIGN KEY(task_id, workspace_id) REFERENCES tasks(id, workspace_id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    session_id TEXT,
    task_id TEXT,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    FOREIGN KEY(session_id, workspace_id) REFERENCES sessions(id, workspace_id) ON DELETE RESTRICT,
    FOREIGN KEY(task_id, workspace_id) REFERENCES tasks(id, workspace_id) ON DELETE RESTRICT
);

CREATE VIRTUAL TABLE IF NOT EXISTS chunk_fts USING fts5(
    chunk_id UNINDEXED,
    content,
    symbol,
    qualified_symbol,
    relative_path,
    tokenize = "unicode61 tokenchars '_:'"
);

CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
    memory_id UNINDEXED,
    content,
    kind,
    tokenize = "unicode61 tokenchars '_:'"
);

CREATE INDEX IF NOT EXISTS idx_documents_workspace ON documents(workspace_id);
CREATE INDEX IF NOT EXISTS idx_chunks_document ON chunks(document_id);
CREATE INDEX IF NOT EXISTS idx_chunks_stable_key ON chunks(stable_key);
CREATE INDEX IF NOT EXISTS idx_embeddings_space ON embeddings(model, dimension);
CREATE INDEX IF NOT EXISTS idx_sessions_workspace_started ON sessions(workspace_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_status ON tasks(workspace_id, status);
CREATE INDEX IF NOT EXISTS idx_memories_workspace_created ON memories(workspace_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_events_workspace_created ON events(workspace_id, created_at DESC);

CREATE TRIGGER IF NOT EXISTS chunks_fts_insert AFTER INSERT ON chunks BEGIN
    INSERT INTO chunk_fts(chunk_id, content, symbol, qualified_symbol, relative_path)
    SELECT NEW.id, NEW.content, COALESCE(NEW.symbol, ''), COALESCE(NEW.qualified_symbol, ''), documents.relative_path
    FROM documents WHERE documents.id = NEW.document_id;
END;

CREATE TRIGGER IF NOT EXISTS chunks_fts_update AFTER UPDATE ON chunks BEGIN
    DELETE FROM chunk_fts WHERE chunk_id = OLD.id;
    INSERT INTO chunk_fts(chunk_id, content, symbol, qualified_symbol, relative_path)
    SELECT NEW.id, NEW.content, COALESCE(NEW.symbol, ''), COALESCE(NEW.qualified_symbol, ''), documents.relative_path
    FROM documents WHERE documents.id = NEW.document_id;
END;

CREATE TRIGGER IF NOT EXISTS chunks_fts_delete AFTER DELETE ON chunks BEGIN
    DELETE FROM chunk_fts WHERE chunk_id = OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memory_fts(memory_id, content, kind) VALUES (NEW.id, NEW.content, NEW.kind);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_update AFTER UPDATE ON memories BEGIN
    DELETE FROM memory_fts WHERE memory_id = OLD.id;
    INSERT INTO memory_fts(memory_id, content, kind) VALUES (NEW.id, NEW.content, NEW.kind);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
    DELETE FROM memory_fts WHERE memory_id = OLD.id;
END;
