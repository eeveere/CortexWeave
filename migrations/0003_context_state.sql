CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_id_workspace
ON memories(id, workspace_id);

CREATE TABLE IF NOT EXISTS working_set_entries (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    task_id TEXT,
    source_id TEXT NOT NULL,
    source_type TEXT NOT NULL,
    activation_score REAL NOT NULL CHECK(activation_score >= 0.0),
    last_activated_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(session_id, workspace_id) REFERENCES sessions(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(task_id, workspace_id) REFERENCES tasks(id, workspace_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_working_set_scope_source
ON working_set_entries(session_id, IFNULL(task_id, ''), source_type, source_id);

CREATE INDEX IF NOT EXISTS idx_working_set_session_activation
ON working_set_entries(session_id, last_activated_at DESC);

CREATE TABLE IF NOT EXISTS context_pins (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    task_id TEXT,
    source_id TEXT NOT NULL,
    source_type TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(session_id, workspace_id) REFERENCES sessions(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(task_id, workspace_id) REFERENCES tasks(id, workspace_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_context_pins_scope_source
ON context_pins(session_id, IFNULL(task_id, ''), source_type, source_id);

CREATE INDEX IF NOT EXISTS idx_context_pins_session_created
ON context_pins(session_id, created_at DESC);

CREATE TABLE IF NOT EXISTS checkpoints (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    task_id TEXT,
    content TEXT NOT NULL,
    objective TEXT,
    completed_json TEXT NOT NULL DEFAULT '[]',
    decision_ids_json TEXT NOT NULL DEFAULT '[]',
    open_problems_json TEXT NOT NULL DEFAULT '[]',
    related_paths_json TEXT NOT NULL DEFAULT '[]',
    related_symbols_json TEXT NOT NULL DEFAULT '[]',
    next_action TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY(session_id, workspace_id) REFERENCES sessions(id, workspace_id) ON DELETE RESTRICT,
    FOREIGN KEY(task_id, workspace_id) REFERENCES tasks(id, workspace_id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_checkpoints_workspace_created
ON checkpoints(workspace_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_checkpoints_session_created
ON checkpoints(session_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_checkpoints_task_created
ON checkpoints(task_id, created_at DESC);

CREATE TABLE IF NOT EXISTS memory_supersession (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    superseded_memory_id TEXT PRIMARY KEY NOT NULL,
    superseding_memory_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK(superseded_memory_id <> superseding_memory_id),
    FOREIGN KEY(superseded_memory_id, workspace_id)
        REFERENCES memories(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(superseding_memory_id, workspace_id)
        REFERENCES memories(id, workspace_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_memory_supersession_workspace_newer
ON memory_supersession(workspace_id, superseding_memory_id);
