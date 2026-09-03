# Verified Experience Core

CortexWeave v0.5 adds a deliberately narrow historical-learning capability:
an explicit, bounded development episode can become an immutable, evidence-backed
Experience; a later session can retrieve it as historical supplemental context.

It is not an agent harness, memory auto-writer, autonomous planner, or source
of current truth. A caller or harness records factual Events and chooses when
to create, close, inspect, or review an episode. CortexWeave does not run tools,
infer a successful repair from a conversation, or claim that a recorded edit
caused a later verification result.

For the accepted release boundary, see [v0.5 Release Boundary](v0.5-release-boundary.md).
For exact transport arguments, see [CLI Reference](cli.md) and
[MCP Setup](mcp-setup.md).

## The verified path

```text
typed factual Events
        -> explicit, session-scoped episode
        -> normalized failure signature
        -> ordered attempts and scoped verification
        -> immutable Experience
        -> explainable bounded search
        -> historical supplement to current context
```

Each arrow is a separate, inspectable boundary. Events remain raw facts;
episodes organize them without changing them; consolidation creates a historical
interpretation only when registered rules support one; and current source,
current task state, and current deterministic observations remain more
authoritative than that interpretation.

## Events, episodes, and typed evidence

An Event is an append-only, workspace-scoped factual record. Its envelope
supplies the event identity, workspace, explicit session, optional task,
recorded time, contract, and version. Decoding a payload validates its bounded
shape and internal consistency. It does not authenticate its producer, turn a
tool exit code into task success, or establish that an observation is current
truth.

The registered v1 contracts are:

- `cortexweave.external_tool_completion`
- `cortexweave.rust_compiler_result`
- `cortexweave.cargo_test_result`
- `cortexweave.generic_verifier_result`
- `cortexweave.source_change_observation`
- `cortexweave.user_acceptance`

Payloads eligible for registered v1 evidence decoding are limited to 64 KiB.
Identifiers and free-form text are bounded, and paths must be workspace-relative
when a contract permits a path. Raw unknown, legacy, or oversized payloads may
remain inspectable Events, but are not consolidation evidence until a registered
decoder accepts them.

An episode belongs to exactly one workspace and one session, and may name one
task from that same scope. It starts `open`; a caller explicitly closes or
abandons it. Both terminal states are irreversible. A primary member Event must
match the episode's workspace, session, and task scope; it can belong to only
one primary episode. Membership order is caller-recorded ordinal order, not an
inference from timestamps.

Episode changes require an expected version and caller-supplied request key.
SQLite makes a repeated identical request idempotent and reports conflicting or
stale mutations rather than merging them. An episode contains at most 100
primary Events. CortexWeave does not automatically group nearby Events or close
an episode when a session ends.

## Signatures, attempts, and verification

Failure signatures are normalized, versioned equivalence classes, not raw error
text. A signature carries a domain, normalizer identity and version,
workspace-scoped scope, bounded stable components, a length-delimited canonical
encoding, and a domain-separated BLAKE3 key. It excludes transient locations,
free-form messages, commands, temporary paths, and environment fields.
Producer-owned stable identifiers such as subjects, targets, tests, and type
classes are not secret-scanned; producers must not place secrets in those
identity fields.

V1 normalizes supported Rust compiler failures, Cargo test failures, and
registered generic verifier failures. A Rust `E0308` needs structured paired
expected and actual type fields; CortexWeave never scrapes those values from
diagnostic prose. A normalizer may use a path only as declared scope and may
not invent a symbol from semantic similarity. Unsupported or ambiguous evidence
returns a typed no-result rather than a convenient guess.

The consolidator reads one terminal episode in its explicit member order. It
records attempts as action evidence followed by their next material result and
keeps the full sequence. `still_failing` means a following verifier observed
the same normalized failure; `verification_changed_failure` means a different
normalized failure; `verification_passed` requires compatible scoped verifier
evidence. These are historical associations, not causal explanations.

An Experience outcome is one of `success`, `failure`, `partial_success`,
`inconclusive`, or `abandoned`. A conclusive outcome requires registered,
subject-scoped verification or explicit user acceptance. A zero exit code,
tool completion, source-change observation, episode closure, or agent message
alone is not universal proof of success. Verification observations retain their
rule and rule version, subject, source Event, and status.

Evidence strength is structured rather than a floating-point confidence. It
records `strong`, `moderate`, `weak`, or `unsupported` plus visible bases such
as deterministic verification, repeated deterministic evidence, explicit user
acceptance, temporal association, or structural association. The strength
describes the stored interpretation's support; it does not prove present state
or that one attempt caused a result.

## Immutable Experience records

An accepted Experience is separate from both explicit Memory and raw Events.
It preserves the episode, session/task/workspace provenance, normalized
signature, ordered attempts, evidence links, outcome, verification assessment,
strength basis, deterministic extractor identity/version, renderer version,
and bounded code or graph snapshots.

