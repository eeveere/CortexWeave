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

## D045: Graph Symbols Have Logical Identity Independent of Embedding Chunks

**Status:** Accepted

Graph node UUIDs are durable storage identities reconciled by a workspace-scoped
logical key. Core-owned keys use reserved namespaces for the workspace, files,
and external dependencies. Analyzer-owned symbol keys are namespaced by analyzer
ID and the analyzer's pre-segmentation logical stable key. Analyzer version,
structural extractor version, content hashes, source offsets, persisted chunk
UUIDs, embedding segment ordinals, and embedding-space identity are provenance
or compatibility data; none of them participates in symbol identity.

Analyzers therefore emit symbols before language-neutral embedding segmentation.
An optional graph-node-to-chunk link is only a hydration aid and must be
consistent with the node's document. A symbol split into several embedding
chunks remains one graph node. Reconciliation preserves a node UUID when its
logical key remains present, even if its body, offsets, embedding segmentation,
or structural metadata changes. A path rename is a delete plus add in v0.4
because the shipped filesystem index has no durable file identity across paths;
CortexWeave must not claim continuity it cannot prove.

## D046: Source Documents Own Relationship Facts and Resolution Is a Projection

**Status:** Accepted

Every analyzer- or core-produced relationship fact is owned by the source
document that supplied its evidence. Its durable identity is the workspace,
source document, and a deterministic relationship key derived from the
normalized source symbol, relationship type, typed target, and duplicate
occurrence ordinal. This permits multiple call sites while preserving unchanged
facts across ordinary body edits. Deleting or reanalyzing a source document
removes or replaces only its owned facts.

The source fact and its raw typed target survive independently of resolution.
`SymbolResolver` deterministically projects a fact to a workspace-local graph
edge, an explicit external target, an ambiguous candidate set, or an unresolved
target. Deleting a target removes the resolved projection but not a still-valid
source fact, allowing it to become unresolved and later resolve again without
inventing evidence. A changed resolution target replaces the edge assertion;
an unchanged target preserves its edge UUID. Ambiguous and unresolved facts
never become edges, and semantic similarity never supplies a graph fact.

Facts and resolved edges retain analyzer ID and version, structural extractor
version, confidence, source document and segment, source content revision, and
normalized target provenance. Core-generated file/container facts follow the
same ownership and provenance rules.

## D047: Content Revisions and Graph Watermarks Are Transactional Workspace State

**Status:** Accepted

SQLite stores a monotonic `content_revision`, a `graph_content_revision`
watermark, and a graph schema version per workspace. Each document also records
the content revision of its last material source-index change. A successful
transaction increments `content_revision` exactly once when it adds or deletes
a document or changes persisted document/chunk identity, content, structure, or
segmentation. A no-op reconciliation, an embedding-only replacement, an event,
a memory or working-set mutation, and a graph-only structural-version rebuild do
not advance it. The new revision and source-index changes commit atomically;
rollback advances neither.

`graph_content_revision` is the greatest committed content snapshot that a
complete graph reconciliation batch acknowledges. A graph batch may coalesce
several content revisions, but it advances the watermark only after all dirty
documents and affected dependents since the prior watermark have been applied
atomically and a compare-and-set proves the workspace still has the source
snapshot the batch analyzed. An out-of-order or overtaken batch cannot mark the
graph current. A body-only edit may advance the watermark while preserving
unchanged node and edge rows; per-document graph state records that the newer
source revision was inspected.

Graph failure leaves the previous committed facts and watermark intact and
records the failed target revision. File-event timestamps and UUIDs remain
diagnostic facts, not revision authorities, and no event sequence is inferred.
Migrating a v0.3 workspace with documents initializes a nonzero content snapshot
and a zero graph watermark so the missing graph is visibly stale; an empty
workspace may begin at zero for both. Initial graph construction must reuse
compatible chunks and embeddings.

## D048: Structural Reads Are Snapshot-Labeled and Current-Only by Default

**Status:** Accepted

