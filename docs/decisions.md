# CortexWeave v0.1 Decisions

## D001: Application Services Are the Integration Boundary

**Status:** Accepted

Adapters depend on a transport-neutral `CortexWeaveService`. Core crates and
modules have no dependency on MCP or CLI types. This keeps direct Rust, local
HTTP, socket, or harness integrations possible without reimplementing behavior.

## D002: SQLite Owns Persistence Details

**Status:** Accepted

SQLite with FTS5 is the initial durable store. SQL, migrations, cascades, vector
encoding, and FTS synchronization remain behind storage APIs. Reconciliation is
committed atomically and foreign keys are enabled on every connection.

## D003: Embeddings Are External and Self-Describing

**Status:** Accepted

CortexWeave calls an OpenAI-compatible embedding service through a provider
trait. Every vector stores model and dimension metadata. Retrieval rejects or
skips incompatible embedding spaces instead of silently comparing them.

## D004: Analyzers Produce a Normalized Contract

**Status:** Accepted

Each language analyzer privately owns parser-specific logic and emits normalized
chunks. Tree-sitter types cannot cross the trait boundary. The registry, not the
indexer, selects analyzers. Unsupported text uses a generic fallback.

## D005: Stable Identity and Content Change Are Separate

**Status:** Accepted

Analyzer-produced stable keys identify logical chunks; BLAKE3 content hashes
identify revisions. Analyzer ID and version participate in compatibility checks.
This permits unchanged embeddings to survive edits elsewhere in a file.

## D006: Filesystem State Wins Over Event Shape

**Status:** Accepted

Watcher events are hints. They are debounced and coalesced into paths, then the
reconciler reads final filesystem state. Rename, duplicate, and reordered events
therefore converge without encoding platform-specific event sequences in core.

## D007: Code, Memory, and Events Stay Distinct

**Status:** Accepted

Indexed workspace content follows source lifecycle, memories are explicitly
recorded durable knowledge, and events are append-only operational facts. They
may share workspace/session/task provenance but do not share lifecycle rules.

## D008: UUID Strings Cross Service Boundaries

**Status:** Accepted

Entities use UUIDs represented as strings in transport-neutral domain models.
Session and task IDs are optional on memories and events. This is portable across
SQLite, JSON, MCP, and future native protocols without transport coupling.

## D009: Bounded Work, No Global Application Mutex

**Status:** Accepted

The service uses a SQLite pool, bounded queues, and embedding concurrency limits.
Callbacks enqueue work; expensive analysis and HTTP calls occur outside locks and
database transactions. A rescan provides convergence after queue saturation.

## D010: MCP Is Delivered After Core Operations

**Status:** Accepted

MCP remains a v0.1 deliverable, but its tool definitions are translations over
already-tested service methods. MCP does not determine domain names, errors,
storage methods, or lifecycle behavior.

## D011: Go Method Identity Includes the Receiver Type

**Status:** Accepted

Go methods are siblings of their receiver type in the grammar tree, rather than
children of a type declaration. The Go analyzer derives `Type.Method` from the
receiver field before emitting its normalized chunk. This preserves distinct,
stable identities for methods with the same name on different types without
adding Go syntax to indexing or storage.

## D012: Source Revisions Gate Reconciliation Commits

**Status:** Accepted

Reconciliation serializes work for a workspace-relative path within a process
and rereads the source after analysis and embedding, immediately before the
SQLite transaction. A changed source rejects the stale attempt instead of
overwriting newer state. Workspace scans rescan before deletion and retain rows
for files whose discovery failed, so transient filesystem failures do not erase
durable knowledge.

## D013: Provenance Is Validated at the Service Boundary

**Status:** Accepted

SQLite continues to enforce workspace-local foreign keys. The transport-neutral
service additionally verifies that optional session and task provenance belong
to the requested workspace and agree with one another. Sessions are ended once,
tasks cannot begin on ended sessions, and terminal task states cannot move
backward.

## D014: MCP Initialization Precedes Watcher Bootstrap

**Status:** Accepted

The stdio adapter acknowledges a successful MCP `initialize` request before it
starts workspace watchers. Watcher startup, including its initial reconciliation,
runs in the background so a large registered workspace cannot exhaust an MCP
client's launch timeout. Watcher state reports `starting`, `ready`, or a startup
failure through the MCP workspace-status response.

## D015: Embedding Capacity Is an Explicit Provider Contract

