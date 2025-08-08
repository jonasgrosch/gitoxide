# gix-receive-pack – Spec-Driven Design

Status: spec-driven; implementation will follow ROADMAP milestones. Scope of this change: documentation only.

1. Purpose & Goals

- Idiomatic Rust-first receive-pack server leveraging gix crates
- Typestate-driven engine; illegal states are unrepresentable
- Two operating modes
  - idiomatic-default with clear errors and deterministic outputs
  - strict-compat (feature) to mirror upstream text/ordering when required
- Clear separation of concerns
  - wire/protocol
  - domain types
  - pack ingestion and quarantine
  - shallow/connectivity
  - ref transactions and policies
  - hooks and proc-receive
  - reporting

Note on crate renaming
- The existing legacy crate will be renamed to gix-receive-pack-deprecated, and this specification applies to the new gix-receive-pack. This task updates documentation only; the actual rename will be executed in a separate code task.

2. External References

- Upstream mapping to receive-pack C (selected functions → target modules)
  - advertise_refs(), send_ref(), send_shallow() → protocol::advertise
  - read_head_info(), parse_command(), read_push_options() → protocol::head_info, protocol::commands, protocol::options
  - receive_shallow_info(), update_shallow_ref() → shallow::plan, shallow::update
  - read_pack_header(), unpack(), use_keep() → pack::ingest::{unpack_objects,index_pack}
  - check_connected(), check_ought_to_succeed() → graph::connectivity
  - try_to_commit_transaction(), update_ref(), execute_commands() → refs::{plan,transaction}
  - run_update_hook(), run_receive_hooks() → hooks::{update,pre_receive,post_receive}
  - proc-receive negotiation/loop → hooks::proc_receive
  - receive_status(), report(), report_v2() → report::{v1,v2}, compat::adapter (strict-compat)
  - Top-level entry receive_pack() and main loop → engine::Session<Phase>
  - Source reference: [git-upstream/builtin/receive-pack.c](git-upstream/builtin/receive-pack.c:1)
- Protocol references (relevant sections in Git docs)
  - Advertisement, capabilities, side-band, stateless-RPC, command list, push-options, shallow, pack transfer, report-status, report-status-v2
  - See [git-upstream/Documentation/gitprotocol-pack.adoc](git-upstream/Documentation/gitprotocol-pack.adoc:1)
- gix crates used (crate → role)
  - [../gix-packetline-blocking/](../gix-packetline-blocking/) → pkt-line IO for blocking mode
  - [../gix-packetline/](../gix-packetline/) → shared pkt-line primitives, async building blocks
  - [../gix-protocol/](../gix-protocol/) → protocol helpers and types
  - [../gix-transport/](../gix-transport/) → IO abstraction (async building blocks)
  - [../gix-pack/](../gix-pack/) → pack decoding, thin-pack fixups, index creation
  - [../gix-odb/](../gix-odb/) → object database, alternates, tmp object directories
  - [../gix-ref/](../gix-ref/) → ref transactions, validation, symrefs
  - [../gix-shallow/](../gix-shallow/) → shallow graph operations
  - [../gix-fsck/](../gix-fsck/) → object verification policies
  - [../gix-command/](../gix-command/) → external hook execution
  - [../gix-worktree/](../gix-worktree/) → updateInstead integration
  - [../gix-config/](../gix-config/) and [../gix-config-value/](../gix-config-value/) → configuration sources
  - [../gix-hash/](../gix-hash/) → ObjectId typed by hash algorithm
  - [../gix-trace/](../gix-trace/) → optional tracing
  
  2.1 Interfaces Overview
  
  - The detailed Rust-style interface specifications are in [gix-receive-pack/INTERFACES.md](gix-receive-pack/INTERFACES.md:1).
  - Key entry points: [rust.Session<Phase>](gix-receive-pack/INTERFACES.md:1), [rust.WireReader](gix-receive-pack/INTERFACES.md:1), [rust.WireWriter](gix-receive-pack/INTERFACES.md:1), [rust.ReportV1](gix-receive-pack/INTERFACES.md:1), [rust.ReportV2](gix-receive-pack/INTERFACES.md:1).
  
  3. Features & Non-Goals

Features
- Typed capabilities and options
- Push-cert handling with pluggable GPG provider
- Quarantine tmp object directory with alternates; migration on success
- Fsck strategies and strictness; size/time limits; thin-pack fix
- Shallow updates and reachability checks
- Atomic and non-atomic refs; alias/symref validation; hidden refs enforcement
- Sideband progress and keepalive
- Stateless-RPC aware framing (blocking first, async parity later)

