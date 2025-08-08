# gix-receive-pack – Roadmap

Status: milestone-based execution plan aligned with SPEC; this task updates documentation only.

Guiding principles
- Spec-first and test-first where feasible with golden transcripts captured from upstream Git.
- Blocking-first implementation; async parity added behind a feature.
- Strict-compat realized via adapter/formatter layers, not embedded in core logic.
- No unsafe code; stable error taxonomy and predictable wire output.

Upstream references for validation
- receive-pack reference implementation: git-upstream/builtin/receive-pack.c
- Protocol specification: git-upstream/Documentation/gitprotocol-pack.adoc

## M0: Scaffolding & Docs (done)

Deliverables
- Compiling crate skeleton with typestate builder and Session scaffolding.
- SPEC.md and ROADMAP.md committed with scope and plan.

Acceptance criteria
- cargo check and cargo test pass for the crate under default features in the workspace.
- SPEC and ROADMAP cover the sections outlined and cross-link to upstream references.

Test plan
- CI job for check and basic doc links validation.

Backout or fallback
- None needed; documentation only.

## M0.5: Feature Matrix & Error Conventions

Deliverables
- Adopt feature gates in manifests (planned; code to be adjusted in future milestones).
- Add interrupt hooks at key loops (planned).
- Define crate::Error, module sub-errors, and Error::kind() (planned).

Acceptance
- Docs updated; CI plan for feature matrix smoke tests listed.

Test plan
- Doc-only for this milestone; implementation tests will be added within M1–M7 where applicable.

Backout or fallback
- None; documentation-only scaffolding.

## M0.Rename: Crate renames and workspace prep (planned, docs-only)

Deliverables
- Rename legacy crate directory to gix-receive-pack-deprecated and update workspace members.
- Rename this crate directory from gix-receive-pack to gix-receive-pack and update workspace members.
- Update all internal documentation links to reference the new crate name.

Acceptance criteria
- Workspace builds after the rename operations with no code errors; documentation reflects the new names.
- CI adjustments are deferred to a follow-up code task.

Notes
- This item describes a forthcoming code change. Perform the actual directory renames and workspace updates in a separate code-mode task.

## M1: Protocol Advertisement (blocking)

Deliverables
- protocol/advertise: write v0/v1-style advertisement with refs and capabilities using gix-packetline-blocking.
- protocol/capabilities: typed CapabilitySet; capability ordering control; agent emission toggle from config.
- Config mapping for receive.advertiseAtomic, transfer.fsckObjects, agent string, and feature toggles.
- Strict-compat adapter for exact capability ordering and spacing when the feature is enabled.

Acceptance criteria
- Empty repository advertisement contains only special refs and capabilities; non-empty includes all visible refs filtered by HiddenRefPredicate.
- Capability list includes at least: report-status, report-status-v2, side-band-64k (optional), quiet, delete-refs, ofs-delta (if supported), agent.
- When strict-compat is enabled, golden snapshots match upstream byte-for-byte for known scenarios.

Feature-gate checklist
- blocking-io and async-io designs accounted for; async-io may be stubbed initially.

Test plan
- Unit: capability encoding/ordering; agent string formatting; hidden refs filtering.
- Golden: advertisement snapshot for empty and non-empty repos compared to fixtures captured from upstream git-receive-pack.
- Negative: ensure unknown/disabled capabilities are not advertised.

Backout or fallback
- Disable strict-compat adapter by default; fall back to deterministic idiomatic capability ordering.

References
- Upstream mapping: advertise_refs(), send_ref() in git-upstream/builtin/receive-pack.c
- Protocol sections: git-upstream/Documentation/gitprotocol-pack.adoc (advertisement, capabilities)

## M2: Head-Info Parsing (commands, shallow, options)

Deliverables
- protocol/commands: parse receive-pack head info including update commands old..new OIDs and refnames.
- protocol/options: parse and type capabilities negotiated by the client; push-options parsing with typed PushOptions.
- Validation: object-format enforcement; reject capabilities not advertised; option defaults resolved.

Acceptance criteria
- Valid command lists produce a typed set of CommandUpdate values with invariants checked (create with zero old, delete with zero new).
- Malformed packets and invalid combinations result in Protocol errors with actionable categories and messages.
- Push-options are parsed and accessible to hooks and policy.