**Status:** Accepted

Embedding providers expose request transformation, vector-space identity, token
counter identity and accuracy, per-input capacity, aggregate batch capacity,
item capacity, and reserved overhead through transport-neutral contracts.
Embedding requests are packed by both item count and transformed-token count.
Per-input and aggregate batch limits remain independent; the tighter aggregate
limit also constrains individual segment size when necessary. Known capacity
failures permit bounded corrective behavior; HTTP 413 is treated as capacity
evidence when a provider gives no more specific classification. Availability,
protocol, and malformed-response failures do not.

## D016: Segmentation Is Language-Neutral and Independently Versioned

**Status:** Accepted

Language analyzers emit logical chunks without provider concerns. A
language-neutral stage applies the provider's document transformation, counts
tokens, and splits only oversized chunks at deterministic UTF-8-safe source
boundaries. Its persisted identity is separate from analyzer version and
embedding-space identity, so changed limits, overlap, tokenizers, transformations,
or adaptive ceilings trigger an atomic document-tree replacement. No recovery
path truncates source or commits a partial replacement.

## D017: Workspace Selection Uses Deterministic Path Identity

**Status:** Accepted

Workspace registration stores a canonical display root when the filesystem can
resolve it and is idempotent by a separate normalized comparison key. Explicit
selectors resolve by UUID, unique exact or case-insensitive name, root path, or
decoded file URI. Paths inside nested workspaces select the longest matching
root. Default hints apply only when no explicit selector is supplied; otherwise
a single registered workspace is the only implicit fallback.

The resolver performs no fuzzy matching. Missing and ambiguous selections report
actionable candidates so adapters can expose the same transport-neutral behavior
without reproducing path or precedence rules.

## D018: Adapter Workspace Hints Never Register or Grant Access

**Status:** Accepted

Adapters may provide an explicit root path or file URI as a default selector.
The service resolves that hint only against existing registrations, after any
explicit tool selector and before singleton fallback. Hints never derive from
the server process's current directory, create registrations, expand registered
roots, or narrow watcher ownership. Missing and ambiguous hints remain visible
through `workspace_list` diagnostics.

## D019: SQLite Uses WAL for Local Read/Write Concurrency

**Status:** Accepted

File-backed SQLite databases use WAL mode, normal synchronization, foreign keys,
and a bounded busy timeout. Reconciliation still owns brief atomic transactions;
WAL reduces ordinary watcher-versus-tool contention without claiming safe
multi-process writer coordination.

## D020: Every Mutating MCP Operation Resolves a Workspace

**Status:** Accepted

The MCP adapter resolves a workspace before every mutation, including ending a
session. The application service verifies that a supplied session belongs to the
resolved workspace. This keeps legacy UUID selectors compatible while refusing
ambiguous or cross-workspace writes.

## D021: Context Orchestration Layers Above Retrieval

**Status:** Accepted

The v0.2 context orchestrator consumes transport-neutral retrieval and state
services. Retrieval does not depend on context assembly, and adapters only
translate `ContextRequest` and `ContextPacket` values. Working-set activation,
pins, checkpoints, and memory supersession remain separate persistence domains
so temporary attention, explicit priority, resume state, and durable knowledge
do not acquire the same lifecycle by accident.

## D022: Working-Set Decay Is Lazy and Pins Stay Explicit

**Status:** Accepted

Working-set rows store an activation baseline and its last activation time.
Inspection computes deterministic half-life decay from those values without
refreshing old rows, and removes stale unpinned entries. Repeated activation
uses a bounded optimistic update so concurrent observations merge without a
global mutex or a lost increment. Pins remain separate rows, are idempotent,
and protect matching entries from decay cleanup and bounded-retention eviction.

## D023: Temporal Retrieval Preserves Source Truth Before Recency

**Status:** Accepted

Temporal retrieval uses one normalized, workspace-scoped candidate stream for
code, documents, memories, events, task state, and session state. It applies
created, modified, recent-window, session, source-type, and supersession filters
before bounded selection. Its configurable half-life produces a standalone
recency score, but result order first prefers current source truth, then
historical evidence, and leaves superseded memory last. This prevents a recent
event or memory from displacing an older but current source representation.
Temporal retrieval does not infer semantic relevance; the candidate and ranking
pipeline must combine that independent signal with recency before final context
selection.

## D024: Checkpoints Are Explicit, Scoped Resume State

