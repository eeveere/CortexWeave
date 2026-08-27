# Breakpoint 14: Fresh-Context Demonstration

The repeatable `fresh_context_demo` test creates a mixed Rust, Python, and
TypeScript workspace and performs the Session A workflow:

1. Inspect the indexed mixed-language project.
2. Change `src/retry.rs`.
3. Record a compiler-result failure for an unknown retry-limit value.
4. Fix the Rust source and reconcile the changed chunk.
5. Record an observation, decision, and TODO with session, task, and related-file
   provenance.
6. Complete the task and end the session.

Session B reopens only the SQLite database with a new service instance. It uses
memory search to recover the work underway, the decision, relevant paths, and
remaining TODO. Semantic retrieval returns the fixed Rust source, then returns a
freshly edited version after one more incremental reindex.

Run the demonstration with:

```text
cargo test --test fresh_context_demo
```