Non-goals
- Byte-for-byte output parity by default (covered by strict-compat feature)
- Legacy quirks not needed for modern servers unless strict-compat requires them

See 3.1 Feature Flags & Conditional Compilation for feature gates, defaults, and dependency mapping.

3.1 Feature Flags & Conditional Compilation

Overview
- This crate follows gix-wide feature gating patterns to keep the core panic-free and lightweight. Default behavior favors out-of-the-box usability for blocking transports while enabling focused builds for servers and async environments.

Defaults
- default = ["blocking-io"]

Feature Matrix
| Feature         | Default | Dependencies (examples)                                                                 | Primary effect                                                                                                      |
|-----------------|---------|------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------|
| blocking-io     | Yes     | std::io; [../gix-packetline-blocking/](../gix-packetline-blocking/); gix-transport blocking client | Enables blocking pkt-line IO and blocking transports. Avoids pulling async runtimes when async-io is disabled.      |
| async-io        | No      | tokio (minimal); [../gix-packetline/](../gix-packetline/) async-io; [../gix-transport/](../gix-transport/) async-client | Enables async pkt-line IO and async transports. When enabled, disable blocking-io to avoid both IO stacks.          |
| parallel        | No      | [../gix-features/](../gix-features/) feature "parallel"; [../gix-pack/](../gix-pack/) "parallel"; [../gix-odb/](../gix-odb/) "parallel" | Enables multi-threaded operations (e.g., connectivity, pack processing). Engine thread-pool config defaults to auto. |
| progress        | No      | [../gix-features/](../gix-features/) progress (prodash transitively via gix-features)    | Enables internal progress sinks and sideband progress emission; otherwise NoopProgress is used. Interacts with parallel (cross-thread updates) and strict-compat (legacy formatting adapter). |
| hooks-external  | No      | [../gix-command/](../gix-command/)                                                       | Enables external hooks via processes. When disabled, a NoopHooks backend is used and the API is present-but-disabled.|
| fsck            | No      | [../gix-fsck/](../gix-fsck/)                                                             | Validates objects during pack ingest. When disabled, ingest skips fsck steps.                                        |
| strict-compat   | No      | —                                                                                        | Preserves upstream text and field ordering in reports and selected user-facing strings.                              |
| interrupt       | No      | [../gix-features/](../gix-features/) interrupt helpers                                   | Adds cancellation points and error mapping to Kind::Cancelled.                                                       |
| tracing         | No      | [../gix-trace/](../gix-trace/)                                                           | Lightweight spans for observability; zero overhead when feature is disabled.                                         |
| metrics         | No      | — (placeholder abstraction only)                                                         | Optional counters/gauges behind a thin internal facade; external backend not fixed yet.                              |
| serde           | No      | serde with derive                                                                        | Derive Serialize/Deserialize for selected domain types (fixtures/diagnostics). Avoid use on public API types by default. |

Default features policy
- Minimal core: build with --no-default-features plus one of {blocking-io | async-io}; add parallel as desired.
- Server builds: enable parallel and fsck; enable hooks-external only if needed; strict-compat optional depending on parity requirements.

Dependency mapping notes
- async-io → tokio (minimal), gix-transport/async-client, gix-packetline async-io adapters.
- parallel → gix-features/parallel, gix-pack/parallel, gix-odb/parallel (propagated via Cargo features).
- hooks-external → gix-command for process execution and env construction.
- fsck → gix-fsck with strategy selection aligned to receive.fsckObjects/transfer.fsckObjects.
- interrupt → gix-features interrupt helpers.
- tracing → gix-trace minimal span integration.
- serde → serde derives on selected, crate-internal diagnostic types.

Engine thread-pool
- When parallel is enabled, the engine may use a shared thread-pool. Default is auto (use number of logical CPUs). Provide a configuration hook to override if needed in later milestones.

Bundle-size and zero-cost notes
- blocking-io and async-io are mutually exclusive in recommended builds to avoid pulling both stacks. Prefer enabling only one.
- Features are additive and opt-in; the core remains panic-free and strives for zero-cost abstractions when features are disabled.

Cross-reference
- See 13. Configuration for keys and recommended matrices; related sections (7, 10, 15) call out feature-gated behaviors explicitly.

3.2 Progress & Reporting Layering (prodash)

Rationale and ecosystem fit
- Adopt gix-features/progress (prodash) as the single internal progress API, similar to usage patterns in other crates (see [gix-protocol/src/remote_progress.rs](gix-protocol/src/remote_progress.rs)).
- Goal: keep the engine pure; render human-facing progress via a thin adapter to Git sideband while keeping report-status emitters separate and deterministic.