**Status:** Accepted

Checkpoints are created through `ContextService`, which validates the workspace,
active session, optional task, and every referenced decision before persistence.
The service owns their creation timestamp so checkpoint ordering is deterministic
under an injected clock. Workspace, session, and task reads return only the most
recent checkpoint in that valid scope; a missing checkpoint is distinct from a
missing or cross-workspace scope. Checkpoints remain structured resume state,
not generic context sources or implicit memories.

## D025: Candidate Pools Merge Evidence by Stable Source Identity

**Status:** Accepted

The context candidate pool is an intermediate, bounded collection rather than a
final context packet. It merges direct hybrid code retrieval, temporal evidence,
active task and session state, and hydrated working-set references by
`(source_type, source_id)`. Merging preserves component scores and selection
reasons instead of duplicating material. A deterministic retention order applies
the configurable candidate limit before later token-budget selection. Active
task references and explicit pins survive that bound in priority order, so a
small pool cannot discard required state before assembly sees it.

## D026: Structural Expansion Uses Analyzer-Normalized Relationships

**Status:** Accepted

Structured analyzers persist optional normalized parent stable keys, container
symbols, structural depth, and sibling ordinals with every logical code chunk.
SQLite resolves a small, deterministic set of enclosing, neighboring, and
directly related chunks from that data and source ranges. `ContextService` consumes only those
relations and never branches on a programming language, parser node kind, or
AST type. Structural additions remain ordinary, bounded candidates with an
explicit selection reason, so later ranking and token budgeting retain final
control. Analyzer version increments force existing structured documents to be
reindexed before the metadata is relied upon.

## D027: Context Ranking Is Deterministic and Component-Visible

**Status:** Accepted

Candidate ranking uses a configurable weighted average of normalized semantic,
lexical, active-task, working-set, recency, provenance, freshness, and
structural-relationship components. Every component and the final score remains
on the candidate for diagnostics and evaluation. Missing relevance components
contribute zero rather than changing the denominator, which keeps scores
comparable across source types. Stable source identity breaks exact ties. Query
text uses existing deterministic full-text retrieval for memories so historical
decisions can carry relevance evidence instead of relying on recency alone.

## D028: Context Packets Use Shared Token Accounting and Bounded Selection

**Status:** Accepted

Context assembly consumes the embedding provider's existing `TokenCounter`
through a small application-service adapter, rather than defining another token
estimator. This keeps context packet estimates in the same model and dimension
accounting space used for embedding capacity. Tests use the conservative byte
counter explicitly for deterministic expectations.

The selector applies configurable soft category allocations before a greedy
score-per-token fill pass. Active task references and explicit pins are required
first and are retained before the bounded candidate-pool cut; when they do not
fit, only a UTF-8-safe prefix that fits the remaining total budget is retained.
Ordinary oversized candidates are omitted, and code contents that contain or
are contained by an already selected chunk from the same path are excluded.
Empty content retains any tokenizer special-token overhead. The emitted total
is always the sum of selected item estimates and never intentionally exceeds
the request budget.

## D029: Semantic Context Is a Facade Operation with Thin Adapters

**Status:** Accepted

`CortexWeaveService::semantic_context` accepts the transport-neutral
`ContextRequest` and returns its `ContextPacket` unchanged from
`ContextService`. The CLI maps positional workspace and query input plus flags
into that request, and MCP maps its schema fields after resolving a workspace.
Neither adapter ranks candidates, applies budgets, or interprets packet
contents. MCP alone caps a requested context budget at 65,536 tokens to bound a
single stdio response; this transport safeguard does not enter the core API.

## D030: Per-Token Selection Preserves Direct Query Evidence

**Status:** Accepted

Candidate selection uses normalized direct semantic and lexical evidence in
addition to the final ranked score before dividing by token cost. The final
score remains the primary cross-source diagnostic, while the direct-evidence
term prevents a tiny, query-independent document path from displacing the
current implementation that actually matched the request. This is deterministic
and affects only the budget selector; it neither changes retrieval scores nor
creates a model-based reranking stage.

## D031: Resume Separates Current Session from Evidence Session

**Status:** Accepted

`resume_context` resolves and reports a selected session and task using explicit,
deterministic precedence. A fresh active session may differ from the session that
owns the selected task or latest checkpoint, so the result also exposes an
evidence session. Checkpoints, scoped memories, prior working sets, and recent
change windows use that evidence session without pretending it is the caller's
current session. A selected task never falls through to a checkpoint owned by a
different task; only a taskless session checkpoint is a valid fallback.

