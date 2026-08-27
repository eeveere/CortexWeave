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
the configurable candidate limit before later ranking, structural expansion, and
token-budget selection.