API surface
- Internal concept ProgressSink with operations: start(name, units), step(delta), info(msg), done(), keepalive_tick(), and hierarchical scoping.
- This is an internal abstraction; the public API remains unchanged.

Implementations (conceptual)
- ProdashProgress: backed by gix-features/progress.
- SidebandProgressWriter (blocking and async variants): subscribes to ProgressSink updates, emits to sideband channel 2 exclusively, implements KEEPALIVE_AFTER_NUL and ALWAYS policies.
- StrictCompatProgress (gated by strict-compat): formats progress strings to match upstream receive-pack transcripts without affecting report-status packets.
- NoopProgress: zero-overhead sink for minimal builds/tests or when progress is disabled.

Separation of concerns
- Progress is human-facing and goes over sideband channel 2 (and keepalive frames).
- Report-status (v1/v2) is a separate subsystem responsible for protocol packets; never sourced from progress.

Parallelism
- With "parallel" enabled, allow cross-thread updates to ProgressSink; prodash supports hierarchical progress with low contention.

Keepalive policy
- Adhere to upstream behavior (KEEPALIVE_AFTER_NUL → ALWAYS). The adapter must never interleave or reorder pkt-lines from the report writer; it uses a sideband-only writer with optional throttling.

Testing and compatibility
- Golden-transcript tests for StrictCompatProgress; throttling tests; sideband channel discipline; ensure no drift in report-status.

3.3 Wire IO Design: Thin Adapters vs Direct gix-packetline

Uses from gix-*
- [../gix-packetline-blocking/](../gix-packetline-blocking/:1) for blocking pkt-line operations
- [../gix-packetline/](../gix-packetline/:1) for async pkt-line operations and shared primitives
- Progress integration: [gix-protocol/src/remote_progress.rs](gix-protocol/src/remote_progress.rs:1)

Default approach
- Use gix-packetline reader and writer directly for pkt-line framing and IO in both blocking and async-io modes.

Thin adapters only
- Introduce minimal adapters to:
  - Enforce receive-pack-specific keepalive policy (KEEPALIVE_AFTER_NUL → ALWAYS) and strict sideband channel-2 discipline without changing pkt-line semantics.
  - Bridge progress events from [rust.ProgressSink](gix-protocol/src/remote_progress.rs:1) to sideband channel 2.

Rationale for not wrapping heavily
- Reduces duplication and drift from gix-packetline; leverages battle-tested framing and error handling.
- Keeps zero-cost where possible; adapters focus solely on sideband/keepalive policy and progress integration.
- Preserves separation between progress over sideband and report-status over pkt-line, with report writers using the core pkt-line writer directly.

When wrapping is acceptable
- Provide a single facade toggling between blocking-io and async-io behind feature gates while retaining a uniform API surface.
- Add small interposition layers for strict-compat progress formatting or keepalive discipline; not for pkt-line framing itself.

4. Architecture Overview

Target module map (folders planned)
- engine/
  - Session<Phase> typestate, transitions, orchestration, invariants
- protocol/
  - advertise, capabilities, options, head_info, sideband, stateless
- domain/
  - refname, oid, command types (create/update/delete), capability tokens, push options, policy types
- pack/
  - ingest::{index_pack,unpack_objects}, quarantine, fsck bridge, limits
- refs/
  - plan (atomic vs non-atomic), transactions, alias/symref validation, hidden enforcement
- shallow/
  - plan, update (shallow/unshallow)
- graph/
  - connectivity checkers (pluggable strategy)
- hooks/
  - traits, env construction, external invocation, proc-receive client
- pushcert/
  - nonce, verifier trait (GPG provider)
- policy/
  - deny_* rules, updateInstead integration, hidden predicate
- report/
  - model, v1/v2 formatters, strict-compat adapter
- io/, config/, errors/

Annotated ASCII data flow (blocking; async mirrors via feature)

```
+-------------------+       +----------------------+       +-----------------------+
| Upstream IO (R/W) | <---> | protocol::advertise  |  ---> | engine::Advertised    |
+-------------------+       +----------------------+       +-----------------------+
                                                          |
                                                          v
                                +----------------------+  |
                                | protocol::head_info |--+  parse commands, options,
                                | protocol::options   |      shallow lines
                                +----------------------+ 
                                                          |
                                                          v
+-------------------+       +----------------------+       +-----------------------+
| pack::ingest      |  <--- | engine::CommandsRead |  ---> | engine::PackIngested  |
| (quarantine, fsck)|       +----------------------+       +-----------------------+
| index/unpack      |                   |                                     
+-------------------+                   v                                     
                              +-----------------------+                        
                              | hooks::pre_receive    |                        
                              +-----------------------+                        
                                         |                                    
                                         v                                    
                              +-----------------------+        +-------------+
                              | graph::connectivity   | -----> | sideband    |
                              +-----------------------+        +-------------+
                                         |                                    
                                         v                                    
+-------------------+       +-----------------------+       +---------------------+
| refs::transaction |  <--- | engine::PreReceived   |  ---> | engine::Updated     |
| (atomic / staged) |       +-----------------------+       +---------------------+
+-------------------+                                              |
                                                                  v
                                                        +---------------------+
                                                        | report::{v1,v2}     |
                                                        | compat::adapter     |
                                                        +---------------------+
                                                                  |
                                                                  v
                                                        +---------------------+
                                                        | engine::Reported    |
                                                        +---------------------+
```

