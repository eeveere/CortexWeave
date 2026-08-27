# Checkpoint 06: Watcher Chaos

**Status:** Passed after corrective changes

## Findings and Fixes

1. The startup scan originally ran before filesystem notifications were armed,
   leaving a race in which an edit could occur after discovery but before the
   watcher started. The watcher is now armed first; events produced during the
   startup scan queue behind it, and queue overflow forces another full scan.
2. A missing directory whose name contained a dot could be mistaken for a file,
   leaving nested document rows behind. Unknown missing paths now trigger an
   authoritative ignore-aware workspace scan regardless of extension.
3. Targeted events could index a newly created ignored file, while an indexed
   text file that became binary or oversized retained stale rows. Unknown files
   now pass through discovery, ignore-control changes force a scan, and the
   reconciler removes documents that are no longer eligible text inputs.

A failed file reconciliation no longer abandons unrelated paths in the same
batch. Overflow recovery and ambiguous-path rescans persist normalized events
with their reason and result counts.

## Chaos Matrix

Automated tests cover:

- rapid repeated saves;
- formatter-style remove/rename replacement;
- create plus modify bursts with a one-slot queue;
- `.py` to `.txt` analyzer reassignment;
- `.ts` to `.tsx` parser reassignment;
- Rust file rename;
- rename collision with an existing indexed target;
- file deletion;
- dotted directory deletion;
- ignored-file activity and ignore-aware convergence;
- text-to-binary and text-to-oversized transitions;
- changes made while the watcher is offline;
- SQLite close/reopen followed by startup reconciliation.

All cases converge to final filesystem state. Removed paths have no document,
chunk, vector, or FTS residue; surviving paths have the correct language and
analyzer assignment.

