# Checkpoint 02: Schema Review

**Status:** Passed

## Review Findings

- Stable chunk identity is unique within its owning document, while documents
  are unique by workspace and relative path. Content hashes remain independent
  from stable identity.
- A document tree can be persisted in one transaction. A duplicate chunk key or
  invalid embedding rolls back the document and all preceding chunks.
- Embeddings store model, dimension, vector bytes, and creation time. The schema
  supports model changes without assuming a fixed dimension.
- Documents retain language, analyzer ID, and analyzer version; chunks retain
  normalized and language-specific metadata.
- Events and memories have independent tables and lifecycle rules.
- Sessions and tasks are workspace-scoped. Composite foreign keys prevent a
  task, memory, or event from referencing provenance in another workspace.
- Session/task rows are retained as provenance and ended or completed in place.
  Workspace deletion remains the owning cascade.
- FTS rows are synchronized by database triggers, including cascaded deletion.
- IDs and JSON payloads remain transport-neutral for a future native harness.

## Verification

Nine tests pass, including restart persistence, cascade deletion, transaction
rollback, stable-key uniqueness, workspace separation, session/task association,
event and memory persistence, vector round-trip, and language provenance.