The prompt-ready result remains a bounded `ContextPacket`. Task state, a
deterministically rendered checkpoint, scoped decisions and failures, merged
working sets, aggregated path changes, and current source enter one candidate
pipeline with explicit reasons. File events are grouped by path; model inference
is not used to summarize changes or decide whether failures were resolved.
Structured scope choices and checkpoint data remain outside the packet as
diagnostics so callers can inspect exactly how resume state was chosen. Change
aggregates separately report session-scoped and unscoped watcher event counts.

## D032: Resume Reuses the Context Candidate and Packet Pipeline

**Status:** Accepted

`ContextService::resume_context` resolves resume-specific evidence but converts
it into ordinary context candidates before ranking and token-bounded packet
assembly. The task receives a deterministic structured renderer, and the
checkpoint is represented as `other:checkpoint` with required priority directly
after the task. Decisions, open failures, build failures, working-set evidence,
aggregated changes, and hydrated source retain their normal source types and
selection reasons. This preserves one scoring and UTF-8-safe truncation path
instead of creating a second prompt builder.

SQLite retains raw events and memories. Resume-specific repository methods are
bounded, workspace-scoped reads; file-change aggregation remains in the service
so raw provenance is never replaced by a model-written summary. Superseded
failure memories are excluded by the existing supersession relation, while
compiler and test failures remain evidence rather than inferred task state.

## D033: Context Transports Stay Thin and Schema-Bounded

**Status:** Accepted

The CLI and MCP expose semantic context, resume context, working-set inspection,
pin management, and checkpoint operations as direct mappings to
`CortexWeaveService`. They resolve workspace selectors, parse transport values,
and serialize service results, but do not select evidence, rank candidates, or
apply resume policy. MCP bounds a single requested context budget at 65,536
tokens; the core request remains transport-neutral. This preserves a single
behavioral contract for programmatic, CLI, and MCP callers.

## D034: Explanations Describe Packets After Selection

**Status:** Accepted

Context explanations are opt-in diagnostics generated from the final selected
items. They report source identity, path or symbol, selection reasons,
component scores, token estimate, and truncation state, but omit duplicate
prompt content. Explanation is attached after packet assembly and is outside
the packet token budget, so enabling it cannot affect retrieval, ranking,
selection, or truncation. CLI and MCP expose the option as a transport flag;
the core carries it as request metadata only.

## D035: Context Evaluation Operates on Final Packets

**Status:** Accepted

The evaluation harness measures final `ContextPacket` and resume outcomes,
rather than reimplementing candidate selection. It records relevance recall,
precision, MRR, token utilization, duplicate-token ratio, current-source path
coverage, resume-task accuracy, and selection latency. Expected source IDs,
paths, and task IDs are explicit fixture inputs, making each scenario
deterministic and suitable for regression tests. The harness remains
transport-neutral and can score CLI, MCP, or direct application-service output.

## D036: Context Documentation Has One Architectural Entry Point

**Status:** Accepted

The v0.2 context model is documented in one architecture guide covering source
assembly, working-set and temporal evidence, token budgeting, checkpoints and
resume, explanation, evaluation, and transport interfaces. CLI, MCP, and
configuration references retain their command and option detail, while the
architecture guide explains the invariants and dependency direction. This keeps
the intended core-versus-adapter boundary visible to future harness work.

## D037: A Dedicated Harness Uses Only the Application Facade

**Status:** Accepted

A dedicated agent harness can run a full reasoning-cycle context loop through
`CortexWeaveService` without MCP semantics. Requests, packets, events, sessions,
tasks, working-set entries, pins, and checkpoints are transport-neutral domain
types. The facade exposes lifecycle mutation, context assembly, feedback
activation, checkpointing, and resume directly. MCP performs only workspace
resolution, argument parsing, response bounding, and serialization.

## D038: Final Hardening Targets Scope, Supersession, and Bounds

**Status:** Accepted

The final adversarial review accepts four v0.2 fixes: exclude superseded direct
memory candidates, constrain task/session state candidates to explicit context
scope, cap configurable candidate expansion, and reject explicit supersession
cycles. Cross-session durable memory remains intentional and must not be removed
by state scoping. Trust policy, overlap-aware duplicate detection, and automatic
contradiction inference are deferred because they require new semantics rather
than corrections to the v0.2 contract.

