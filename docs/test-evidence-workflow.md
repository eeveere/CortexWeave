# CortexWeave test-evidence workflow

This is the repeatable Crush-side sequence for a supported capture. Keep the
capture bundle path and returned Event ID in the session transcript; do not
copy a runner report into an Event by hand.

1. Resolve the project workspace with `workspace_list` or an explicit root
   path. Start or reuse a session and create a debugging or verification
   episode.
2. Run one helper from the project's own environment. For Vitest, select one
   complete test file:

   ```text
   node integrations/vitest/capture_vitest.mjs --workspace <project> --test-file <relative-test-file> --output <bundle-path>
   ```

   For Python, select one complete module or `TestCase` class using the
   project's interpreter:

   ```text
   python integrations/unittest/capture_unittest.py <module-or-class> --workspace <project> --output <bundle-path>
   ```

3. Call `test_evidence_record` with the workspace, session, a fresh request
   key, and the bundle JSON. When the helper wrote the bundle below the
   registered workspace, `test_evidence_record_file` accepts its
   workspace-relative `.json` path instead; it is the preferred MCP route for
   a large capture. Save the returned Event ID, eligibility result, and failure
   normalization.
4. If the run is part of the same repair episode, call `episode_add_events`
   with the returned IDs and the current episode version. Close the episode
   only after the final verification run.
5. Use `event_evidence_inspect` for the stored canonical signature. Preview
   consolidation, then explicitly accept the exact preview when the service
   reports a supported failed-attempt and terminal pass with unchanged scope.
6. In a fresh Crush session, provide current task/code/failure evidence first,
   then request bounded context using `active_failure_event_id` (preferred) or
   an already inspected active failure signature. Historical
   Experience is compatible context; it does not prove the current failure is
   identical or authorize source changes.

The initial profiles require a completed non-watch run with a complete,
nonempty inventory. Focused, filtered, sharded, fail-fast, retry/repeat,
snapshot-update, compound check, and unittest subtest-containing scopes stay
inspectable but are not automatic verification. A runner crash or incomplete
report must be retained as a diagnostic capture and never promoted to a pass.

The helper only observes the external runner. It does not edit source, install
packages, retry a repair, create episodes, or submit evidence. The CortexWeave
service owns validation, receipts, workspace scope, failure identity, and
persistence.