Every structural traversal, impact result, and context packet that uses graph
evidence carries a transport-neutral graph snapshot containing workspace content
revision, graph content revision, graph schema version, graph state, and any
applied limits. `Current` requires equal content watermarks, no active or failed
update for the current target, and a successful per-document analysis record for
the configured structural extractor version, including documents that produced
zero facts. `Updating`, `Stale`, and `Error` remain distinct observable states.

Structural service operations and context expansion require current graph state
by default. A caller may explicitly allow stale evidence; every returned path
and graph-derived context reason must then retain the stale snapshot label.
Context may instead omit structural expansion and report deterministic
degradation. Existing `ContextFreshness` continues to describe the selected
content itself and must not be used to disguise stale relationship evidence as
current content. No adapter may discard or reinterpret the graph snapshot.

## D049: Structural Extraction Is Separately Versioned and Capability-Gated

**Status:** Accepted

`LanguageAnalyzer` gains a normalized analysis result with logical chunks,
symbols, typed relationship facts, exact capability flags, and a structural
extractor version. Analyzer version continues to govern logical chunk
compatibility; segmentation identity continues to govern embedding chunks;
structural extractor version governs graph facts. Changing only the structural
version reruns graph analysis and resolution without replacing compatible chunks
or embeddings and without advancing the content revision.

A per-document graph-analysis record stores the applied analyzer and structural
versions even when an analyzer emits no symbols or relationships. A configured
version mismatch makes graph state stale until reconciled. Generic fallback
advertises no symbol or relationship capabilities and produces no structural
claims; graph core may still create the workspace-scoped file node. An analyzer
may emit symbols without calls, and adding a language or relationship capability
requires no language branch in graph persistence, resolution, retrieval, or
context code.

## D050: The Graph Is a Workspace-Isolated Derived Domain Behind Services

**Status:** Accepted

Language analyzers return parser-neutral facts. A graph indexer coordinates
source-owned reconciliation, a deterministic workspace-scoped resolver, and a
graph storage port. `StructuralService` owns bounded reads and impact operations;
context orchestration consumes that service rather than parser or SQL details.
`CortexWeaveService` remains the application facade, and CLI/MCP remain thin
adapters. Core graph code depends on neither MCP types nor language-specific AST
logic.

Graph nodes, relationship facts, and edges are a derived code domain, separate
from chunks and embeddings, explicit memories, append-only events, and transient
working-set state. Graph updates do not automatically mutate working sets, and
watcher events do not own graph consistency. SQLite owns graph transaction
boundaries and enforces workspace isolation with composite unique keys and
foreign keys for source documents, local endpoints, candidate endpoints, and
optional document/chunk links. Service filters alone are not sufficient. A
local edge or ambiguity candidate that crosses workspaces must fail at the
database boundary.

## D051: Symbol Resolution Uses Conservative, Observable Precedence

**Status:** Accepted

`SymbolResolver` operates only inside the requested workspace and returns one
of four explicit states: resolved, ambiguous, external, or unresolved. A local
resolution records the rule that produced it. Precedence is deterministic:
exact analyzer stable key; analyzer-supplied alias expansion; enclosing
container; exact qualified name; source-document simple name; unique workspace
simple name; and exact module or file identity. Candidates are sorted by stable
key and UUID before they are returned. Language supplied by the request, or by
its source node, is a hard structural boundary rather than an invitation to
guess across languages.

The resolver performs no fuzzy, case-folded, suffix, embedding, or semantic
matching. Multiple surviving candidates remain ambiguous, including duplicate
aliases and mixed local/external alias targets. An absent target remains
unresolved. A target becomes external only when the analyzer classified it as
external or the request supplied an explicit external module root; absence from
the local registry is not evidence of externality. An exact local module may
shadow a same-named external root, while aliases with multiple valid meanings
never silently pick that local meaning.

## D052: Document Symbol Registration Is Atomic and Collision-Intolerant

**Status:** Accepted