Snapshots preserve historical meaning independently of mutable indexed rows.
They record source or graph revision, stable logical identity when available,
path, content hash, ranges, and resolution provenance. A current chunk or graph
UUID is not a historical identity. If v1 cannot support a requested historical
snapshot claim, consolidation returns a review-required proposal rather than
substituting current source contents.

Automatic consolidation is a two-step operation:

1. Preview computes a deterministic proposal or typed no-result.
2. If the proposal disposition is `automatic`, explicit acceptance supplies
   the preview's exact episode version, fingerprint, and proposal hash.

A `review_required` proposal is inspection-only in v0.5. It cannot be accepted,
and this release provides no review override or reviewed-acceptance API.

Acceptance recomputes and rechecks the terminal episode frontier inside one
SQLite transaction. A unique fingerprint converges identical retries on the
same immutable Experience; a mismatch is a conflict, never a second
interpretation. Acceptance does not update Events, episodes, memories, current
source, or graph state.

## Search, context, and history

Experience search is workspace-scoped, bounded, and explainable. It combines
eligible lifecycle/outcome state with deterministic failure-signature matching,
bounded lexical matching, and optional explicit path or graph filters. Exact
and compatible signature reasons are reported separately. Normal search returns
only active Experience; historical inspection is explicit and can expose an
excluded record together with its assessment trail.

Experience is a distinct context source, never a disguised Memory or current
source item. An optional canonical active failure signature can request it. The
context service selects ordinary current context first, then considers eligible
historical Experience from its separate pool and budget. Selected Experience is
labeled `historical_supplemental`, with lifecycle, outcome, verification,
strength, score, and selection reasons retained in packet explanation.

Current task, current source, and active Event evidence must remain in the
packet ahead of Experience. Historical content cannot replace them, reclaim
their budget, or assert that its old verification is still true. A compatible
Experience may originate at a different current path or symbol; explicit
caller-supplied path and graph filters remain exact. See
[Context Orchestration](context.md) and [Configuration](configuration.md) for
request and budget controls.

Review is append-only and idempotent by a caller-supplied request key. Reusing
that key with identical review content returns the original assessment; reusing
it with different content is a conflict. History is returned in bounded pages
with a continuation cursor. A reviewed `disputed`, `refuted`, or `superseded`
assessment excludes an Experience from ordinary search and context;
`confirmed` adds evidence but cannot reactivate a negative lifecycle. A
supersession names one replacement Experience and cannot form a cycle. Dispute
proposal is read-only: it may surface a compatible recurring failure, but it
does not change lifecycle or eligibility.

## Interfaces and operational limits

The native `CortexWeaveService` owns all episode, evidence, consolidation,
search, assessment, and context policy. CLI and MCP are thin adapters: they
resolve a workspace, validate bounded input, call the facade, and serialize the
result. They do not reimplement normalization, ranking, lifecycle, or
consolidation policy.

The public operations cover episode start, member association, close/abandon,
detail/listing; Experience preview, explicit acceptance, search, detail,
explanation/history, reviewed assessment, and read-only dispute proposal. CLI
and MCP use the same expected-version, request-key, fingerprint, proposal-hash,
and review-evidence requirements. Search and assessment history pages return
at most 50 records; an
Experience has at most 32 attempts, 256 evidence links, 64 snapshots of each
snapshot kind, and 64 assessment-evidence Events.

`doctor` checks the episode, Experience, assessment, and Experience FTS
projections plus foreign-key integrity. `metrics` reports persisted counts,
bounded consolidation/no-result and normalization observations, search and
context-category utilization, and process-local latency counters. These
operational signals are observational: they never tune ranking, budgets,
eligibility, or automatic action.

## What the acceptance experiment established

The release experiment created one verified Task A Experience and applied a
related compiler failure to fresh baseline and assisted Task B sessions. The
assisted packet selected the intended active verified Experience across a
different path as historical supplemental context, while keeping Task B's
current task, failure, and source first.

Both lanes repaired the simple mutation. The frozen measurement rule classified
the pair as narrowly helped because assisted used two compiler/test cycles and
baseline used three, but that difference was a discretionary post-verification
test rather than a demonstrated diagnostic advantage. The substantive result is
that the conservative retrieval and authority path behaved correctly; it is not
evidence of broad model improvement. Full controls, traces, and caveats are in
[the paired experiment result](experiments/v0.5-real-repository-paired-experiment-result.md).

## Deliberately deferred

v0.5 does not include automatic episode grouping, cross-session episodes,
semantic duplicate merging, automatic memory merging or promotion, automatic
pattern generalization, reuse-driven confidence updates, automatic retention or
deletion, workspace-to-global promotion, graph-driven experience
generalization, background consolidation, model-backed consolidation, or a
dedicated coding-agent harness. Future work must be evaluated against this
explicit, deterministic, provenance-preserving baseline.