Feature-gate checklist
- blocking-io and async-io designs accounted for; async-io may be stubbed initially.

Test plan
- Unit: parsing for commands, shallow lines, and push-options; capability negotiation resolution.
- Golden: transcripts from upstream clients for typical flows; ensure round-trip equivalence for valid cases.
- Negative: malformed pkt-lines, duplicate capabilities, non-advertised options.

Backout or fallback
- Treat unsupported features as declined; proceed with minimal compatible set.

References
- Upstream mapping: read_head_info(), read_shallow_info(), parse_command() in git-upstream/builtin/receive-pack.c
- Protocol sections: git-upstream/Documentation/gitprotocol-pack.adoc (command list, options, push-options)

## M3: Pack Ingestion & Quarantine

Deliverables
- pack/ingest: streaming reader that accepts the incoming pack and chooses index-pack vs unpack-objects based on size/config.
- Quarantine: create tmp object directory, set alternates to main ODB, export environment for hooks, and migrate on success.
- fsck: integrate gix-fsck with strictness levels; thin-pack fix-ups; size/time guards.
- Error taxonomy: parse → ingest → finalize with stable categories and user-facing mapping.
- Progress: integrate ProgressSink; implement SidebandProgressWriter (blocking); wire ingestion progress to sideband channel 2; add initial KEEPALIVE_AFTER_NUL policy.

Acceptance criteria
- Valid packs are written to quarantine and indexed or unpacked per policy; invalid packs fail with correct error mapping.
- Thin-pack correction handled; quarantined objects are not visible to main ODB until finalization.
- Size limits and timeouts enforce early abort with resource cleanup.
- Progress appears only on channel 2; keepalive frames respect policy; report-status packets unaffected.

Feature-gate checklist
- fsck gating exercised: ingestion works with fsck off and on; document behavior when disabled.

Test plan
- Integration: ingest valid pack; ingest invalid pack (bad CRC, missing base) to assert fsck failure; thin-pack that needs fix-ups.
- Resource: verify quarantine isolation; ensure alternates resolve; ensure cleanup on failure.
- Performance smoke: ensure ingestion is streaming (bounded memory).
- Progress: formatting sanity; sideband wiring test (blocking); ensure fsck on/off behavior does not affect progress ordering.

Backout or fallback
- Fallback to unpack-objects when index-pack fails and configuration permits; disable quarantine via config for emergency scenarios.

References
- Upstream mapping: receive_pack(), read_pack_header(), unpack(), use_keep() in git-upstream/builtin/receive-pack.c
- Protocol sections: git-upstream/Documentation/gitprotocol-pack.adoc (pack transfer, thin-pack)

## M4: Shallow & Connectivity

Deliverables
- shallow/plan: build plan from client-provided shallow/unshallow lines; compute assignment matrices.
- graph/connectivity: pluggable ConnectivityChecker that excludes hidden refs; sideband progress reporting.
- Deferral policy: allow per-ref deferred reachability based on configuration and workload.
- Progress: pass ProgressSink to connectivity checker; add rate-limiting; support cross-thread progress with "parallel" on.

Acceptance criteria
- Shallow boundaries updated correctly; errors for impossible shallow updates mapped to protocol errors.
- Connectivity checks honor hidden refs; sideband progress emitted if negotiated.
- Configurable deferral of per-ref checks without compromising safety.
- Progress visible for connectivity; no protocol packet interference; parallel on/off behaves identically from client perspective.

Feature-gate checklist
- parallel connectivity path implemented with thread-pool auto default; single-thread fallback verified when parallel is disabled.

Test plan
- Unit: shallow plan edge cases (already-shallow, unshallow sequences).
- Integration: connectivity failure paths; sideband progress snapshots.
- Negative: hidden ref exclusion verified; ensure no leakage of hidden objects.
- Progress: parallel vs single-thread progress; throttling; cancellation coexistence if interrupt is enabled.

Backout or fallback
- Disable shallow updates or enforce full connectivity checks if plan cannot be produced reliably.

