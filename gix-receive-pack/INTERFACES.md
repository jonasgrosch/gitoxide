# gix-receive-pack – Interface Specifications

Status: documentation-only. All items below are Rust-style interface specs; not implemented here.

Linking policy
- Every Rust language construct is linked as [rust.Name](gix-receive-pack/INTERFACES.md:1) or [rust.Name::method()](gix-receive-pack/INTERFACES.md:1).
- References into this repository use the project’s file-link format, e.g. [gix-pack/src/index/write/mod.rs](gix-pack/src/index/write/mod.rs:1), [rust.remote_progress](gix-protocol/src/remote_progress.rs:1), [rust.capabilities](gix-transport/src/client/capabilities.rs:1).
- For module paths or std items without a specific file target, constructs are made clickable and anchored to this file at line 1.

Commonly referenced standard items
- [rust.Result](gix-receive-pack/INTERFACES.md:1), [rust.Option](gix-receive-pack/INTERFACES.md:1), [rust.Vec](gix-receive-pack/INTERFACES.md:1), [rust.String](gix-receive-pack/INTERFACES.md:1), [rust.PathBuf](gix-receive-pack/INTERFACES.md:1), [rust.bool](gix-receive-pack/INTERFACES.md:1), [rust.u8](gix-receive-pack/INTERFACES.md:1), [rust.u64](gix-receive-pack/INTERFACES.md:1), [rust.Read](gix-receive-pack/INTERFACES.md:1), [rust.Write](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)

Feature gates used across interfaces
- blocking-io, async-io, parallel, hooks-external, fsck, strict-compat, interrupt, progress, tracing


1) Engine typestate

Preamble: Uses from gix-*
- [rust.capabilities](gix-transport/src/client/capabilities.rs:1)
- [rust.ProgressSink](gix-protocol/src/remote_progress.rs:1), [rust.remote_progress](gix-protocol/src/remote_progress.rs:1)
- Object identifiers via [rust.gix_hash::ObjectId](gix-receive-pack/INTERFACES.md:1)
- Interrupt helpers via [rust.gix_features::interrupt](gix-receive-pack/INTERFACES.md:1)

Interfaces
- [rust.Session<'sess, Phase, IO, Store, Hooks, Verifier, Conn, PS>](gix-receive-pack/INTERFACES.md:1)
  - Typestate phases: [rust.Start](gix-receive-pack/INTERFACES.md:1), [rust.Advertised](gix-receive-pack/INTERFACES.md:1), [rust.CommandsRead](gix-receive-pack/INTERFACES.md:1), [rust.PackIngested](gix-receive-pack/INTERFACES.md:1), [rust.PreReceived](gix-receive-pack/INTERFACES.md:1), [rust.Updated](gix-receive-pack/INTERFACES.md:1), [rust.Reported](gix-receive-pack/INTERFACES.md:1)
  - Generics
    - IO: requires a paired [rust.WireReader](gix-receive-pack/INTERFACES.md:1) and [rust.WireWriter](gix-receive-pack/INTERFACES.md:1)
    - Store: [rust.gix_odb::Handle](gix-receive-pack/INTERFACES.md:1)
    - Hooks: [rust.Hooks](gix-receive-pack/INTERFACES.md:1)
    - Verifier: [rust.GpgVerifier](gix-receive-pack/INTERFACES.md:1)
    - Conn: [rust.ConnectivityChecker](gix-receive-pack/INTERFACES.md:1)
    - PS: [rust.ProgressSink](gix-receive-pack/INTERFACES.md:1)
  - Methods (each returns [rust.Result](gix-receive-pack/INTERFACES.md:1)<Next, [rust.Error](gix-receive-pack/INTERFACES.md:1)>)
    - [rust.Session<Start>::advertise()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.Session](gix-receive-pack/INTERFACES.md:1)<[rust.Advertised](gix-receive-pack/INTERFACES.md:1), ...>, [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Session<Advertised>::read_commands()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.Session](gix-receive-pack/INTERFACES.md:1)<[rust.CommandsRead](gix-receive-pack/INTERFACES.md:1), ...>, [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Session<CommandsRead>::ingest_pack()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.Session](gix-receive-pack/INTERFACES.md:1)<[rust.PackIngested](gix-receive-pack/INTERFACES.md:1), ...>, [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Session<PackIngested>::pre_receive()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.Session](gix-receive-pack/INTERFACES.md:1)<[rust.PreReceived](gix-receive-pack/INTERFACES.md:1), ...>, [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Session<PreReceived>::update_refs()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.Session](gix-receive-pack/INTERFACES.md:1)<[rust.Updated](gix-receive-pack/INTERFACES.md:1), ...>, [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Session<Updated>::report()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.Session](gix-receive-pack/INTERFACES.md:1)<[rust.Reported](gix-receive-pack/INTERFACES.md:1), ...>, [rust.Error](gix-receive-pack/INTERFACES.md:1)>
  - Notes
    - Cancellation: transitions may return [rust.Error](gix-receive-pack/INTERFACES.md:1) with [rust.Kind::Cancelled](gix-receive-pack/INTERFACES.md:1) when [rust.interrupt](gix-receive-pack/INTERFACES.md:1) is enabled.
    - IO: mutually exclusive [rust.blocking-io](gix-receive-pack/INTERFACES.md:1) vs [rust.async-io](gix-receive-pack/INTERFACES.md:1).
    - Progress: injected via [rust.ProgressSink](gix-receive-pack/INTERFACES.md:1); strictly separated from [rust.ReportModel](gix-receive-pack/INTERFACES.md:1).


2) Protocol IO (wire)

