# Memory Integrity

CortexWeave distinguishes memory origin from memory trust. This keeps imported
material inspectable without letting it silently become context evidence.

Memory is intentionally distinct from v0.5 Experience. Memory is explicit
durable knowledge with its own trust-review workflow; Experience is an immutable
historical interpretation of an explicit evidence-backed episode. See
[Verified Experience Core](verified-experience.md).

## Origin and Trust

`human_authorized` memory is trusted when recorded. `imported` memory must be
recorded as `unreviewed` and must include at least one non-empty source byte
segment. Imported memory remains available through memory search and listing so
a reviewer can inspect it, but automatic context, temporal retrieval, working
sets, pins, resume evidence, and checkpoint decision references accept only
trusted memory.

`review_memory_trust` changes imported memory to `trusted` or `rejected`. Every
change requires a non-empty reviewer and reason. SQLite updates the memory and
inserts an immutable `memory_trust_reviews` row in one transaction. A caller
cannot record imported memory as already trusted.

Legacy rows migrate to `human_authorized` and `trusted`, preserving the meaning
of memory that was explicitly recorded before the policy existed.

## Source Segments

Memory provenance and selected context items use `SourceSegment` values with a
source locator and half-open byte range. Segments within one memory cannot
overlap. Code chunk ranges flow from storage through retrieval, context packets,
and native-harness audit records.

Packet evaluation measures duplicate tokens in proportion to the union of
already-selected byte ranges. A later item whose range overlaps an earlier item
by half contributes half its estimated tokens to the duplicate count. Identical
text from a disjoint range is not treated as duplicated source evidence.

## Consolidation

`consolidate_memories` is bounded, deterministic, and read-only. It compares an
explicit list of memory IDs and reports:

- normalized token overlap;
- source-segment overlap;
- contradictions where both memories declare the same structured claim key
  with different JSON values;
- a possible older-to-newer supersession relation when the newer memory is
  trusted.

A duplicate proposal requires at least 80% normalized token overlap, or at
least 80% source overlap supported by at least 50% token overlap. Free-form
prose alone never produces a contradiction claim.

Proposals do not change durable memory. `apply_memory_supersession` is a
separate operation requiring trusted records, a reviewer, and a reason. SQLite
still rejects cycles transactionally, and the accepted review provenance is
stored with the relation.