References
- Upstream mapping: receive_shallow_info(), update_shallow_ref() in git-upstream/builtin/receive-pack.c
- Protocol sections: git-upstream/Documentation/gitprotocol-pack.adoc (shallow)

## M5: Policies, Hooks, Proc-Receive

Deliverables
- policy/set: enforce deny_deletes, deny_non_fast_forwards, deny_current_branch, deny_delete_current; updateInstead flow via gix-worktree.
- hooks: trait model for update, pre-receive, post-receive; environment construction; sideband relay options.
- proc-receive v1: version negotiation and command streaming, mapping responses into command results.

Acceptance criteria
- Policy matrix results in expected allow/deny outcomes with precise reason codes.
- Hook environment matches documented variables; stdout/stderr relayed via sideband if enabled.
- proc-receive transcript adherence for typical workflows; clear error mapping.

Feature-gate checklist
- hooks-external off uses NoopHooks with present-but-disabled behavior; on enables gix-command invocation. Proc-receive gated as planned.

Test plan
- Unit: policy combinations; symref alias validation.
- Integration: hook invocation with environment snapshot; proc-receive IO transcript fixtures.
- Negative: hook failure handling; policy rejections; alias conflicts.

Backout or fallback
- Feature-gate hooks and proc-receive; when disabled, proceed with internal policy only.

References
- Upstream mapping: run_update_hook(), run_receive_hooks(), proc-receive helper calls in git-upstream/builtin/receive-pack.c
- Protocol sections: git-upstream/Documentation/gitprotocol-pack.adoc (proc-receive, hooks expectations)

## M6: Ref Transactions (atomic and non-atomic)

Deliverables
- refs/plan: transaction planner supporting atomic apply and non-atomic delete-then-create phases.
- Rejection mapping: ref-store rejections mapped back to per-command statuses with detailed reasons.
- Idempotency: ensure safe retries and cancellation points around transaction boundaries.

Acceptance criteria
- Atomic transactions commit all or none when supported; non-atomic path maintains consistency and clear failure logs.
- Conflicts are mapped back to the originating command with precise status text for report-status v1/v2.
- Idempotent behavior on retry after transport hiccups.

Feature-gate checklist
- interrupt cancellation points validated around transaction boundaries when interrupt is enabled; deterministic single-thread behavior when parallel is disabled.

Test plan
- Integration: conflict scenarios (non-ff push, locked refs, concurrent updates).
- Unit: planner transforms; alias/symref validation rules.
- Negative: partial failures handled with rollback for non-atomic plans.

Backout or fallback
- Force non-atomic mode if store does not support atomic updates; provide clear warning.

References
- Upstream mapping: execute_commands(), update_ref(), try_atomic_transaction() in git-upstream/builtin/receive-pack.c

## M7: Reporting (V1, V2) + Strict-Compat Adapter

Deliverables
- report/model with per-command and overall results.
- report/v1 and report/v2 writers; sideband progress integration.
- compat/adapter to produce upstream-equivalent text when strict-compat is enabled.
- StrictCompatProgress: adapter matches upstream textual transcripts for progress; report-status remains unchanged.

Acceptance criteria
- Golden transcripts for v1 and v2 reports match expectations; when strict-compat enabled they match upstream exactly for fixtures.
- Errors mapped to human-readable and machine-parseable forms consistently.
- Golden progress transcript parity under strict-compat; default formatter remains idiomatic otherwise.

Feature-gate checklist
- strict-compat formatting gated; default formatter remains idiomatic when strict-compat is disabled. tracing and metrics remain off by default.

Test plan
- Golden: multiple scenario suites, including success, policy rejection, hook failure, connectivity failure.
- Unit: formatter correctness; line wrapping and pkt-line framing under sideband.
- Golden progress transcripts; ensure report-status golden tests stay unchanged.

Backout or fallback
- Disable strict-compat without impacting correctness; default to idiomatic output.

References
- Upstream mapping: receive_status(), report(), report_v2() in git-upstream/builtin/receive-pack.c
- Protocol sections: git-upstream/Documentation/gitprotocol-pack.adoc (report-status, report-status-v2)

## M8: Async Transport & IO Abstraction