5. Typestate Protocol Engine

States (monotonic progression)
- Start → Advertised → CommandsRead → PackIngested → PreReceived → Updated → Reported

Allowed transitions and invariants
- Start → Advertised
  - side effect: write advertisement; invariant: capabilities reflect config and hidden predicate
- Advertised → CommandsRead
  - require: client flush; options subset of advertised; object-format compatible
- CommandsRead → PackIngested
  - require: command list present unless allow-empty; pack expected if any command updates
- PackIngested → PreReceived
  - require: fsck passes; quarantine populated; limits respected
- PreReceived → Updated
  - require: hooks permit; policy set approves; transaction plan built
- Updated → Reported
  - always; produce report v1/v2 based on negotiation

Error handling
- Each transition returns Result<NextState, Error> with stable categories
- Idempotency: Start/Advertised/Reported are safe to re-run under transport retry
- Cancellation: safe abort points at PackIngested and PreReceived with cleanup of quarantine and locks

IO abstraction and features
- Blocking: std::io::Read/Write + [../gix-packetline-blocking/](../gix-packetline-blocking/)
- Async (feature async-io): Tokio-based with parity; [../gix-packetline/](../gix-packetline/) and [../gix-transport/](../gix-transport/) async utilities
- Side-effect injection via traits: time source, hook runner, verifier, connectivity strategy

6. Domain Types & Invariants

- RefName
  - Follows Git refname rules; optional namespace prefix; normalized storage form
- ObjectId<H>
  - Typed by negotiated hash algorithm; consistent across session; conversion guarded
- CommandUpdate
  - Create/update/delete variants; precomputable fast-forward when base reachable; delete has zero new id
- HiddenRefPredicate
  - Closure or trait object applied to advertisement and connectivity; prevents leakage
- PolicySet
  - deny_deletes, deny_non_fast_forwards, deny_current_branch, deny_delete_current, updateInstead
- Options/Capabilities
  - Parsed tokens mapped to typed set; session-id, object-format, push-options list; construction validates preconditions

7. Pack Ingestion & Quarantine

Decision policy
- Prefer index-pack for larger or delta-heavy packs (kept pack); prefer unpack-objects for small updates if configured
Quarantine tmp objdir
- Create tmp ODB; set alternates to main store; export env vars for hooks; migrate or drop on completion
Fsck strategy
- Levels: off → lenient → strict → pedantic; aligned with receive.fsckObjects/transfer.fsckObjects; apply thin-pack fix; size/time guards
Error taxonomy
- Parse (wire/pack) → Ingest (IO/format) → Finalize (migrate/index); map to user-facing protocol messages and internal causes

Feature gating
- When feature fsck is disabled, pack ingest skips object verification steps; thin-pack correction may still apply. For production, enable fsck to restore strict checks aligned with receive.fsckObjects/transfer.fsckObjects.

8. Shallow & Connectivity

Shallow plan
- Parse shallow/unshallow lines; compute assignment matrices; update boundaries
Connectivity checker
- Pluggable; excludes hidden refs; optional sideband progress; can defer per-ref checks when safe for throughput

9. Ref Transactions & Policies

Planner
- Atomic plan when supported; non-atomic staged (delete-then-create) with rollback notes
Policies
- Enforce deny_*; updateInstead delegates to worktree updater; enforce symref/alias validity
Store errors
- Map ref-store rejections back to commands with reason codes suitable for v1/v2 reporting

10. Hooks & Proc-Receive

Hooks
- Traits for update, pre-receive, post-receive; env construction includes quarantine dirs, push-options, session-id
- Sideband relay of stdout/stderr optional
Proc-receive
- Version negotiation; command streaming; map responses into per-command results; gated by feature

Feature gating
- hooks-external controls whether external processes are invoked via gix-command. When disabled, a NoopHooks backend is used and the API remains present-but-disabled to maintain deterministic behavior.