Preamble: Uses from gix-*
- [../gix-packetline-blocking/](../gix-packetline-blocking/:1) for blocking pkt-line
- [../gix-packetline/](../gix-packetline/:1) async-io adapters
- [../gix-transport/](../gix-transport/:1) client abstractions

Design note
- [rust.WireReader](gix-receive-pack/INTERFACES.md:1) and [rust.WireWriter](gix-receive-pack/INTERFACES.md:1) are aliases or thin adapters around gix-packetline reader and writer, adding only receive-pack-specific keepalive and sideband policy. They do not reimplement pkt-line semantics.

Interfaces
- [rust.WireReader<'a, R>](gix-receive-pack/INTERFACES.md:1) where R: [rust.Read](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.WireReader::read_pkt_line()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.Option](gix-receive-pack/INTERFACES.md:1)<[rust.Vec](gix-receive-pack/INTERFACES.md:1)<[rust.u8](gix-receive-pack/INTERFACES.md:1)>>, [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.WireReader::read_sideband()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.SidebandFrame](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
- [rust.WireWriter<'a, W>](gix-receive-pack/INTERFACES.md:1) where W: [rust.Write](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.WireWriter::write_pkt_line()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.WireWriter::flush()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.WireWriter::write_sideband()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
- Feature notes
  - [rust.blocking-io](gix-receive-pack/INTERFACES.md:1) uses gix-packetline-blocking readers/writers.
  - [rust.async-io](gix-receive-pack/INTERFACES.md:1) uses gix-packetline async traits.


3) Advertisement

Preamble: Uses from gix-*
- [rust.capabilities](gix-transport/src/client/capabilities.rs:1)
- Upstream mapping: [git-upstream/builtin/receive-pack.c](git-upstream/builtin/receive-pack.c:1)

Interfaces
- [rust.Advertiser<W>](gix-receive-pack/INTERFACES.md:1) where W: [rust.Write](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.Advertiser::enumerate_refs()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.Vec](gix-receive-pack/INTERFACES.md:1)<[rust.RefRecord](gix-receive-pack/INTERFACES.md:1)>, [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Advertiser::build_capabilities()](gix-receive-pack/INTERFACES.md:1) → [rust.CapabilityLine](gix-receive-pack/INTERFACES.md:1)
    - [rust.Advertiser::write_advertisement()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
  - Types
    - [rust.RefRecord](gix-receive-pack/INTERFACES.md:1), [rust.CapabilityLine](gix-receive-pack/INTERFACES.md:1), [rust.HiddenRefPredicate](gix-receive-pack/INTERFACES.md:1)


4) Command parsing and options

Preamble: Uses from gix-*
- Upstream mapping: [git-upstream/builtin/receive-pack.c](git-upstream/builtin/receive-pack.c:1)
- [rust.gix_hash::ObjectId](gix-receive-pack/INTERFACES.md:1)

Interfaces
- [rust.CommandUpdate](gix-receive-pack/INTERFACES.md:1)
  - Variants: [rust.Create](gix-receive-pack/INTERFACES.md:1), [rust.Update](gix-receive-pack/INTERFACES.md:1), [rust.Delete](gix-receive-pack/INTERFACES.md:1)
  - Fields: old: [rust.ObjectId](gix-receive-pack/INTERFACES.md:1), new: [rust.ObjectId](gix-receive-pack/INTERFACES.md:1), name: [rust.RefName](gix-receive-pack/INTERFACES.md:1)
- [rust.CommandList](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.CommandList::parse_from_wire()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.CommandList](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.CommandList::iter()](gix-receive-pack/INTERFACES.md:1) → [rust.impl Iterator](gix-receive-pack/INTERFACES.md:1)<Item = &[rust.CommandUpdate](gix-receive-pack/INTERFACES.md:1)>
- [rust.Options](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.Options::parse()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.Options](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Options::validate_against()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>


5) Pack ingestion & quarantine

Preamble: Uses from gix-*
- [gix-pack/src/bundle/write/mod.rs](gix-pack/src/bundle/write/mod.rs:1)
- [gix-pack/src/bundle/write/types.rs](gix-pack/src/bundle/write/types.rs:1)
- [gix-pack/src/index/write/mod.rs](gix-pack/src/index/write/mod.rs:1)
- [rust.gix_odb](gix-receive-pack/INTERFACES.md:1), [rust.gix_object](gix-receive-pack/INTERFACES.md:1), [rust.gix_fsck](gix-receive-pack/INTERFACES.md:1)

Interfaces
- [rust.PackIngestor](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.PackIngestor::index_pack()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.IndexOutcome](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.PackIngestor::unpack_objects()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.UnpackOutcome](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
- Helpers
  - [rust.UnpackObjects](gix-receive-pack/INTERFACES.md:1), [rust.IndexPack](gix-receive-pack/INTERFACES.md:1)
- [rust.Quarantine](gix-receive-pack/INTERFACES.md:1)
  - Fields: tmp_odb: [rust.gix_odb::Handle](gix-receive-pack/INTERFACES.md:1), alternates: [rust.PathBuf](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.Quarantine::activate()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Quarantine::migrate_on_success()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Quarantine::drop_on_failure()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
- Features
  - [rust.fsck](gix-receive-pack/INTERFACES.md:1) enables object verification strategies.


6) Shallow & connectivity

Preamble: Uses from gix-*
- [rust.gix_shallow](gix-receive-pack/INTERFACES.md:1), [rust.gix_negotiate](gix-receive-pack/INTERFACES.md:1), [rust.gix_traverse](gix-receive-pack/INTERFACES.md:1)

Interfaces
- [rust.ShallowPlan](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.ShallowPlan::from_lines()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.ShallowPlan](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
- [rust.ShallowUpdate](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.ShallowUpdate::apply()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
- [rust.ConnectivityChecker](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.ConnectivityChecker::check()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>


7) Refs & transactions

Preamble: Uses from gix-*
- [rust.gix_ref::transaction](gix-receive-pack/INTERFACES.md:1), [rust.gix_odb](gix-receive-pack/INTERFACES.md:1)

Interfaces
- [rust.RefTransactionPlanner](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.RefTransactionPlanner::plan()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.RefTransaction](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
- [rust.RefTransaction](gix-receive-pack/INTERFACES.md:1)
  - Modes: [rust.atomic](gix-receive-pack/INTERFACES.md:1), [rust.non-atomic](gix-receive-pack/INTERFACES.md:1) staged
  - Methods
    - [rust.RefTransaction::prepare()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.RefTransaction::commit()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.RefTransaction::rollback()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>


8) Policies

Preamble: Uses from gix-*
- [rust.gix_config](gix-receive-pack/INTERFACES.md:1), [rust.gix_revision](gix-receive-pack/INTERFACES.md:1), [rust.gix_merge](gix-receive-pack/INTERFACES.md:1)

Interfaces
- [rust.PolicySet](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.PolicySet::deny_deletes()](gix-receive-pack/INTERFACES.md:1) → [rust.bool](gix-receive-pack/INTERFACES.md:1)
    - [rust.PolicySet::deny_non_fast_forwards()](gix-receive-pack/INTERFACES.md:1) → [rust.bool](gix-receive-pack/INTERFACES.md:1)
    - [rust.PolicySet::current_branch()](gix-receive-pack/INTERFACES.md:1) → [rust.Policy](gix-receive-pack/INTERFACES.md:1)
    - [rust.PolicySet::delete_current()](gix-receive-pack/INTERFACES.md:1) → [rust.Policy](gix-receive-pack/INTERFACES.md:1)
    - [rust.PolicySet::update_instead()](gix-receive-pack/INTERFACES.md:1) → [rust.bool](gix-receive-pack/INTERFACES.md:1)
  - Evaluators
    - [rust.PolicySet::evaluate()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>


9) Worktree updateInstead

Preamble: Uses from gix-*
- [rust.gix_worktree](gix-receive-pack/INTERFACES.md:1), [rust.gix_index](gix-receive-pack/INTERFACES.md:1), [rust.gix_diff](gix-receive-pack/INTERFACES.md:1), [rust.gix_command](gix-receive-pack/INTERFACES.md:1)

Interfaces
- [rust.WorktreeUpdater](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.WorktreeUpdater::apply()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
  - Notes
    - May shell-out via [rust.gix_command](gix-receive-pack/INTERFACES.md:1) under [rust.strict-compat](gix-receive-pack/INTERFACES.md:1).


10) Hooks & proc-receive

Preamble: Uses from gix-*
- [rust.gix_command](gix-receive-pack/INTERFACES.md:1)

Interfaces
- [rust.Hooks](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.Hooks::update()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.HookDecision](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Hooks::pre_receive()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.HookDecision](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.Hooks::post_receive()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
- Implementations
  - [rust.ExternalHooks](gix-receive-pack/INTERFACES.md:1), [rust.NoopHooks](gix-receive-pack/INTERFACES.md:1)
- [rust.ProcReceive](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.ProcReceive::negotiate()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
    - [rust.ProcReceive::stream_results()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<[rust.ProcOutcome](gix-receive-pack/INTERFACES.md:1), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
- Features
  - [rust.hooks-external](gix-receive-pack/INTERFACES.md:1) controls external invocation; API present with [rust.NoopHooks](gix-receive-pack/INTERFACES.md:1) otherwise.


11) Push certificates

Preamble: Uses from gix-*
- [rust.gix_hash](gix-receive-pack/INTERFACES.md:1), [rust.gix_object](gix-receive-pack/INTERFACES.md:1), [rust.gix_command](gix-receive-pack/INTERFACES.md:1)

Interfaces
- [rust.PushCert](gix-receive-pack/INTERFACES.md:1)
  - Fields: signer: [rust.String](gix-receive-pack/INTERFACES.md:1), key: [rust.String](gix-receive-pack/INTERFACES.md:1), nonce: [rust.Nonce](gix-receive-pack/INTERFACES.md:1), payload: [rust.Vec](gix-receive-pack/INTERFACES.md:1)<[rust.u8](gix-receive-pack/INTERFACES.md:1)>, signature: [rust.Vec](gix-receive-pack/INTERFACES.md:1)<[rust.u8](gix-receive-pack/INTERFACES.md:1)>
- [rust.Nonce](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.Nonce::verify()](gix-receive-pack/INTERFACES.md:1) → [rust.bool](gix-receive-pack/INTERFACES.md:1)
- [rust.GpgVerifier](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.GpgVerifier::verify()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>


12) Reporting (status v1/v2)

Preamble: Uses from gix-*
- Upstream mapping: [git-upstream/builtin/receive-pack.c](git-upstream/builtin/receive-pack.c:1)
- Pkt-line writers: [../gix-packetline-blocking/](../gix-packetline-blocking/:1), [../gix-packetline/](../gix-packetline/:1)

Interfaces
- [rust.ReportModel](gix-receive-pack/INTERFACES.md:1)
  - Fields: per_command: [rust.Vec](gix-receive-pack/INTERFACES.md:1)<[rust.CommandResult](gix-receive-pack/INTERFACES.md:1)>, overall: [rust.OverallStatus](gix-receive-pack/INTERFACES.md:1)
- Writers
  - [rust.ReportV1](gix-receive-pack/INTERFACES.md:1)
    - [rust.ReportV1::write()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
  - [rust.ReportV2](gix-receive-pack/INTERFACES.md:1)
    - [rust.ReportV2::write()](gix-receive-pack/INTERFACES.md:1) → [rust.Result](gix-receive-pack/INTERFACES.md:1)<(), [rust.Error](gix-receive-pack/INTERFACES.md:1)>
  - [rust.StrictCompatReport](gix-receive-pack/INTERFACES.md:1) under [rust.strict-compat](gix-receive-pack/INTERFACES.md:1)
- Separation rule
  - Progress lines over sideband only; report writers never source progress.


13) Progress adapters

Preamble: Uses from gix-*
- [rust.remote_progress](gix-protocol/src/remote_progress.rs:1)

Interfaces
- [rust.ProgressSink](gix-receive-pack/INTERFACES.md:1)
  - Methods
    - [rust.ProgressSink::start()](gix-receive-pack/INTERFACES.md:1)
    - [rust.ProgressSink::step()](gix-receive-pack/INTERFACES.md:1)
    - [rust.ProgressSink::info()](gix-receive-pack/INTERFACES.md:1)
    - [rust.ProgressSink::done()](gix-receive-pack/INTERFACES.md:1)
    - [rust.ProgressSink::keepalive_tick()](gix-receive-pack/INTERFACES.md:1)
- Implementations
  - [rust.ProdashProgress](gix-receive-pack/INTERFACES.md:1)
  - [rust.SidebandProgressWriter](gix-receive-pack/INTERFACES.md:1)
  - [rust.StrictCompatProgress](gix-receive-pack/INTERFACES.md:1) under [rust.strict-compat](gix-receive-pack/INTERFACES.md:1)
  - [rust.NoopProgress](gix-receive-pack/INTERFACES.md:1)
- Feature notes
  - [rust.progress](gix-receive-pack/INTERFACES.md:1) enables sinks; otherwise [rust.NoopProgress](gix-receive-pack/INTERFACES.md:1).


14) Error model

Preamble: Uses from gix-*
- IO/parse mapping aligned with pkt-line and pack readers: [../gix-packetline-blocking/](../gix-packetline-blocking/:1), [../gix-packetline/](../gix-packetline/:1), [../gix-pack/](../gix-pack/:1)

Interfaces
- [rust.Error](gix-receive-pack/INTERFACES.md:1), [rust.Kind](gix-receive-pack/INTERFACES.md:1)
  - [rust.Kind](gix-receive-pack/INTERFACES.md:1) variants: [rust.Io](gix-receive-pack/INTERFACES.md:1), [rust.Protocol](gix-receive-pack/INTERFACES.md:1), [rust.Validation](gix-receive-pack/INTERFACES.md:1), [rust.NotFound](gix-receive-pack/INTERFACES.md:1), [rust.Permission](gix-receive-pack/INTERFACES.md:1), [rust.Cancelled](gix-receive-pack/INTERFACES.md:1), [rust.Resource](gix-receive-pack/INTERFACES.md:1), [rust.Bug](gix-receive-pack/INTERFACES.md:1), [rust.Other](gix-receive-pack/INTERFACES.md:1)
- Module-specific enums
  - [rust.WireError](gix-receive-pack/INTERFACES.md:1), [rust.PackError](gix-receive-pack/INTERFACES.md:1), [rust.ShallowError](gix-receive-pack/INTERFACES.md:1), [rust.TxError](gix-receive-pack/INTERFACES.md:1), [rust.HookError](gix-receive-pack/INTERFACES.md:1), [rust.PolicyError](gix-receive-pack/INTERFACES.md:1), [rust.ReportError](gix-receive-pack/INTERFACES.md:1), [rust.ConfigError](gix-receive-pack/INTERFACES.md:1)
- Mapping rules
  - pkt-line parse errors map to [rust.Kind::Protocol](gix-receive-pack/INTERFACES.md:1)
  - filesystem and IO map to [rust.Kind::Io](gix-receive-pack/INTERFACES.md:1)
  - user cancellation to [rust.Kind::Cancelled](gix-receive-pack/INTERFACES.md:1)

Appendix: Additional concrete references
- [git-upstream/builtin/receive-pack.c](git-upstream/builtin/receive-pack.c:1)
- [gix-pack/src/bundle/write/mod.rs](gix-pack/src/bundle/write/mod.rs:1)
- [gix-pack/src/index/write/mod.rs](gix-pack/src/index/write/mod.rs:1)
- [gix-pack/src/bundle/write/types.rs](gix-pack/src/bundle/write/types.rs:1)
- [gix-protocol/src/remote_progress.rs](gix-protocol/src/remote_progress.rs:1)
- [gix-transport/src/client/capabilities.rs](gix-transport/src/client/capabilities.rs:1)