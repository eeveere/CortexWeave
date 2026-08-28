# Resume Context Design

## Contract

`resume_context` is a transport-neutral `ContextService` operation. Its request
contains only:

```text
workspace_id
session_id?
task_id?
token_budget
```

The result is a `ResumeContext` envelope with:

```text
selected_session?
session_selection
selected_task?
task_selection
evidence_session_id?
checkpoint?
recent_changes[]
working_sets[]
packet: ContextPacket
```

The envelope exposes selection and provenance for inspection. `packet` is the
prompt-ready, token-bounded material. Envelope diagnostics are not implied to be
inside that token budget.

## Scope Resolution

Explicit input always wins and is validated against the workspace.

Session selection is deterministic:

1. An explicit session.
2. The session associated with an explicit task, when one exists.
3. The most recently started active session.
4. The most recently ended session, ordered by `ended_at`.
5. No session.

The result reports `explicit`, `task_association`, `latest_active`,
`latest_ended`, or `none_available`. Exact ID breaks timestamp ties.

Task selection is deterministic:

1. An explicit task.
2. The most recently updated active task in the selected session.
3. The most recently updated incomplete task in the selected session.
4. When the session was inferred rather than explicit, the most recently
   updated active task in the workspace.
5. When the session was inferred rather than explicit, the most recently
   updated incomplete task in the workspace.
6. No task.

Incomplete means `pending` or `active`; terminal tasks are not silently resumed.
An explicit session constrains inferred tasks to that session. If explicit
session and task IDs disagree, the request fails instead of combining scopes.
The result reports the exact task-selection rule used.

The selected session is the current interaction scope. The evidence session is
the checkpoint's session when a checkpoint exists, otherwise the selected
task's session, otherwise the selected session. This matters when a fresh
Session B resumes a task and checkpoint from ended Session A.

## Evidence Order

Resume candidates are assembled in this order of importance:

1. Selected task state.
2. Latest checkpoint for the selected task. If none exists, only a taskless
   checkpoint from its evidence session may be used. Without a selected task,
   use the latest checkpoint from the evidence session, then the workspace.
3. Unsuperseded decisions explicitly referenced by the checkpoint.
4. Recent unsuperseded decisions and failure memories in the task or evidence
   session scope.
5. Recent compiler and test failure evidence in that scope.
6. Pins and decayed working-set entries from the selected and evidence sessions.
7. Aggregated file changes during the evidence session.
8. Current source referenced by the checkpoint, memories, working set, or
   aggregated changes.

Candidates retain ordinary scores and selection reasons. The selected task is
required first. A deterministically rendered checkpoint is required second and
uses `other:checkpoint` as its source type plus a new `current_checkpoint`
selection reason. Pins retain their existing required priority. Required items
may be truncated only by the existing bounded UTF-8-safe mechanism.

The task renderer includes title, status, structured details, and update time in
a fixed order; resume does not reuse the temporal task candidate that contains
only a title. The checkpoint renderer includes its human-readable content, objective,
completed work, open problems, related paths and symbols, and next action in a
fixed field order. It performs no model synthesis. The structured checkpoint is
also returned in the diagnostic envelope.

## Recent Changes

Recent file events are aggregated in application logic; raw events remain
unchanged in SQLite. Only path-bearing `file_created`, `file_modified`,
`file_removed`, and `file_renamed` events participate.

For an evidence session, the aggregation window is its start through its end,
or the current service clock for an active session. Events explicitly scoped to
another session are excluded. Unscoped watcher events within the time window are
eligible because watcher events do not currently carry session IDs.

Events are grouped by normalized root-relative path and produce:

```text
path
change_count
session_scoped_count
unscoped_count
change_kinds
first_changed_at
last_changed_at
currently_present
```

Paths and change kinds have stable lexical ordering. Rescan events without a
path are omitted, preventing low-level event spam. Existing files seed current
document and code candidates; removed files retain only the aggregate. Separate
scoped and unscoped counts make the watcher-attribution limitation visible when
sessions overlap.

An unsuperseded `failure` memory is treated as open. A compiler or test event is
reported as recent evidence, not automatically declared open or resolved. A
later success event can explain that a failure was fixed, but v0.2 does not infer
semantic contradiction or resolution without explicit memory supersession or
checkpoint state.

## Current Source

Current source paths come from checkpoint paths, decision/failure memory
metadata, working-set code or document entries, and recent-change aggregates.
Storage resolves exact current documents and their normalized chunks. Related
symbols select matching normalized symbols first; remaining chunks retain source
order and the global candidate bound. Structural expansion continues to consume
analyzer-normalized relationships and contains no language-specific branches.

This path-first hydration is deterministic. An optional future semantic query is
not needed to make transcript-free resume work.

## Persistence

No new domain table is required. Implementation adds bounded repository queries
for session selection, task selection, scoped resume memories/events, and exact
path hydration. A migration may add lookup indexes for ended-session ordering,
task status plus update time, and session-scoped event time; it must not rewrite
existing rows.

## Implementation Tests

The Terra implementation should cover:

- explicit, active, latest-ended, and absent session selection;
- explicit, session-scoped, workspace fallback, and absent task selection;
- explicit session/task scope conflicts;
- Session B using Session A as its evidence session;
- checkpoint-first structured rendering and token enforcement;
- superseded versus open failure memories;
- deterministic path aggregation without raw event spam;
- removed paths and current source hydration;
- bounded working-set merge across current and evidence sessions;
- a transcript-free Session A to Session B integration test explaining what,
  why, changed files, remaining work, and next action.
