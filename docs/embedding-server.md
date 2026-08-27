# Embedding Server

CortexWeave requires an HTTP endpoint compatible with the OpenAI embeddings
request and response shape.

Request:

```json
{
  "model": "local-embedding-model",
  "input": ["first text", "second text"]
}
```

Response:

```json
{
  "model": "local-embedding-model",
  "data": [
    { "index": 0, "embedding": [0.1, 0.2] },
    { "index": 1, "embedding": [0.3, 0.4] }
  ]
}
```

The configured base URL and endpoint are joined before each request. Results may
arrive out of order, but every input index must appear exactly once. CortexWeave
rejects wrong counts, missing or duplicate indexes, empty vectors, mixed widths,
non-finite values, a reported model mismatch, and a configured dimension
mismatch.

Run this check after configuring the server:

```text
cortexweave --config cortexweave.toml doctor
```

`doctor` sends one small embedding request and reports the returned dimension.
Set `embedding.dimension` to that width for strict restart compatibility checks.

Set `embedding.limits.max_input_tokens` to the model's trained context and
`max_batch_tokens` to the server's aggregate request capacity. CortexWeave
packs requests by both item count and transformed token count. Oversized source
chunks are segmented before embedding; known provider capacity errors trigger
bounded splitting or repacking. Search queries are rejected with an actionable
error when they exceed the same input ceiling rather than being truncated.

The input and aggregate batch ceilings are independent. When the aggregate
batch ceiling is smaller than one configured input budget, CortexWeave segments
each document input to that smaller capacity before packing requests.

Embedding calls occur before SQLite reconciliation transactions. If the server
is offline or returns malformed data, the prior document, chunks, FTS rows, and
embeddings remain intact. Restore the server and run `reindex`, or let the
watcher process a later file change.
