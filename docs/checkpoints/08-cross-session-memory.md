# Checkpoint 08: Cross-Session Memory

**Status:** Passed

Recorded decision:

```text
Use BLAKE3 for deterministic change detection.
```

The service was shut down after recording the decision, then recreated against
the same SQLite file. A natural-language search for `Why are we using BLAKE3?`
returned the decision as a `Decision` memory.

The executable proof is
`service::cortex::tests::memory_search_survives_a_service_restart`.
