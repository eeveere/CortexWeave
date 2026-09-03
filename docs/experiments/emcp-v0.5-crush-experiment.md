# emCP v0.5 Crush experiment

## Protocol, revision 1

Date: 2026-09-02. Subject: `C:/dev/agentic/emCP`, as selected by the user.

Question: can the shipped CortexWeave v0.5 MCP surface retain a verified
TypeScript/Vitest repair and supply it to a fresh Crush session?

Before spending model calls on a paired repair, run the evidence eligibility
gate below. A failed gate is an integration finding, not a Helped/Harmed result.
Do not substitute ordinary Memory for Experience or label Vitest as Cargo.

### Isolation and evidence

- Copy tracked emCP files and its local `AGENTS.md` into a new disposable
  workspace. Copy installed dependencies; do not modify the original checkout.
- Use the existing release binary, recording its version and SHA-256, and a
  dedicated SQLite database. Preserve raw command results and MCP messages.
- This first gate tests typed evidence and consolidation only. Exclude source
  indexing to avoid embedding work before verifier eligibility is established.
  It makes no assertion about retrieval quality or analyzer coverage.
- Keep actual test reports outside the disposable source workspace. Validate
  test counts and assertion outcomes, rather than inferring success from an
  exit code alone.

### Controlled Task A

Use the existing `tests/unit/hybrid-retrieval.test.ts` suite and the stable
sequence-key tie-break in `src/core/retrieval/fusion.ts`.

1. Verify the unmodified suite passes.
2. Reverse the tie-break from `left.key - right.key` to `right.key - left.key`;
   require the existing ordering assertion to fail.
3. Attempt to remove the tie-break entirely; require the assertion to remain
   failing. Record this as a controlled operator attempt, not model behavior.
4. Restore the exact original source bytes and require the suite to pass.
5. In a dedicated MCP session, record the three test observations as truthful
   `cortexweave.generic_verifier_result` v1 Events declaring `vitest.run` v1,
   tool `vitest`, operation `run`, and the exact test-file subject. That declared
   identifier does not claim a production verifier is registered.
6. Interleave the two actual edits as external editor-completion Events, then
   associate the five Events with one explicit debugging episode and close it.
7. Preview consolidation. Only an automatic, successful proposal with supported
   verification may advance to acceptance and a paired fresh-session test.

### Stop rule

If preview reports an unsupported verifier or no supported failure, preserve
the negative result and do not dispatch scored Crush lanes. Record the missing
integration and the completed test/evidence checks. No comparative performance
label is valid without an eligible Experience and a fresh controlled pair.

If the gate passes, seal a separate Task B protocol before dispatching models;
fix model, prompt, initial source, ordinary context, verification requirements,
and database fork, with Experience inclusion as the intended difference.

## Run 01 result

**Outcome: evidence gate blocked; no paired model result.** The controlled
repair and MCP probe completed on 2026-09-02. Zero Crush model lanes were
dispatched, because there was no eligible Experience for the assisted lane.

| Source state | Tests passed | Tests failed | Exit code |
| --- | ---: | ---: | ---: |
| Original emCP source | 7 | 0 | 0 |
| Reversed sequence-key tie-break | 6 | 1 | 1 |
| Attempt: remove explicit tie-break | 6 | 1 | 1 |
| Repair: restore original tie-break | 7 | 0 | 0 |

Both failures were the existing test named `reciprocal-rank fusion promotes
candidates supported by both retrievers and preserves stable sequence ties`.
These are operator-controlled source states; the experiment measured no model
diagnosis, edit iterations, or failed-approach avoidance.

The shipped `cortexweave 0.5.0` binary accepted five session-scoped Events,
associated them with one debugging episode, and closed the episode at version
2 through MCP. Consolidation preview returned:

```json
{
  "kind": "no_result",
  "reason": "no_supported_failure",
  "diagnostics": [{
    "code": "unregistered_verifier_rule",
    "membership_ordinal": 0,
    "message": "generic verifier evidence has no exact registered rule"
  }]
}
```

Experience search returned an empty list and the dedicated database contained
zero Experiences. SQLite integrity check returned `ok`. File hashes confirmed
that the original emCP source was unchanged and the disposable source was
restored to its initial bytes.

### Interpretation

This is a verified integration limitation for this Vitest evidence path. The
generic verifier contract can represent the reports, but the production rule
registry does not register `vitest.run`. Its standard generic rule set currently
contains only `cargo.check`; Rust compiler and Cargo test evidence also have
their dedicated built-in handling. See
[`src/service/failure.rs`](../../src/service/failure.rs) and
[`src/service/consolidation.rs`](../../src/service/consolidation.rs).

The refusal is consistent with the v0.5 evidence policy. It prevents an
unregistered producer from becoming a verified historical result. It is not a
Crush tool-selection failure: this gate exercised MCP directly, without a Crush
model session. Code search, explicit memory, and other CortexWeave capabilities
were not evaluated here.

The next prerequisite for this experiment is a registered Vitest verification
rule and an adapter that validates structured Vitest results, actual executed
tests, failed assertion identity, invocation scope, and exit consistency. A
follow-up should also specify whether compatibility means the same assertion,
test file, or broader component before scoring retrieval. Installing a rule
alone must not turn every nonzero Vitest invocation into the same failure.
Any such implementation would require its own design and tests, followed by a
new sealed experiment revision. No production code was changed in this run.

### Artifacts and reproduction

- Subject commit: `e527fdaaa2f6c7d5447a55998fccfb90048c0842`.
- Binary SHA-256: `ce78563a388b8343eff0ace629c23e206cea2eddc3f1d9daf04c0bbecf0b39da`.
- Driver SHA-256: `1d372b17b9dfd6b7cf74e02463ccf7618b38b59e67abb5993df84d24442d24a4`.
- Database SHA-256 after shutdown/checkpoint: `cebd78e0bb347d73945d9e5353b6ad81dec04ae30625a5f80dd2c1206ea2539c`.
- Episode: `1b42c8bb-7401-4f5d-8672-3b2e06c728b6`.
- Runner: `.cortexweave/experiments/emcp/run_experiment.py`.
- Raw artifacts: `.cortexweave/experiments/emcp/run-01/artifacts/`.
- Machine-readable result: `artifacts/result.json` within that run.

The raw artifacts include source manifests, test command stdout/stderr and
structured Vitest reports, each experimental source state, the complete MCP
request/response trace, episode records, consolidation preview, and final
source checks. They are retained locally under the ignored experiment folder;
this report is the repository-visible record.

To repeat the evidence gate, choose a new, nonexistent run directory:

```powershell
python .cortexweave/experiments/emcp/run_experiment.py `
  --source C:/dev/agentic/emCP `
  --binary C:/dev/CortexWeave/target/release/cortexweave.exe `
  --run C:/dev/CortexWeave/.cortexweave/experiments/emcp/run-02
```

The runner requires installed emCP dependencies, copies them into the disposable
workspace, and refuses to overwrite an existing run. It uses a separate database
and does not require an embedding service for this evidence gate.