`SymbolRegistry` always materializes a core-owned file node and namespaces each
analyzer symbol by analyzer ID plus its pre-segmentation logical stable key. A
per-document registry reconciliation upserts current nodes and deletes stale
nodes in one SQLite transaction. A retained logical key preserves its graph
node UUID across source offsets, body content, analyzer metadata, and content
revision changes. Removing a key deletes the node and allows dependent edge
projections to disappear through database constraints; a later resolution then
reports unresolved rather than retaining a ghost target.

Workspace-wide stable keys are ownership claims. If two documents present the
same namespaced key, registration fails and the transaction rolls back instead
of moving the node to the second document. The lower-level graph-node upsert
enforces the same ownership check. This treats analyzer identity collisions as
integrity failures that must be fixed at extraction, rather than resolution
ambiguities that can be guessed around.

## D053: Relationship Extraction Emits Source Facts, Not Resolved Edges

**Status:** Accepted

Language analyzers emit normalized `AnalyzedRelationship` source facts only.
Every fact has a typed target, source range, deterministic relationship key,
confidence, and source symbol or core file-node key. The key is derived from
the source key, relation type, typed target, and duplicate occurrence ordinal;
the ordinal is ordered by source position only to distinguish otherwise equal
facts. Parsing produces no graph-node UUIDs, SQL rows, resolver decisions, or
cross-workspace edges.

All structured analyzers emit exact `Contains` and `DeclaredIn` facts. Imports
emit paired `Imports` and `DependsOn` facts from the source file to the exact
parser-derived module text. Export facts are emitted only where syntax or the
language contract proves them. This preserves analyzer pluggability: a language
can expose any subset of capabilities, and graph storage or resolution needs no
language-specific AST branch.

## D054: Structural Claims Prefer Direct Syntax Evidence Over Dispatch Guesses

**Status:** Accepted

Calls and references are emitted only for direct identifier call expressions;
member and receiver dispatch, reflection, dynamic attribute access, and other
indirect forms remain unclaimed. A direct call emits both `Calls` and
`References` facts at 0.9 confidence. Inheritance facts are emitted only from
syntax that distinguishes the relation: Python and JavaScript/TypeScript
`extends`, TypeScript `implements`, and Rust `impl Trait for Type`. C# base
lists emit `UsesType` at 0.9 confidence because extraction alone cannot safely
distinguish a base class from an interface. No analyzer advertises `Overrides`
until it can identify the overridden member without a guess.

Test nodes are explicit: Rust test attributes, Python `test_` functions,
C#/Go test conventions, and JavaScript/TypeScript `test` or `it` declarations
are represented as test symbols. `Tests` facts are currently derived only from
direct calls inside an explicit test and carry 0.9 confidence with
`likely/direct-call` provenance. They are useful association evidence, not proof
of behavioral coverage. Co-location, imports, naming resemblance, and semantic
similarity do not claim coverage.

## D055: Graph Reconciliation Is One Source-Snapshot Compare-and-Swap

**Status:** Accepted

After source chunks commit, the graph indexer prepares the complete structural
delta against an immutable in-memory workspace snapshot. SQLite then applies
the document's nodes, source-owned relationship facts, affected resolved and
unresolved projections, ambiguity candidates, per-document analysis state, and
workspace graph watermark in one transaction. The transaction first proves
that the workspace `content_revision` and graph-update ownership token identify
the snapshot the batch analyzed. The token also serializes graph-only rebuilds
that legitimately share one content revision. An overtaken batch returns
`Superseded` without writing and may be rebuilt against the newer snapshot; it
can never mark an older view current or overwrite a same-revision winner.

Document deletion is stronger than a follow-up cascade: deleting the source
document, advancing the content revision, reprojecting surviving dependents,
and acknowledging the graph revision occur in the same transaction. Any
storage error therefore rolls back both source deletion and graph changes.
For ordinary edits, source indexing may already be committed when graph
analysis fails, but the prior graph rows and watermark remain intact and the
current target is labeled `Error`. Retrying graph-only work reuses compatible
chunks and embeddings. `Current` is written only when every remaining document
has a successful analysis record matching its own content and analyzer identity
plus the structural version expected by the active analyzer registry; a
structural-version-only rebuild does not advance source revision.

## D056: Dependent Invalidation Follows Typed Resolution-Surface Changes