11. Reporting

Report model
- Per-command and overall results with stable codes
Formatters
- v1 and v2 writers; sideband-aware progress
Strict-compat adapter
- Optional matching of upstream text, field ordering, and spacing

12. Error Model

thiserror-based hierarchy
- Categories: Protocol, Pack, Fsck, Policy, Storage, IO, Hook, Config
- Preserve source context; include OIDs and refnames where helpful
Mapping
- Deterministic mapping to wire/reports: e.g., non-ff → non-fast-forward status; hook failure → rejected by hook

12.1 Error Handling Conventions

- Library surface uses thiserror; anyhow is permitted only in binaries and tests.
- Central error type: crate::Error with module-level sub-errors: WireError, PackError, ShallowError, TxError, HookError, PolicyError, ReportError, ConfigError. All implement std::error::Error via thiserror.
- Fast classification helper: Error::kind() returns Kind with stable variants: Io, Protocol, Validation, NotFound, Permission, Cancelled, Resource, Bug, Other. Underlying sources (io errors, pkt-line parse, filesystem) are mapped explicitly to these kinds.
- Rich context via source chaining; Display messages are stable and operator-focused; internal types are not exposed in messages by default.
- No panics for user input; prefer explicit Result returns and early exits; assert or panic only for internal invariants (debug_assert in hot paths).
- Cancellation/interrupt is first-class (Kind::Cancelled) and feature-gated via interrupt; cancellation points are placed at safe boundaries.
- Safety: forbid blanket From&lt;io::Error&gt; into crate::Error; require explicit mapping at boundaries to maintain clarity and consistent Kind values.
13. Configuration

Relevant keys (read from [../git-upstream/Documentation/config/](../git-upstream/Documentation/config/))
- receive.fsckObjects, receive.advertiseAtomic, receive.denyCurrentBranch, receive.denyDeleteCurrent, receive.denyNonFastForwards, receive.quarantineDir, receive.updateInstead
- transfer.fsckObjects, transfer.unpackLimit
Feature flags → runtime toggles
- blocking-io (default), async-io, parallel, hooks-external, fsck, strict-compat, interrupt, tracing, metrics, serde

Default features policy
- default = ["blocking-io"] for out-of-the-box usability.
- Recommended builds:
  - Minimal core: --no-default-features plus one of {blocking-io | async-io}; add parallel as needed.
  - Server: enable parallel and fsck; enable hooks-external only if needed; strict-compat optional.

Defaults
- Safety-oriented defaults; strict-compat off by default; tracing off by default; metrics off by default

14. Testing Strategy

Layers
- Unit: domain rules, parsers, engine transitions
- Integration: protocol IO, pack ingestion with fixtures, connectivity
- Golden: advertisements and reports; strict-compat parity against upstream fixtures
- Fuzz: pkt-line and command parsing
- Property: refname validation, policy matrices
Fixtures and parity
- Capture transcripts from upstream git-receive-pack and git CLI
- Progress: golden progress transcripts under strict-compat mode
- Progress: sideband channel-2 discipline tests (no interference with report-status)
- Progress: throttling and keepalive policy adherence (KEEPALIVE_AFTER_NUL → ALWAYS), including rate limiting behavior

15. Performance & Observability

Benchmarks
- criterion micro and scenario benchmarks (ingestion, connectivity)
Parallelism
- Optional parallel connectivity; cautious pipelining of ingestion vs verification
Observability
- Feature-gated tracing spans; metrics counters for key paths; allocation profiles

Feature flags and overheads
- parallel: enables multi-threading in supported crates; default auto thread count; single-thread fallback when disabled.
- progress: optional feature; minimal overhead when disabled (NoopProgress); orthogonal to tracing/metrics.
- tracing: integrated via gix-trace; disabled by default and compiled out with minimal overhead.
- metrics: placeholder counters/gauges behind a minimal facade; disabled by default; no external backend mandated.

16. Backward Compatibility & Migration

- strict-compat as convergence path; toggles to preserve legacy text formatting when needed
- Deprecation notes if defaults evolve; documented migration

17. Security Considerations

- Input size/time limiters; pkt-line bounds; pack limits
- Quarantine isolation until finalize
- Hook command/env sanitization; restrict inherited env; path controls for GPG verifier

18. Open Questions

- Async executor scope and surfaces under feature async-io
- Workspace-wide MSRV alignment and policy
- Heuristics and default thresholds for index-pack vs unpack-objects
- Extent of strict-compat text parity across upstream versions and how to version fixtures
- Default for the "progress" feature (opt-in vs implicit via IO features), and whether to expose an engine-level verbosity knob