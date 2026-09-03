# Shuttle — First Dedicated Harness Plan

Status: planning revision 4, 2026-09-02. Product requirements below are confirmed
by the user; detailed implementation choices remain proposals unless explicitly
marked otherwise. Implementation belongs in a separate repository. This document
is temporarily kept with CortexWeave for architectural reference.

Revision 4 incorporates the requested review changes: snapshot-bound acceptance,
recovery before live writes, explicit episode capacity handling, separate
historical references and hydration candidates, deterministic progress rules,
an earlier complete Qwen task, and development/held-out evaluation separation.

Working product name: **Shuttle**, from the user's textile-themed candidates
Shuttle and CortexShuttle. CortexShuttle remains an alternate repository/package
name; name availability and the final repository location have not been decided.

## 1. Confirmed product direction

- A daily-use local coding agent that also supports controlled research and
  evaluation using the same execution engine.
- Initially one local user; eventual open-source release.
- A separate repository with its own release version.
- Rust with Ratatui is acceptable provided it delivers the Crush-inspired
  aesthetic and interaction quality. The ashy purple terminal design is a
  product requirement, with an early visual and interaction review gate.
- Local inference through llama.cpp at `http://localhost:8080`.
- Initial reference model: `unsloth/Qwen3.5-4B-GGUF:Q4_K_M`, as specified by
  the user's launch command, superseding the earlier informal Q4_0 description.
- Worker launch configuration requests 175,104 context tokens on port 8080,
  Flash Attention, `q8_0` K cache, `iq4_nl` V cache and GPU offload.
- Dedicated `nomic-embed-text-v1.5` Q8_0 embedding server on port 8081, requesting
  8,192 context tokens, 4,096 batch/microbatch tokens and GPU offload.
- Reference hardware: RTX 2060 Mobile, 6 GB VRAM, 32 GB system RAM.
- Windows and Linux are the two priority release platforms.
- Selectable autonomy levels, with tool permissions following the selected level.
- Both existing-checkout and Git-worktree operation.
- Shared verified historical Experience between worktrees of the same local
  repository is required in the first release.
- One active task at a time.
- Completion combines an explicit verification plan with user confirmation.
- Automatic CortexWeave session, task, Episode, Event, and eligible Experience
  lifecycle management.
- Initial qualification covers investigation, a small feature, a failing-test
  repair, and fresh-session reuse of prior verified experience.
- Multi-agent delegation, browser automation, background task scheduling, IDE
  integration, and a separate orchestration layer are deferred.
- Growth of knowledge across different projects is a future CortexWeave goal;
  the first sharing boundary is linked worktrees of one repository.

The user's last exclusion said "model"; this draft interprets it as model
routing. One configured local reasoning model per run is the working default.
The two launch configurations are supplied. Effective runtime capacity and GPU
residency, exact executable/model/template identities, and final repository
location remain qualification inputs. No further general setup questionnaire is
required to complete this planning pass.

## 2. Proposed implementation boundary