**Status:** Accepted

Every analyzer relationship remains a durable source-owned fact independent of
its current projection. Reconciliation always replaces and reprojects facts
owned by the changed document. Facts owned by other documents are re-resolved
only when the changed document alters a typed resolver-visible candidate:
stable or logical key, simple or qualified name, module/file identity,
language, or module-capable node type. Projections that target a removed node
are also invalidated directly. This covers additions that introduce ambiguity,
renames, type changes, and deletions without a workspace-wide resolution pass.

A body-only edit with an unchanged resolution surface updates the changed
document's analysis state but does not rewrite dependent edges. A resolved edge
keeps its UUID and creation time when its source, target, and relation type are
unchanged. A changed or missing target yields an explicit ambiguous, external,
or unresolved projection; ambiguity candidates retain workspace-enforced node
identities. No fuzzy or model-based invalidation is used.

## D057: Structural Reads Are Bounded Application-Service Operations

**Status:** Accepted

`StructuralService` owns symbol and path lookup, typed one-hop queries, bounded
multi-hop traversal, and impact analysis. Storage supplies workspace-scoped
node, edge, and source-chunk primitives but does not choose traversal intent;
adapters receive transport-neutral domain values and do not issue graph SQL.
Hard ceilings cap nodes, edges, and depth, while every result records the
effective limits and whether they truncated the answer.

Reads verify current graph state and configured analyzer structural versions by
default. Explicit stale reads retain the non-current graph revision. A revision
change during an operation rejects the result as retryable rather than labeling
facts from multiple graph snapshots as one answer.

## D058: Structural Retrieval Is Deterministic Evidence, Not Semantic Guessing

**Status:** Accepted

Hybrid retrieval has an independently configurable structural component. Exact
query symbols take precedence as graph seeds; baseline semantic and lexical
chunks seed the graph only when no exact symbol or path token matched. Query
phrases deterministically choose caller, callee, implementation, test,
dependency, dependent, general-neighborhood, or reverse-impact traversal.

A structural score is the product of path confidence, the most conservative
typed-relation multiplier on the path, and configured per-hop distance decay.
It is normalized with semantic and lexical components only after source chunks
are deduplicated. Every selected structural candidate keeps its seed, target,
typed path, graph revision, limits, and truncation state. A stale or failed graph
causes hybrid retrieval to omit this component; it does not relabel old edges as
current or prevent the baseline retrieval modes from answering.

## D059: Impact Is Reverse Dependency Reachability Without Diff Inference

**Status:** Accepted

Impact analysis starts from exact symbol nodes or a file node and walks incoming
calls, references, implementations, inheritance, type use, construction,
overrides, tests, imports, and dependencies. A file seed first expands its
contained declarations. Results are unique graph nodes ordered by distance,
confidence, and stable identity, with an explainable seed-to-impact path whose
confidence is the product of its edge confidences.

Context assembly uses the same structural paths to distinguish implementation,
usage, test, dependency, and transitive-impact reasons while retaining existing
token budgeting. v0.4 impact makes no claim about a git diff or runtime behavior;
diff seeding remains a separate change-source adapter and later roadmap work.

## D060: Graph Adapters Expose Exact IDs and Bounded Provenance

**Status:** Accepted

CLI and MCP expose the structural application service without graph SQL,
resolver policy, parser objects, or adapter-specific graph types. Callers first
resolve a symbol or path through `graph_find`, then use returned exact node IDs
for relation tools. Impact accepts an exact symbol or indexed relative path.
Each response is the original transport-neutral traversal or impact result,
including snapshot, limits, typed edges, paths, confidence, and truncation.

The CLI accepts the core service ceilings. MCP applies tighter request ceilings
of 100 nodes, 500 edges, and depth 4 before calling the service so one stdio
tool response remains operationally bounded. Both adapters default to
current-only graph reads and require an explicit stale-read opt-in. Neither
adapter creates graph facts, changes working sets, infers a diff, or maps raw
observations to nodes; those remain core indexing and deferred Phase 13 work.