## D039: Explicit Context State Is Scoped While Durable Evidence Remains Broad

**Status:** Accepted

Candidate-pool temporal retrieval filters only task and session state when a
request supplies an explicit task or session. The effective session is the
explicit session when present, otherwise the selected task's owning session.
An explicit task accepts only that task state; session state accepts only the
effective session. Memories, events, code, and documents remain workspace-wide
evidence, preserving intentional cross-session recall without allowing unrelated
operational state to consume the requested packet.

Superseded memory is excluded from direct lexical candidates after hydration,
and storage rejects supersession cycles transactionally with a recursive query.
Context configuration also bounds candidate pools at 10,000 and structural
expansion at 64, retaining conservative defaults while preventing pathological
growth.

## D040: Native Harnesses Enforce Evidence Policy

**Status:** Accepted

Context packets are bounded evidence, not a mechanism for enforcing an external
agent's behavior. MCP clients may select unintended tools or read source outside
a packet despite prompt instructions. The v0.3 native harness therefore owns an
explicit context cycle and policy for selected hydration or audited overrides.
CortexWeave continues to own transport-neutral retrieval, provenance,
explanations, working state, memories, events, and checkpoints; it does not
execute tools or become an agent runner.

Analyzer readiness is part of the same contract. A supported language disabled
by configuration falls back to generic chunking, so readiness reporting must
make that state and the cost of rebuilding it visible without silently changing
configuration. The OPiHype implementation-coverage scenarios are the initial
acceptance fixtures for this direction.

## D041: Analyzer Readiness Is Read-Only Capability Comparison

**Status:** Accepted

The analyzer registry retains metadata for bundled analyzers independently from
the configured analyzer selection. Workspace readiness compares an ignore-aware
filesystem scan, configured selection, bundled availability, and persisted
document analyzer identity. It reports supported and unsupported generic
fallback separately, along with document, chunk, and embedding replacement
counts for the current configuration and recommended enablement.

Readiness never changes `LanguageConfig` and never invokes reconciliation.
Analyzer identity or version drift requires an explicit reindex, while generic
fallback for a format with no bundled analyzer remains a ready, deterministic
state.

## D042: Implementation Coverage Requires Exact Final-Packet Evidence

**Status:** Accepted

Implementation-coverage fixtures are named and declare exact expected source
paths and normalized symbols. An expectation is satisfied only by a code item in
the final bounded packet with both values. Other selected items that contain the
path or symbol are retained as mention-only diagnostics and cannot satisfy the
expectation.

Coverage results remain separate from recall, precision, MRR, token,
duplication, freshness, resume, and latency metrics. This makes failures
actionable without redefining the existing context-quality measurements or
reimplementing packet selection in the evaluator.

## D043: Native Hydration Is Policy-Gated and Auditable

**Status:** Accepted

The direct harness contract requires an active workspace, session, and task and
always returns a bounded packet with explanation. Selected-source audit records
repeat workspace, source type, path, symbol, and scores so a harness does not
need transport-specific reconstruction. A caller-provided policy evaluates
sufficiency; CortexWeave supplies the types but does not decide the policy.

Exact hydration accepts packet-selected code IDs. An out-of-packet ID requires a
non-empty harness override reason or is rejected before source is returned. An
accepted override records a workspace/session/task-scoped factual event with
the reason and durable chunk/path/symbol provenance. Because the item bypassed
selection, its score provenance is explicitly `out_of_packet_not_scored` rather
than a fabricated score.

## D044: Imported Memory Requires Explicit Trust and Consolidation Review

**Status:** Accepted

Memory origin and trust are separate typed fields. Existing and newly
human-authorized records are trusted; imports begin unreviewed, require source
segments, remain visible for inspection, and are excluded from every automatic
context path until reviewed. Trust changes append an immutable review record in
the same SQLite transaction as the state update.

Consolidation is deterministic and read-only. It reports normalized token and
source-segment overlap, recognizes contradiction only from equal structured
claim keys with unequal values, and may propose an older-to-newer supersession
when the newer record is trusted. Applying supersession remains a distinct,
reviewed operation with a reviewer and reason; cycle prevention remains inside
the storage transaction. This adds integrity semantics without introducing a
reasoning model or allowing analysis to mutate durable memory.
