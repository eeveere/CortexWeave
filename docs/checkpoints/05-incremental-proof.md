# Checkpoint 05: Incremental Proof

**Status:** Passed

A mixed Rust, Python, and TypeScript workspace is indexed, edited, and reopened
from a new SQLite connection. Changing one Rust function and one TypeScript
function produces exactly two replacement embeddings. The unchanged Python file
short-circuits without analysis or embedding, and restart performs no redundant
embedding work.

Additional failure tests prove:

- deleting a file removes its document, chunks, vectors, and FTS rows;
- an embedding outage leaves the last committed document and chunks intact;
- changing embedding model and dimension re-embeds all affected chunks;
- compatible vectors are never silently mixed with the new query space.