## D061: Graph Health Is an Explicit Workspace Diagnostic

**Status:** Accepted

Workspace status carries a deterministic graph diagnostic alongside indexing
status: persisted graph revision, explicit current/stale/error state, total
nodes, resolved edges, unresolved relationship facts, and per-indexed-language
active analyzer identity and capability set. Counts are calculated from the
stored graph and source documents, not estimated from adapter output.

`graph status`, MCP `graph_status`, and `doctor` all obtain this one
application-service report. Diagnostic reads intentionally report a stale or
failed projection instead of applying the current-only rule used for structural
answers. `doctor` treats a registered non-current graph as unhealthy and shows
the workspace-level report, while no registered workspaces remains healthy.

## D062: Graph Performance Baselines Use Distributional Fixture Measurements

**Status:** Accepted

The deterministic in-repository benchmark fixture records initial integrated
index-and-graph build, incremental graph update, exact symbol lookup, one- and
two-hop traversal, reverse impact, graph-aware context assembly, and SQLite
growth for single-language and mixed-language workspaces. Query operations use
nine samples and report p50 and p95; averages alone are not an acceptance
signal. The fixture uses a local deterministic embedding provider, so results
isolate graph and storage overhead from network model inference.

These numbers are a reproducible regression baseline rather than a universal
latency promise: release qualification records the fixture size, hardware,
warm/cold state, p50/p95, database growth, and agreed regression budget before
comparison. Functional tests preserve incremental embedding reuse; timing
assertions are deliberately avoided in variable CI environments.

## D063: Analyzers Own Module-Path Semantics

**Status:** Accepted

Language analyzers own source-relative paths, extension candidates, package
indexes, namespace separators, import aliases, and explicit external-module
roots. They emit those possibilities as deterministic normalized relationship
metadata. The graph resolver consumes that transport-neutral metadata and
matches graph identities generically; it does not acquire Rust, Python,
JavaScript, TypeScript, C#, or Go path rules.

No candidate permits a best-effort guess. Zero matches remain unresolved,
multiple matches remain ambiguous, and external classification requires an
explicit analyzer-provided root. This keeps language-specific syntax and module
semantics out of the indexing core while allowing ordinary local imports to
resolve against canonical workspace paths.

## D064: Release Identity Is Diagnostic and the Upgrade Floor Is Explicit

**Status:** Accepted

The v0.4.1 operational release aligns the Cargo package version, binary
`--version` output, release checkpoint, and release notes on `0.4.1`. Earlier
roadmap labels that differed from the package manifest remain historical
metadata. Neither a roadmap label nor the Cargo package version may select a
migration, repair path, compatibility classification, or persisted-data rewrite.
Those decisions use validated migration history and the identities of the
affected persisted components.

The supported database floor is any intact SQLx-validated prefix of the bundled
migration history beginning at `0001_initial.sql`. The primary populated
pre-graph qualification state ends at `0004_memory_integrity.sql`; the graph
domain begins at `0005_graph_domain.sql`, with `0006_graph_repair_generations.sql`
adding durable graph-repair ownership and per-document projection manifests. A fixture ending at `0003` proves that
the current binary applies the real `0004`, `0005`, and `0006` transitions in one open,
while a populated `0001` fixture proves the entire supported chain. Missing,
modified, or newer migration history fails closed before application writes.

OPiHype and Crush are named v0.4.1 operational evaluation environments only.
They may supply release evidence, but core and application policy remain
workspace-generic and contain no branches for either client or repository.

## D065: Graph Repair Is Serialized In Place and Incomplete Generations Are Unreadable

**Status:** Accepted

v0.4.1 uses serialized in-place reprojection rather than a duplicate staged
graph. One durable workspace repair generation owns `if_needed` or `force` work
for one target content revision. The generation carries an unguessable owner
token, mode, target revision, lifecycle state, start and lease times, progress,
and bounded failure provenance. Per-document projection remains atomic under the
existing D055 compare-and-swap contract, while the repair generation remains
active across all document transactions. A new migration is justified because
the current watermark cannot distinguish active, abandoned, failed-partial, and
complete same-revision repair.