Use the accepted Rust/Ratatui direction for the engine and terminal. This permits direct
use of CortexWeave's existing Rust service contract in one local process.
Ratatui supplies terminal UI components; the suggested layout and palette are
our own design. [Ratatui documentation](https://ratatui.rs/).

Depend on a pinned CortexWeave revision in the separate repository. A local
path override can support development. Establish the first supported revision
after any required substrate changes are verified; the inspected checkout
contains substantial uncommitted work and is not a reproducible release pin.

```text
terminal chat              headless evaluation runner
      \                         /
       \                       /
             harness engine
     task state / policy / verification / journal
           |          |                |
    llama.cpp     tool executor    CortexWeave adapter
      :8080       files/processes      service facade
                                      |
                               CortexWeave storage,
                               indexing, context,
                               evidence, Experience
```

The harness owns inference, action selection, process execution, authorization,
verification planning, and user interaction. CortexWeave owns its established
domains, ranking, evidence validation, consolidation rules, and transactions.

Use a small model-provider interface with one llama.cpp implementation and a
scripted test implementation. Build no routing engine or provider catalog.
Keep the terminal independent of the engine so headless evaluations exercise
the same behavior and future interfaces have an application boundary to call.

Keep harness transcripts, run state, tool invocation receipts, and evaluation
artifacts in harness-owned storage. CortexWeave retains its own database and
domain records. The adapter uses public application operations, with no direct
SQL or dependency on MCP types. Cross-database operations use durable delivery
records and reconciliation; there is no shared atomic transaction with a shell
process or model request.

## 3. First usable task cycle

1. Open the selected checkout or worktree, inspect its current changes, resolve
   its CortexWeave identity, and check analyzer/index readiness.
2. Create or recover the run's session and task. Capture the user's objective,
   constraints, chosen autonomy, and workspace mode.
3. Prepare a short working plan and explicit verification plan. Persist their
   revisions, including user changes and approved execution scope.
4. Assemble model context, request a response, and validate any tool call.
5. Apply the selected permissions, execute an authorized action, and record the
   actual result and source changes.
6. Refresh affected evidence and task state; repeat within configured limits.
7. Run the declared checks against an identified source snapshot and verification-
   plan revision. Record passed, failed, skipped, waived, and unverified requirements.
8. Present that snapshot's diff and verification receipts. Enter
   `awaiting_user_acceptance`; relevant changes invalidate the offer.
9. Revalidate the offer when the user accepts. Persist the acceptance receipt,
   then enter `finalizing`. Rejection or requested changes returns to the loop.
10. Deliver factual Events, close eligible episodes, preview consolidation, and
    accept only proposals that CortexWeave marks automatic. Record each disposition
    before completing the task through the recoverable finalization sequence.

Verification status, user acceptance, and task status are separate fields.
A model's final message cannot complete the task. A passing command cannot
stand in for checking the whole verification plan. Acceptance with a waived
check records the waiver and remaining limitation rather than a passing result.

Use explicit engine states for planning, gathering context, generating,
waiting for permission, executing, verifying, awaiting acceptance, finalizing,
paused, completed, cancelled, and failed. Persist state transitions and their reasons.
User interruption and changes of direction must be handled at safe action
boundaries, with in-flight effects reported accurately.

### 3.1 Verification and acceptance bind to exact state

Persist these versioned records in harness-owned SQLite storage:

| Record | Required identity and meaning |
| --- | --- |
| Verification plan | Revision, requirement/check IDs, executable and arguments, working directory, relevant environment/runtime identity, required scope, and explicit waivers |
| Source snapshot | Workspace/root identity, Git starting state where available, content hashes for declared tracked and untracked inputs, staged/unstaged changes, diff identity, and versioned input/exclusion manifest |
| Verification receipt | Actual run/action ID, check ID, plan revision, input snapshot, producer/runtime identity, observed status and immutable artifact references |
| Acceptance offer and receipt | Offer ID, objective/plan revisions, reviewed snapshot and diff, verification receipt IDs, visible limitations/waivers, and the actual user's response |

Verification inputs include relevant test definitions, runner configuration,
dependency manifests and lockfiles, not just edited production files or Git HEAD.
Declare generated-output exclusions before running checks. Do not retroactively
exclude a changed input to preserve a pass. Hash inputs before and after a run,
track observed intervening mutations, and reject uncertain input stability.
This detects ordinary concurrent changes; it is not an atomic filesystem snapshot
or isolation guarantee. Controlled evaluation requires isolated, frozen inputs.

| Change or event | Required transition |
| --- | --- |
| Relevant source, test, configuration, or dependency input changes during verification or while awaiting acceptance | Mark affected receipts stale, invalidate the offer, refresh evidence, and reverify |
| Objective, required scope, check definition, or waiver changes | Issue a new plan/offer revision; previous receipts cannot silently satisfy changed requirements |
| User accepts a stale offer or current hashes cannot be established | Do not complete; show the mismatch and obtain fresh verification and acceptance |
| User accepts the current offer with an explicit waiver | Record the limitation as waived, never passed |
| Source changes after a valid acceptance | Preserve the historical receipt; do not represent it as acceptance of the new state |

The engine rechecks current identity at acceptance and performs no further source
writes for that completed objective. External edits during finalization leave the
accepted snapshot historical and make the current checkout visibly diverged.
Completion describes the accepted snapshot, not an unqualified claim about files
that may subsequently change. Test edit-during-check, edit-while-awaiting, changed
test definitions, stale offers after restart, and explicit waiver paths.

### 3.2 Durable progress and bounded control

Maintain a compact task ledger separate from both the full transcript and durable
CortexWeave knowledge. It records objective/plan revisions, constraints, current
checkpoint, attempted changes with result references, current failures, verified
facts versus hypotheses, unresolved questions, next needed evidence, remaining
budgets, and pending effects. Rebuild every prompt from this ledger and current
evidence; context compaction and restart must not erase failed attempts. Model
summaries are neither verifier facts nor automatic Memory/Experience.

The harness journal is authoritative for controller phases, action state, budgets,
and acceptance offers. CortexWeave remains authoritative for its domain records.
Project a running/paused/verifying/awaiting task as its existing active task state;
do not add orchestration states to the substrate merely to mirror the UI.

Start with configurable development limits, then freeze qualified values per
evaluation protocol: 64 model responses, 60 minutes of active run time, two
consecutive malformed-call repair responses, and at most two safe pre-dispatch
transport retries. Exclude user/permission waiting time. Persist counters and
elapsed active time across restart; only the user can extend a run budget.

Detect stalls deterministically using normalized tool/argument identity, input
snapshot, result fingerprint, and verification state. Three redundant actions on
unchanged inputs/results, or six decision turns with no new evidence, source
change, or verification progress, trigger one bounded replan. A repeated stall
after that replan pauses for direction. Invalid-call exhaustion also pauses.
New prose, IDs, or reordered equivalent arguments do not count as progress.
These runtime fingerprints do not claim CortexWeave exact failure recurrence.

The replan must state the unsuccessful approach and next evidence to obtain in
the task ledger. All actions still consume global budgets. Required verification
and genuine episode re-anchoring runs may repeat a command under an explicit
controller purpose, but do not reset stall counters merely by getting a new ID.
Budget exhaustion pauses with a checkpoint and remaining uncertainty; it is
neither successful completion nor an invented failed verifier result.

## 4. Context policy

Compose and budget a model request before every model invocation. Retrieving new
CortexWeave evidence is a separate operation, governed by freshness triggers.
This policy is an evaluation hypothesis, not a claim of established superiority.

The model receives the active objective and constraints, verification plan,
current task progress, a small recent interaction window, the current tool
exchange, and CortexWeave-selected evidence. The full chat remains available to
the user without being sent in full at every cycle. Preserve valid tool-call
and tool-result pairing whenever constructing a continuation request.

| Trigger | Evidence refresh rule |
| --- | --- |
| Objective/query, verification scope, or requested evidence changes | Prepare a new bounded packet for the revised request |
| Relevant source/index, verifier/Event, working-set/pin, trust, or Experience state changes | Invalidate affected evidence and refresh before the next decision using it |
| Repository membership changes, workspace switch, or resume | Revalidate scope and freshness; do not trust a persisted packet without validation |
| No dependency change, such as repairing malformed tool arguments | Recompose the request; reuse selected evidence only with a valid freshness key |

The freshness key covers scope/membership revision, query and selection policy,
source/index revision, task and relevant Event state, working set/pins, Memory
trust and Experience lifecycle. Time-dependent ranking needs an explicit validity
window as well. If the facade cannot establish an adequate key, refresh instead
of guessing. Begin with this conservative fallback, not a new caching subsystem.

Pass a canonical active failure signature when registered normalization produces
one. Clear or supersede it when scoped current verification resolves or changes
the failure. Historical Experience remains supplemental to current task, source,
and verification evidence. Ledger updates and tool-call/result pairing apply on
every request even when the selected packet is reused.

Hydrate selected sources when exact implementation details are needed. Out-of-
packet reads require an engine-authorized reason and provenance; the configured
policy may authorize ordinary relevant reads without asking the user every
time. Unindexed and ignored files still require explicit path/read policy.
Never describe packet selection as an operating-system access boundary.

Recheck source hashes before editing. Watcher activity alone is not proof that
indexing has caught up. Wait for relevant reconciliation or explicitly use a
fresh, provenance-recorded file read while marking stale graph/index evidence.

Budget the complete reasoning-model request, including instructions, tool
schemas, chat-template overhead, tool results, and output reserve. CortexWeave
currently accounts with its embedding provider's token counter; that estimate
cannot prove the request fits a different reasoning tokenizer. Qualify a count
of the actual serialized prompt with the installed llama.cpp version, or use
a conservative bounded fallback and report the counting method. The server
documents template and tokenization endpoints, but exact tool-template coverage
must be checked against the qualified build. [llama.cpp server documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md).

Keep four separate limits explicit: the worker's configured server capacity
(`-c 175104`), its effective per-request capacity, Shuttle's chosen complete
request budget, and CortexWeave's evidence-packet budget. The current substrate
caps a context packet at 65,536 tokens under its own counting contract. A large
worker context does not authorize passing 175,104 as that packet budget. The
full prompt can also contain bounded harness-owned state and authorized source
reads, all counted with the reasoning model and with space reserved for output.

Maintain a stable instructions/tool-schema prefix where possible and measure
cached versus newly evaluated prompt tokens. Refreshing current evidence still
takes priority over cache reuse. Support larger user-selected budgets when the
qualified server capacity permits; the initial default is selected empirically.

Packet freshness does not replace source-hash preconditions for edits or the
verification/acceptance identity checks in section 3.1. Record refresh reasons,
freshness keys, cache hits and prompt-evaluation measurements before introducing
more elaborate reuse policies.

## 5. Tools and autonomy

The first registry should cover bounded file listing, text search, file/range
reads, patch application and file creation, command execution, Git inspection
and authorized operations, structured verification, CortexWeave retrieval,
and asking the user a question. Package installation and ordinary network
commands can use the executor under the chosen permission scope. Browser
automation and a general plugin platform remain deferred.

Expose a compact, stable tool vocabulary. Validate arguments and resolve paths
before execution. Tool output must have bounded model-visible rendering, with
larger output stored as a local artifact and truncation made visible. Tool
adapters produce facts; model-authored summaries are not verifier observations.

Proposed profiles, with exact defaults still reviewable:

| Profile | Automatic actions | Actions requiring a user grant |
| --- | --- | --- |
| Inspect | Bounded approved-source reads, search, and retrieval | Writes and process execution |
| Confirm | Reads and retrieval | Edits and commands, with reusable scoped grants |
| Workspace | Workspace edits and explicitly configured build/test commands | General commands and effects outside those grants |
| Trusted | Tools covered by the user's declared run scope | Any expansion of that scope |

Propose `Workspace` as the daily-use default. Allow changes during a task;
apply them to future actions and revalidate queued actions. Make the active
profile visible. The model cannot change its own permissions. A command that
runs repository scripts is code execution even if named "test" or "check".

Permission policy and process containment need separate, accurate descriptions.
An unrestricted host shell is not confined by setting its current directory.
Qualify process behavior on both Windows and Linux, including timeouts,
cancellation, descendants, path aliases/symlinks, and terminal restoration. Do not claim sandbox
guarantees without an actual tested containment mechanism.

Use exact patch preconditions and show edit conflicts instead of overwriting
intervening changes. Treat repository instructions and tool output as data with
explicit trust boundaries; they cannot grant additional permissions.

## 6. Checkouts and worktrees

Offer both modes and remember the user's preference per project. Opening a
project initially uses the selected checkout; an explicit worktree action
creates or opens an isolated working directory. This remains a proposed default.
Record the initial dirty state and the chosen Git starting state; preserve
pre-existing changes throughout the task. Shared history works in both modes.

A worktree has its own canonical root and, under the current contract, its own
CortexWeave workspace identity. Register and index that root separately. Do not
reuse the main checkout's source identity or rewrite historical provenance.
Retain worktrees and their history until explicit cleanup; worktree removal is
not a substitute for accepting a task's result.

### 6.1 Repository identity and membership

Cross-worktree Experience sharing is a required substrate prerequisite. Add a
durable repository identity above existing workspace identities, with explicit
workspace membership and a versioned sharing policy owned by CortexWeave.
SQLite owns registration, membership changes, and their consistency rules.

A trusted Git integration discovers the canonical common Git directory and
linked worktree roots. Git documents both a common directory and per-worktree
state; use that relationship as the local membership evidence. Store a stable
repository ID with its observed location rather than deriving permanent identity
from a branch name or raw path hash. [Git worktree documentation](https://git-scm.com/docs/git-worktree),
[Git path discovery](https://git-scm.com/docs/git-rev-parse).

Normalize native paths on each OS and control Git environment overrides during
discovery. A matching remote URL, directory name, commit, or copied checkout
does not enroll an independent clone, fork, submodule, or unrelated project.
Moves and repaired Git metadata require membership revalidation. Record how
pre-existing workspaces were linked; do not retroactively invent historical
repository membership. Historical search eligibility follows the current
explicitly authorized group while original evidence ownership remains intact.

Related worktrees must use the same configured CortexWeave service/database for
v1. Resolve an absolute database path outside disposable worktree roots. Existing
per-worktree databases require a separately designed import workflow; the
harness does not merge SQLite files or duplicate Experience records.

### 6.2 Shared history, separate current state

| Data | First-release sharing rule |
| --- | --- |
| Current files, chunks, vectors, graph, pins and working set | Remain owned and resolved by the active workspace |
| Sessions, tasks, episode membership and current verification | Remain within their existing workspace/session scope |
| Eligible verified Experience | May be retrieved from registered sibling workspaces as historical supplemental context |
| Evidence underlying a shared Experience | Available as bounded historical explanation with original ownership; not injected as current Events |
| Explicit decisions, notes and other Memory | Existing scope/trust rules remain; repository-scoped Memory requires an explicit visibility design before being added |
| Knowledge from a different repository | Deferred; no automatic inclusion through this feature |

The user's confirmed requirement covers Experience. General repository-scoped
Memory is a possible next increment, not a blanket permission to promote branch
decisions or private notes. Future cross-project learning should use explicit
scope, provenance, review, and applicability rules within CortexWeave. Do not
build a global knowledge pool or generalization engine for the first harness.

Default Shuttle requests to the registered repository's eligible Experience
pool; retain a workspace-only request for isolation and evaluation. Add an
explicit opt-in scope to the native facade and keep existing callers' default
workspace behavior. The substrate resolves membership and applies one bounded
candidate/selection policy; Shuttle does not fan out arbitrary searches or
rank merged histories itself. Apply lifecycle exclusions and authority ordering
before every bound across the complete authorized pool.

An Experience keeps its original workspace/session/task/episode, immutable
evidence, and signature. Current exact failure keys encode workspace identity:
do not change the workspace inside a stored signature or strip it to invent an
exact match. Introduce a separately explained repository-compatible match using
supported domain, language/tool, normalizer/version, scope and stable-component
rules. It does not establish exact recurrence or prove the repair applies to the
consumer's branch. Normalizer compatibility and any future repository-level
exact identity need an explicit versioned contract.

Every cross-worktree result and context item must distinguish consuming workspace
from source workspace and include repository-sharing provenance. The current
native helper stamps all selected items with the packet workspace ID; extend
that representation rather than mislabeling a sibling Experience as local.

Separate the prepared native context contract into two inventories:

- Current-code hydration candidates: active workspace, selected chunk/source ID,
  path/symbol, selection metadata, and content/revision identity for freshness.
- Historical references: original workspace/session/task/episode, Experience and
  snapshot IDs, historical authority, and repository-sharing provenance. These
  explain history and never authorize current-code hydration.

Currently `HarnessHydrationRequest::from_context` copies all selected sources,
while hydration rejects any selected source from a different workspace. Merely
adding correct origin IDs would therefore break a mixed local-code/sibling-history
packet. Introduce a versioned representation or a compatible, explicit conversion
that supplies only active-workspace code candidates to hydration. Keep the
service's workspace enforcement; do not weaken it to make mixed packets pass.
An explicitly requested foreign chunk or historical ID must be rejected, not
silently dropped or rebased. Historical detail uses separately authorized,
bounded native reads and immutable source snapshots, not live sibling file reads.

Require a mixed-packet contract test: select active code plus sibling Experience,
hydrate the active code successfully with selection provenance preserved, and
reject attempts to hydrate the sibling through that request. Also test stale
content hashes, revoked membership and identical paths with divergent contents.
Historical source snapshots and graph paths remain labeled as historical;
matching relative paths or symbols never rebase them onto the consumer's branch.

Resolve membership, Experience lifecycle, and selected historical detail from
one consistent read or detect revision changes and retry. A revoked membership
or disputed/refuted/superseded Experience must be excluded from subsequent
requests and invalidate applicable caches. Explain selection, exclusions, and
any truncation. Shared history cannot displace required current source or active
verification, and cannot supply a missing passing check for the consumer task.

### 6.3 Worktree retirement and proof

Removing a working directory does not automatically delete its CortexWeave
workspace or history. Retain an inactive source workspace and immutable snapshots
so sibling worktrees can still explain eligible history. Explicit deletion of
a CortexWeave workspace retains its existing destructive meaning and removes
its owned Experience; the UI must distinguish that from worktree retirement.

Require tests for sibling sharing, isolated clones with identical remotes,
divergent same-path code, detached HEAD, Windows path aliases and Linux case,
membership revocation, disputed history, origin worktree removal, aggregate
bounds, and concurrent membership/lifecycle changes. A fresh worktree must reuse
a verified historical repair while resolving and verifying its own source.

## 7. Automatic evidence and Experience

The harness starts explicit episodes around declared task phases or a scoped
failure/repair effort. It assigns factual Events to those episodes using its
execution state and IDs. It does not ask CortexWeave to infer episode membership
from timestamps or unstructured chat.

Keep detailed tool lifecycle telemetry in the run journal and factual domain
Events in CortexWeave. Associate the material failure, attempts, source-change
observations, verifier results, and acceptance with an episode in explicit order.
An episode currently permits 100 primary Events, with additional bounds on
Experience attempts and evidence. Episode closure does not accept a caller-chosen
outcome: consolidation derives its result from the actual evidence. In particular,
closing pending unverified edits cannot manufacture an inconclusive Experience.

Plan capacity before each episode-dependent action using the qualified producer's
bounded material Event/attempt/evidence counts. Reserve room for the next complete
verification and, when needed, user acceptance. Keep high-volume process telemetry
outside primary episode membership. If capacity cannot be bounded safely, pause
before the next mutation rather than promising eventual consolidation.

Roll over only at a completed failure-attempt-verification boundary. Finish the
pending verification while reserved capacity remains, then close according to the
facts and record the segmentation reason in the harness ledger. If continuing a
repair in a new episode, run a fresh real verifier observation to establish its
own initial failure before another attempt. Never copy an old Event into a second
primary episode or relabel a cached result as fresh evidence. If the recheck passes,
do not invent a failure anchor. If a limit is already exhausted or evidence is
incomplete, pause or record a supported no-result/abandonment disposition; do not
force an outcome or claim verification continuity across episodes.

Test near-capacity rollover, insufficient verification reserve, multiple attempt
bounds, fresh failure anchoring, and a recheck that unexpectedly passes. A feature
or investigation without a supported initial failure may have a valid completed
task and factual Events but no repair Experience under the current extractor.
Generalized learning from every successful task is not an implicit v1 feature.

Preview and, when eligible, accept consolidation at a task/phase boundary while
the harness is active. `review_required` and typed no-result are valid recorded
outcomes; no Experience is fabricated to make the automatic workflow appear
complete. Explicit memories retain their separate intent and trust rules.

Rust compiler, Cargo test, and registered generic-verifier contracts exist.
Real command-to-evidence capture still needs qualification; do not assume raw
stdout or a zero exit code supplies every structured identity field. Unsupported
checks retain factual results and their limitations.

The proposed CortexWeave v0.5.1 work adds Vitest and Python `unittest` producers
and retry-safe capture import. Treat that work as a tracked dependency for
those qualification scenarios, not as implemented capability. Language analyzer
support does not imply test-runner evidence support. Avoid duplicating producer
normalization in the harness if the substrate project ships reusable capture
helpers.

## 8. Recovery and local inference

Recovery correctness is a prerequisite to the first live-model write, including
the early isolated-fixture experiment. It is not deferred to daily-use polish.
Use one in-flight action and persist its stable action ID, normalized tool and
arguments, workspace/input preconditions, permission grant/scope, execution state,
observed result/artifact references, and separate domain-delivery state.

Before dispatch, commit the authorized intent, then mark it started before any
effect can occur. After observation, durably store artifacts and atomically commit
the result and ordered delivery outbox in harness-owned SQLite. Execution states
include prepared, started, succeeded, failed, cancelled, and unknown. Authorization
and delivery status are separate: a failed delivery does not mean the command
failed, and an unknown completion must not be reported as a known failure.

Resubmitting the same action ID returns its recorded state/result, not another
execution. Never automatically replay an action that may have started. Inspect
process identity and observable effects, or pause for user direction; any newly
authorized retry is a distinct action linked to the uncertain one. Use the same
rules for patch writes and commands. A policy change invalidates unstarted grants
where necessary and cannot undo effects already dispatched.

| Crash window | Recovery rule and required fault-injection test |
| --- | --- |
| Before durable intent | No action may have been dispatched |
| Intent committed, provably not started | Revalidate permission and input preconditions before dispatch |
| Started marker committed, no durable completion | Completion is unknown, even if dispatch may not have happened; do not replay |
| Durable result/outbox, no CortexWeave acknowledgement | Retry delivery only; do not rerun the tool |
| CortexWeave committed, acknowledgement lost | Resolve the same request key to the original record/receipt; do not duplicate history |
| Acceptance or finalization interrupted | Resume the recorded offer/receipt and remaining ordered deliveries; do not invent acceptance or another verifier observation |

Add minimal native idempotent creation/delivery contracts before this gate:
caller request keys for session/task/episode creation and retry-safe factual Event
delivery, scoped to the operation and validated owner. Reusing a key with the same
canonical request returns the original identity/receipt; different content is a
conflict. CortexWeave owns these receipts and domain changes in one SQLite
transaction. Existing episode mutation/consolidation idempotency remains in use;
qualified capture-import receipts are reused where applicable. No direct database
access or timestamps-as-identity workaround belongs in the harness adapter.

If journaling fails, stop before further effects. If critical provenance delivery
fails, retain the outbox and pause dependent actions/finalization with a visible
delivery-pending state. Acknowledged acceptance, episode closure, consolidation
disposition, and terminal task update must reconcile after a crash. A typed
no-result or review-required consolidation disposition can satisfy finalization;
an Experience is not mandatory for every valid task completion.

Recover the same still-open session/task when their persisted identity is known.
If a prior session has ended, use resume context to create a linked continuation
task in a new session, with new episodes. Episodes never cross sessions. If a
creation committed before its ID was saved, recover it through the same persisted
request key. Exercise interrupted creation, evidence delivery, cancellation,
process descendants, and finalization through both scripted interfaces.

Connect to the user-managed llama.cpp service. Use streamed chat completions,
validated structured tool calls, bounded retries, timeouts, and cancellation.
Pin model identity, server build, template, sampling settings, and context
capacity for a run. Model replacement during a run requires explicit handling.

Qualify real tool calls and then a complete small coding loop on the chosen
model/template before building the full product. llama.cpp documents native and
generic tool formats, with differing efficiency, and `--jinja` setup for function
calling. Protocol compatibility
alone does not establish reliable tool behavior. [llama.cpp function calling](https://github.com/ggml-org/llama.cpp/blob/master/docs/function-calling.md).

Inference and embedding service configuration remain independent; CortexWeave's
documented embedding default is port 8081. Check that the chosen hardware can
run both workloads and record their resource contention during evaluation.
Endpoint failure produces a visible paused/degraded state; never silently switch
to a remote model or another embedding space.

### 8.1 Reference model and hardware profile

Qualify `unsloth/Qwen3.5-4B-GGUF:Q4_K_M` on the user's RTX 2060 Mobile with 6 GB
VRAM and 32 GB RAM. Preserve the supplied launch profile as the baseline.
Record the resolved GGUF filename/hash, both executable versions/backends,
offload, template, effective context, generation limits and sampling settings.
The model card and GGUF selector do not establish the memory footprint or
latency of this particular installation.
[Qwen3.5-4B model card](https://huggingface.co/Qwen/Qwen3.5-4B),
[selected GGUF repository](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF).

Worker command supplied by the user:

```text
llama serve -hf unsloth/Qwen3.5-4B-GGUF:Q4_K_M --port 8080 --no-models-autoload --no-mmproj --jinja -ngl 36 -np -1 -fa on -ctk q8_0 -ctv iq4_nl -c 175104
```

Embedding command normalized to one line from the pasted PowerShell transcript:

```text
llama-server -m "C:\Users\Capta.cache\huggingface\hub\models--nomic-ai--nomic-embed-text-v1.5-GGUF\snapshots\0188c9bf409793f810680a5a431e7b899c46104c\nomic-embed-text-v1.5.Q8_0.gguf" --embedding --alias nomic-embed-text-v1.5 --host 127.0.0.1 --port 8081 --ctx-size 8192 --batch-size 4096 --ubatch-size 4096 -ngl -1
```

The model path above is retained as supplied; the `Capta.cache` portion is not
a verified filesystem location or a corrected installation instruction. Capture
the resolved file during setup. These are records of the user's configuration,
not commands executed or services reconfigured during this planning session.

| Setting | Worker | Embeddings |
| --- | --- | --- |
| Entry point | `llama serve` | `llama-server` |
| Model weights | Qwen3.5 4B Q4_K_M | nomic-embed-text-v1.5 Q8_0 |
| Endpoint | Port 8080 | `127.0.0.1:8081`, alias `nomic-embed-text-v1.5` |
| Requested context | 175,104 tokens | 8,192 tokens |
| GPU offload option | `-ngl 36` | `-ngl -1` |
| Cache and attention | K `q8_0`, V `iq4_nl`, Flash Attention on | Defaults of the installed binary |
| Server slots | `-np -1` | Defaults of the installed binary |
| Batch/microbatch | Defaults of the installed binary | 4,096 / 4,096 tokens |
| Modality | Projector disabled; first harness uses text | Dedicated embeddings |

Both commands request GPU offload. Confirm actual layer/device placement from
their startup/runtime reports; do not equate a flag with measured residency.
Current upstream represents numeric `-ngl -1` as automatic selection, while
historical builds differ. Record the installed binary's semantics as part of
the profile. [llama.cpp argument implementation](https://github.com/ggml-org/llama.cpp/blob/master/common/arg.cpp).

Current server documentation defines `-np -1` as automatic slot selection.
Treat 175,104 as configured capacity until the installed server reports its
effective per-request capacity and slot/cache behavior. Shuttle still issues
one active inference request; server slots do not enable harness subagents.
Record the model ID exposed by the actual endpoint rather than guessing it from
the filename. Respect the supplied no-autoload policy where the server exposes
model routing: metadata checks should not implicitly load models, and an
unloaded configured worker produces an explicit readiness state. Preserve the
user-managed service lifecycle. [Server options](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md).

Use one active inference request, a compact tool catalog, bounded command output,
and short explicit task state. Prefer one tool action at a time initially; keep
any model-returned batch sequential, bounded, and policy checked. Measure the
resource cost of background indexing and embedding concurrency before choosing
their defaults. The primary qualification runs both supplied GPU-offload
configurations together. CPU embedding is a separately labeled optional
comparison if contention warrants it, keeping model/space identity explicit.

The inspected CortexWeave configuration uses the supplied nomic alias at port
8081. Its provider input limit is 2,048 tokens, batch limit 4,096 tokens,
reserved allowance 32 tokens, one text per request, and at most two concurrent
embedding jobs. These are client ingestion limits independent of the server's
8,192 context setting. Preserve them for the baseline and measure any tuning
separately. Keep nomic's configured document/query prefixes and embedding-space
compatibility checks in force.

At qualification, hold the supplied server configuration fixed and compare
Shuttle request budgets such as 8K, 16K, 32K and 64K when they fit the effective
capacity after reserving output. Those are initial experiments, not established
defaults. Add larger requests when a representative task justifies them. Any
change to server context allocation is a separate experiment so its resource
effect is not conflated with the amount of evidence supplied to the model.
Reserve generation capacity, including any emitted reasoning tokens. Record
peak VRAM, system RAM, prompt-processing and cached-token counts, time to first
token, generation speed, and time to verified outcome with embeddings present.

Qualify the installed Qwen3.5 tool template and both enabled/disabled thinking
settings where supported; choose and freeze a default from correctness and
latency evidence. Keep structured tool calls separate from prose and reasoning
fields, and enforce call-schema validation in the engine. The upstream llama.cpp
tree has a Qwen3.5-4B template; its existence does not qualify the installed server.
[Qwen template](https://github.com/ggml-org/llama.cpp/blob/master/models/templates/Qwen3.5-4B.jinja).

## 9. Terminal experience

Use charcoal/ash surfaces, muted violet accents, pale lavender text accents,
and legible neutral body text. Establish color tokens and a reduced-color
fallback. State labels, glyphs, and text must convey status without relying on
color alone.

Treat the Crush reference as guidance for composition and interaction: prominent
input, compact status, readable chat spacing, restrained decoration, crisp tool
summaries, and contextual dialogs. Ratatui is the implementation toolkit; it
does not prescribe the visual identity. Establish a Shuttle wordmark and theme
instead of reusing Crush branding. [Crush's official demo](https://github.com/charmbracelet/crush#readme).

Proposed theme tokens, subject to actual terminal readability checks:

| Token | Color | Use |
| --- | --- | --- |
| Background | `#17151C` | Main charcoal/ash canvas |
| Panel | `#211E29` | Input and selected detail surfaces |
| Raised surface | `#2B2535` | Dialogs and focused controls |
| Accent | `#A597B8` | Muted violet focus and selection |
| Primary text | `#DED8E8` | Chat and important labels |
| Secondary text | `#A59BAB` | Metadata and inactive labels |
| Success | `#92AA9D` | Muted sage with text/glyph status |
| Failure | `#CA8E9A` | Dusty rose with text/glyph status |
| Attention | `#CEBA91` | Muted amber with text/glyph status |

The main view contains streaming chat and collapsible tool results. A compact
status area shows workspace/branch, active model, autonomy, task phase, context
usage, and verification state. Supporting panels show the plan, diff, context
sources and selection reasons, and history. Permissions and acceptance appear
as contextual prompts in the terminal.

Support multiline input, paste, keyboard navigation, scrolling, resize, clear
interrupt behavior, command history, and resume. Keep indexing and model health
visible without flooding the chat. Establish the theme and basic interaction
early, then finish the richer inspectors after the first verified task cycle.

Before the UI is accepted, review a visual prototype covering idle/input,
streaming response, tool execution, permission, diff, and task acceptance states.
Validate the actual Ratatui rendering in Windows Terminal and a named Linux
terminal at both compact and wide sizes. Check keyboard flow, Unicode fallback,
selection/copy, paste, and reduced-color behavior. A correct engine alone does
not satisfy the user's condition for choosing this UI stack.

## 10. Research and evaluation

Provide a headless runner over the same engine, tools, context adapter, and
policy used in terminal chat. Use fixture-supplied verification plans and record
automated benchmark judgments separately from human acceptance. A benchmark
judge is not a synthetic user-acceptance Event.

Begin with four task families: repository investigation, a small feature with
acceptance tests, a scoped failure repair, and a fresh-session related repair
with useful historical Experience. Use small deterministic fixtures first,
then representative real repositories and supported runners.

Separate a development corpus from a frozen held-out evaluation corpus, with
related repairs/repository families kept together to reduce leakage. Tune prompts,
tool schemas, context budgets, thinking settings and controller limits only on
development tasks. The early Qwen fixture is development evidence, not a held-out
result. Freeze task manifests, check definitions, source and database snapshots,
seed historical Experience, versions and repeat counts before held-out execution.
If held-out observations inform tuning, retire those cases into development and
use a new independently frozen holdout for subsequent claims.

Include a mandatory sibling-worktree scenario: Worktree A records a supported
repair, fresh Worktree B starts with no transcript and a related failure, and
the repository-scoped treatment retrieves A's Experience without importing A's
live source or task state. Add a workspace-only lane and an unrelated clone
negative control. Preserve and version the same repository membership mapping
in the sealed database for both treatment lanes.

Separate two research questions: the effect of constructed context versus a
bounded transcript baseline, and the effect of adding eligible Experience to
otherwise identical current context. Predeclare each comparison. Keep model,
tool permissions, initial repository state, verification requirements, run
limits, and non-treatment settings fixed. Fork sealed initial databases and
source snapshots for each lane; never let an earlier lane teach the next lane.

Record factual evidence in every lane, archive its results, and start the next
from the same sealed initial state. Prevent the baseline from fetching disabled treatment
data through an explicit retrieval tool, direct database read, transcript, or
artifact path. Keep the same model-visible tool surface and enforce treatment
scope in the adapter. For the Experience comparison, current evidence and its
budget/authority remain identical; only eligible historical supplementation
changes. Reset model conversations and qualify any server-side session/cache reuse
so it cannot carry prior-lane content. Do not claim that public tasks were absent
from the model's pretraining; report that limitation separately from run leakage.

Keep independent held-out checks outside model-visible evidence and distinguish
them from the task's declared verification plan. Restrict fixture access where
supported and audit accidental exposure; a leaked check invalidates that held-out
run. Do not claim an unrestricted host shell provides containment. The benchmark
judge records evaluation outcomes without fabricating human acceptance, and the
interactive acceptance path is separately tested with actual user input.

Record correctness under an independent evaluator, unrelated changes, human
interventions, repair iterations, required and optional verification calls,
time to first verified result, completion time, input/output/context tokens,
retrieval/indexing latency, and Experience selection/authority. Record seed,
model/quantization, server/build/template, hardware, prompt and config hashes,
source/input snapshot and dirty diff, verification-plan revision, treatment and
corpus IDs, and tool/runtime versions. Required checks are frozen per case; optional
checks, waivers and human interventions are separately visible.

Repeat live runs and report variability. A fixed seed is not a promise of
identical inference across hardware/builds. Replay can reproduce recorded
controller decisions; it does not prove independent model correctness. Null or
negative results remain valid. The existing v0.5 experiment's apparent saving
came from a discretionary verification difference, so required checks must be
held constant in the new comparison.

Define the primary outcome before execution: required independent checks satisfied
within the fixed run budget. Keep failures, timeouts, budget/stall pauses, and
no-result runs in the denominator; report harness/infrastructure-invalid runs
separately with reasons and retain their artifacts. Do not select only successful
repeats or change required checks to improve savings. Report latency, tokens and
optional verification as secondary outcomes, alongside correctness and variability.
Deterministic conformance/isolation is a release gate; a positive measured benefit
is not. Early small-sample findings do not establish general model superiority.

## 11. Repository-grounded prerequisites

| Finding in the inspected source | Planning consequence |
| --- | --- |
| `HarnessContextRequest` carries query/scope IDs/budget but no active failure signature; `prepare_harness_context` constructs a default `ContextRequest` | Propose a narrow facade extension for failure-aware native context and needed explicit source scopes |
| Context counting uses the embedding provider's tokenizer | Harness must budget the complete reasoning-model request separately |
| Hydration checks source identity/path/symbol, without proving unchanged content bytes | Bind verification/acceptance to input snapshots and plan revisions; check current hashes before acting; hydration is not a snapshot lock |
| `from_context` copies all selected sources and hydration rejects foreign workspace entries | Separate current-code candidates from historical references; pass the mixed-packet hydration and rejection gates |
| `record_event` inserts a caller-ID Event without a retry receipt; lifecycle creation generates IDs inside the service | Add native creation/delivery keys and fault-injection recovery tests before the first live-model write; reuse qualified capture import where applicable |
| Workspace registration is based on canonical root; Experience search is workspace-scoped | Required repository membership and explicitly scoped historical retrieval extension |
| Failure keys include workspace identity; native selected-source records assume the packet's workspace | Preserve exact keys; add explained repository-compatible matching and per-item origin provenance |
| Episodes are session-scoped and bounded; closure is irreversible and does not accept an outcome | Reserve complete verification capacity, roll over at factual boundaries, and obtain a new failure observation for a new repair episode |
| Task status has domain terminal states, not every controller phase | Keep paused/verification/acceptance/action state and the compact progress ledger in harness-owned storage |
| No complete selection-freshness key is established by this plan | Compose every request; conservatively refresh evidence unless all relevant dependencies can be validated |
| v0.5.1 runner work is a proposed plan | Pin and qualify the actual version before claiming Vitest/unittest support |

These are substrate service-contract proposals, not permission to move harness
policy into CortexWeave. Record accepted changes in CortexWeave's decisions
document and verify them there. Record harness architecture decisions in the
new repository once the design is accepted.

## 12. Implementation milestones and gates

| Milestone | Deliverable | Exit evidence |
| --- | --- | --- |
| 0a. Safe experimental foundation | Minimal reusable controller, scripted model, durable action journal/outbox, native creation/delivery receipts, pinned substrate, development fixture, early terminal prototype | Crash-window matrix passes before model-directed writes; unknown execution never replays; prototype states reviewed against the intended visual direction |
| 0b. First complete Qwen task | Supplied local worker/embedding profile, compact tools, snapshot-bound checks/acceptance, bounded failure-repair loop in the isolated development fixture | Real read, patch, failing check, repair, recheck, verification and actual user acceptance; supported factual evidence/consolidation disposition recorded; malformed-call/stall paths pause correctly |
| 1. Engine and terminal shell | Expand the same controller into themed terminal and headless interfaces, durable task ledger and run controls | Both interfaces drive the same scripted tasks; compaction/resume preserve attempts and budgets; actual rendering and interrupt/resize/resume pass on Windows and Linux |
| 2. Authorized tools and workspace modes | Full initial read/search/patch/command registry, permission profiles, checkout/worktree identity, verification-plan and acceptance inspectors | Dirty changes preserved; unauthorized actions blocked; changed tests/source invalidate offers; cancellation accounts for process effects |
| 3. Context and repository history | Bounded request composition, conservative refresh policy, native repository membership, eligible sibling retrieval, separate history/hydration inventories | Mixed-packet local hydration succeeds; foreign/stale hydration rejects; sibling positive and isolation/lifecycle/removal negative cases pass |
| 4. Full verified Experience cycle | Qualified producers, bounded episode rollover, finalization/reconciliation, repair and feature workflows | Near-limit failure/attempt/verification boundaries pass; supported repair consolidates; honest no-result feature case completes; fresh sibling worktree reuses history while verifying its own source |
| 5. Daily-use completion | Richer inspectors, recovery UX and diagnostics, resource/performance tuning on development tasks | Early recovery guarantees remain green under dirty checkouts, outages and long runs; reference machine is usable with embeddings present |
| 6. Held-out evaluation and public preparation | Frozen protocols/corpora, repeated trials, docs, license choice, Windows/Linux installation | Required checks and all run outcomes reported; leakage controls and shared engine verified; both platforms qualified and hardware/model profiles documented |

Milestone 0 is deliberately a complete, tiny development experiment alongside the
visual prototype, not merely a streamed tool-call demonstration. Use a controlled
fixture with a staged failing check so the real loop must observe and repair a
failure; do not rely on the model accidentally introducing one. Model-directed
filesystem writes are restricted to the isolated fixture; journal and domain
storage remain application-managed. The minimal engine and adapter become the
production path rather than a disposable second implementation.

The first live experiment may run on the user's Windows machine while native
Linux qualification follows through the listed gates. Repository-wide Experience
sharing is still required before release, but it does not block this initial
single-workspace model/loop feasibility test. Unsupported protocols or unacceptable
resource use discovered here trigger an explicit plan revision before broad buildout.

Do not release a retrieval-only prototype as the first completed harness. The
release includes the automatic evidence cycle, task verification, user acceptance,
both workspace modes, sibling-worktree Experience sharing, and the shared
evaluation engine. The reference setup must be usable on the stated 6 GB VRAM
machine; its latency and memory budgets will be established by measurement.

Windows and Linux are release gates. Keep process invocation, argument handling,
terminal lifecycle, paths and platform-specific persistence directories behind
small adapters. Prefer executable/argument arrays for typed tools; label and
record shell mode when needed. Qualify PowerShell and the chosen Linux shell
separately rather than translating commands by string substitution. A WSL run
does not substitute for native Windows qualification. Desktop Mac support,
other GPU backends and distribution-specific packaging can follow later.

Apply formatter, build/check, Clippy, and focused unit/integration gates as the
Rust implementation develops. Use deterministic model/embedding doubles for
controller tests and separate documented live-model qualification. Add tests
for policy enforcement, crash windows, verification scope, provenance, stale
source, malformed tool calls, context overflow, and cross-workspace isolation.

## 13. Remaining inputs and implementation probes

The user has supplied both launch commands, resolving the configured context,
quantization, cache types and requested offload settings. Milestone 0b verifies
effective runtime behavior; development trials choose prompt/concurrency defaults
before the held-out protocol freezes them.
The remaining checks can be performed during implementation qualification.

Use Shuttle as the working product name. Select the final repository/package
name and location before creating the separate repository. No repository was
created as part of this planning revision.

Engineering probes include a supported Windows and Linux version/terminal,
the exact GGUF/server/template identity, implementation of the specified recovery
contracts, repository membership migrations, qualified evidence producers, and
inference/embedding resource contention. These have concrete milestone gates
above and do not need another general product questionnaire.

## References

Repository references, relative to the CortexWeave repository root:

- `src/service/harness.rs`, `src/service/cortex.rs`, `src/domain/context.rs`.
- `src/domain/episode.rs`, `src/storage/repositories.rs`.
- `src/service/consolidation.rs`, `src/domain/failure.rs`.
- `docs/native-adapter.md`, `docs/context.md`, `docs/verified-experience.md`.
- `docs/decisions.md` (D069, D080-D081, D105-D106).
- `docs/experiments/v0.5-real-repository-paired-experiment-result.md`.
- `CortexWeave v0.5.1 Test Evidence and Vitest Integration Plan.md` (proposed).
- `Small Models, Smart Systems_ A Roadmap Toward Frontier-Level Agentic Results.md`.

External documentation was checked on 2026-09-02. Implementation qualification
must record specific dependency and server versions rather than assuming that
moving documentation defines a stable protocol.