Deliverables
- Feature async-io: async IO layer mirroring blocking API using gix-packetline async APIs and a minimal tokio feature set.
- Parity tests: same semantics and wire output between blocking and async.

Acceptance criteria
- All advertisement, parsing, ingestion, hooks, and reporting paths have async equivalents behind a unified public API.
- Cancellation and keepalive behaviors validated; no deadlocks under backpressure.

Feature-gate checklist
- async-io parity with blocking-io; cancellation tests when interrupt is enabled; ensure blocking-io builds do not pull tokio.

Test plan
- Parity: run full test matrix under --features async-io; compare transcripts to blocking mode.
- Stress: cancellation in the middle of pack ingestion; keepalive intervals.

Backout or fallback
- Feature-gate async-io; if unstable on a platform, keep disabled in CI matrix for that platform.

References
- Protocol building blocks: gix-packetline/async and gix-transport async clients
- Upstream parity checked against git CLI over ssh or stdio

## M9: Performance & Observability

Deliverables
- Benchmarks: criterion-based micro and end-to-end for pack ingestion and connectivity.
- Observability: optional tracing with spans; metrics counters for key paths; memory/alloc profiles.

Acceptance criteria
- Performance budgets documented; no regressions beyond threshold across releases.
- Tracing can be enabled without affecting behavior; minimal overhead when disabled.

Feature-gate checklist
- tracing on/off overhead measured; metrics facade on/off impact verified negligible; parallel on/off performance budget documented.

Test plan
- Perf: benchmark suite in CI with guardrails (compare against baseline).
- Resource: track peak memory and allocation counts on representative scenarios.

Backout or fallback
- Disable tracing feature by default; keep benchmarks out of default CI but runnable on demand.

## Cross-Cutting Tracks

Error taxonomy stabilization
- Formalize thiserror-based hierarchy; maintain stable public categories; map to wire messages.

Fuzzing targets
- pkt-line parsing, command list parsing, refname validation.

MSRV and CI matrix
- Pin MSRV aligned with workspace; test combinations of features: default, blocking-io, async-io, parallel, hooks-external, fsck, strict-compat, interrupt, tracing, metrics.

Docs and examples
- Minimal server usage example; integration notes for stdio and TCP hosting; guidance on quarantine and hooks.

Progress Integration
- Blocking-IO first (M3/M4); async-IO counterpart in M8.
- Strict-compat formatting limited to progress adapter only.
- Keepalive policy validated using black-box tests.
- CI: include a matrix job enabling progress+strict-compat alongside previous combinations.

## Deliverables Checklist by Milestone

- M0.5: feature matrix docs, error conventions, CI plan for feature matrix
- M1: protocol/advertise, protocol/capabilities, golden ads, strict-compat adapter for ordering
- M2: protocol/commands, protocol/options, typed push-options, negotiation validation
- M3: pack/ingest, quarantine, fsck integration, size limits, thin-pack fix
- M4: shallow/plan, graph/connectivity with sideband, hidden ref exclusion
- M5: policy/set, hooks, proc-receive v1
- M6: refs/plan, transaction executor, rejection mapping
- M7: report/model, report/v1, report/v2, compat/adapter
- M8: async IO parity and cancellation/keepalive behavior
- M9: benchmarks, tracing, metrics

## Test Strategy Alignment

- Golden transcripts sourced from upstream git-receive-pack for advertisements and reports.
- Integration tests use local repositories; pack fixtures generated via upstream git where applicable.
- Property-based tests: ref policy, refname validation.
- Fuzz: pkt-line parser and command parser.
- Async parity suite mirrors blocking suite.

## Backout and Fallback Principles

- Prefer feature-gating of risky subsystems (async, hooks, proc-receive, strict-compat).
- Provide deterministic idiomatic behavior as baseline; strict-compat opt-in.
- Ingest fallback from index-pack to unpack-objects when permitted.
- Transaction fallback to non-atomic if store capabilities are limited.

## Risk Register (selected)

- Strict-compat drift across upstream versions → mitigate via versioned fixtures.
- Quarantine migration corner cases on concurrent access → mitigate with locking and tests.
- Async differences in backpressure → mitigate with parity tests and timeouts.