The repair planner records a per-document projection expectation sufficient to
detect missing projection rows, not only a compatible analysis-state row. The
exact schema is a Phase 1 decision, but final promotion must prove every indexed
document has the expected analysis identity and complete projection manifest for
the target revision. Only one transaction that still owns the generation may
promote the workspace to `Current` and complete the generation.

Current-only structural reads reject every active, failed-partial, or abandoned
repair generation. Explicit stale reads may use the last coherent projection
when the graph is stale or failed and no in-place generation has begun mutating
it. From repair lease acquisition until successful completion, explicit stale
reads also fail; after interruption or failure they remain unavailable because
the persisted graph may contain a mixed generation. Status, doctor, and repair
diagnostics remain readable throughout. Semantic and lexical retrieval continue,
while graph-aware retrieval omits structural evidence under the same rule.

This availability tradeoff is accepted for v0.4.1. It avoids graph-table
duplication and unbounded database growth while preserving revision truth. A
forced rebuild of a current graph therefore creates a bounded structural-read
outage. A later staged-generation design requires new evidence and a separate
decision; it is not implemented speculatively.

Phase 4 will make watcher, manual reindex, and explicit rebuild acquire the same
generation lease through the application service. A competing caller never writes around
the owner: it reports in-progress work or, after a winning generation completes,
that convergence was completed elsewhere. Lease expiry and takeover use a
durable compare-and-swap rule. A process restart does not clear an owner merely
because another process opened the database; only an expired lease can become an
explicit interrupted generation and be resumed or replaced.

## D066: Repair Ownership Is Fenced at Every Graph Mutation Boundary

**Status:** Accepted

Breakpoint 1 confirmed that generation identity alone is insufficient: lease
validity and incomplete-generation state must participate in every storage
compare-and-swap. Graph update entry, projection application, progress renewal,
failure recording, and final publication now require the matching active
generation, target content revision, and an unexpired lease. An expired owner
cannot renew itself, write, fail, or publish; a later acquisition atomically
marks it interrupted and installs a new unguessable owner.

Unowned graph writes are rejected while a workspace repair is active, failed,
or interrupted. They resume only after a generation completes or a new owner
replaces the incomplete generation. Source indexing remains authoritative and
may advance the content revision during repair; that revision change supersedes
the old repair before graph mutation or publication.

Structural evidence reads reject active, failed, and interrupted generations,
including callers that explicitly allow stale evidence. Operational workspace
and graph status bypass that evidence gate and remain readable so recovery can
be diagnosed. These fences live in storage and application services, not in CLI
or MCP adapters, and are the invariant Phase 4 integration must reuse.

## D067: Broad Source Reindexing Ends with One Fenced Graph Repair

**Status:** Accepted

Workspace reindexing and watcher batches reconcile source documents first,
including source deletions, and then invoke exactly one `IfNeeded` graph repair
generation. They do not publish a partial graph as each source file is handled.
This makes the generation lease from D066 the sole owner of broad graph
convergence while leaving incremental single-file reconciliation available for
callers that intentionally need it.

The public reindex outcome reports source work separately from the graph repair
outcome. An unchanged scan can therefore truthfully report zero updated files
and a completed graph reprojection, or an explicit current/no-op reason. This
preserves source revisions and compatible embeddings while making graph-only
recovery observable.

## D068: Exact Graph Seeds Are Evidence Without Invented Relationships

**Status:** Accepted

A graph-aware retrieval query may identify an exact declaration even when the
analyzer cannot prove a caller, callee, dependency, implementation, or likely
test edge. The exact seed is still structural evidence: it has a stable graph
node identity, source provenance, graph revision, and bounded read limits.
Retrieval may therefore return a zero-distance seed path and the code candidate
overlapping that node before relationship traversal.

A zero-distance seed must not be relabeled as a relationship. Direct graph
tools remain fact-only and may return an empty result for dynamic member calls
such as Python attribute dispatch. This gives context packets honest graph
provenance without weakening the conservative analyzer contract or claiming an
edge the graph does not contain.
