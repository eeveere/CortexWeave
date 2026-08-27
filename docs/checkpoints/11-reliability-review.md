# Breakpoint 11: Reliability Review

The reliability suite verifies that the last committed document, chunks, FTS
rows, and embeddings survive failures. Covered cases are embedding outage,
SQLite exclusive lock, a foreign-key failure after transaction mutations,
process restart, model change, same-model dimension change, analyzer failure,
unsupported-language generic fallback, and one-file failure during workspace
indexing.

Workspace reindex now reports `files_failed` and continues with other files.
SQLite reconciliation remains atomic. A configured embedding dimension is part
of compatibility checks and malformed or changed-width responses are rejected.

Result: all reliability tests pass, including persisted file-database restart
and lock scenarios.
