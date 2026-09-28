# Managed SCM transfer lifetime

Status: implementation approved for capability-bounded v1; production adoption
remains gated. Native primitives are tested; production integration is unproved.

Native feasibility checkpoint (2026-09-18): delayed protected import followed
by lease retirement and positive exchange passes; carrier close and sender/
receiver death retirement pass. Native PEEK returns zero control integers, not
new FDs: queue-retention passes, but guest peek semantics remain unimplemented.
Zero/undersized native control and plain native read leave two undisclosed FDs
and the guardian open in isolated tests. Full native control intake followed by
explicit manual discard passes10 separate processes with balanced FD counts;
post-retirement positive exchange also passes10. Adoption remains withheld:
all consuming paths need full private ancillary intake. V1 deliberately has no
receive cache and rejects managed ancillary MSG_PEEK with genuine EOPNOTSUPP
before consumption. Adoption requires an actual-operation flags audit showing
the unchanged acceptance APKs do not require that operation. Observed required
peek blocks adoption until its shared-alias contract is implemented; it cannot
be waived, delegated to native peek, or special-cased by application identity.

## V1 capability boundary

This is APK-scoped compatibility, not full Android/POSIX SCM support. Kernel
socket ownership retains byte ordering, readiness, aliases and concurrent reads;
no process-local or distributed receive cache is introduced. Every managed
read/readv/recv/recvfrom/recvmsg consuming path must use full native ancillary
intake and deliberate descriptor discard/publication. Unmanaged carriers retain
their existing contract. Capability rejection must leave queued bytes and rights
untouched, verified by subsequent successful consumption.

## Evidence and scope

The 2026-09-18 Root-channel checkpoint in architecture-migration.md joins an
unchanged Chromium Root failure to native descriptor transfer. Its reciprocal
socket was exported at state258 and received in the GPU at state290, before
broker import. Receiver-side duplication cannot repair that failure. Native
private tests separately reproduce loss after last sender close, while retaining
an alias until actual receiver ownership preserves positive-byte exchange.

This decision concerns the Unix descriptor-transfer provider, not Android Binder
policy, Chrome lifecycle, GPU selection, or app-specific rendering behavior.
Android guest payload bytes, descriptor ordering, syscall results, and ancillary
semantics must remain authoritative. There is no acceptance exception for probes.

## Proposed provider boundary

Evaluate a private ancillary envelope on explicitly provider-managed carriers:

`[metadata-file FD, guardian-write FD, guest payload FDs…]`

A narrow Rust `ScmTransferLeaseOwner` owns retained payload aliases and the
guardian-read endpoint. It is separate from Binder routing and Android policy.
The C++ Android ABI adapter translates control layouts and calls this owner;
extract its mixed SCM responsibilities before extending the oversized adapter.
Do not add another generic Bridge or a textual include split.

Prepare authenticates process/carrier incarnation and reserves bounded capacity.
The sender keeps payload aliases throughout deposit; only a genuine installed-
ownership response authorizes enqueue. Metadata describes protocol version,
authority incarnation, transfer ticket/generation, carrier direction and count.
Validate against authoritative state, never file magic or kernel socket identity.
Managed-carrier identity must survive dup and interprocess import. Unmanaged
native consumers must never receive the private descriptors as guest payload.

Carrier propagation uses a typed owned-FD bundle with bounded opaque provider
attributes in the private Binder TransferImage FD manifest. Binder routing
transports them without interpreting carrier policy; Parcel bytes, FD object
ordering and guest FD count stay unchanged. Export must bind the exact bundle
and ordinal to the authenticated source session/transfer token before capture;
import validates that grant before publishing a managed endpoint. Endpoint side
survives aliases/import; the holder PID is not the carrier identity. A bare-FD
escape cannot silently become unmanaged. Initial Binder native FD delivery also
needs retained aliases until genuine acquisition/discard, not its preceding
TAKE response; metadata alone does not repair the lifetime gap.

That bootstrap gap belongs to a separate `HostFdDeliveryOwner`, not Binder
routing or the direct-SCM carrier state machine. On the dedicated private TAKE
connection, offer/acquired/admitted frames bind daemon epoch, ticket and count;
the native group carries guardian writer, existing image and existing payload
FDs. Receiver strips the guardian, retains it through image validation and fresh
same-epoch admission, then closes it. The owner survives handlers and retires
only on guardian EOF, with global/per-destination delivery AND retained-FD
quotas. Native254-right capacity leaves252 guest payload FDs after guardian and
image overhead. Partial-positive send never retransmits rights. These are
private host frames, not guest socket or Parcel acknowledgments.

Authenticated destination lifetime must bind the connection's native audit
token process version to the live kernel process before using its registered
birth identity. A current birth lookup of a numeric peer PID alone is not that
proof; native credential verification remains a narrow Darwin provider.

The Rust owner has one immutable fresh daemon AuthorityEpoch, distinct from
authenticated sender/receiver ProcessEpochs. It allocates non-reused carrier and
transfer serials within that epoch. Sender quotas use ProcessEpoch across
reconnects. Import validates the same carrier at the opposite endpoint side and
admits legitimate cross-process holders. Sender death prevents new operations
but does not revoke already queued transfers. Receiver keeps its guardian FD
locally through fresh admission and transactional publication/discard; admission
does not transfer that FD back to the daemon and is not lease retirement.

The receiver removes private descriptors, acquires payload ownership, validates
the same live authority incarnation, and only then publishes guest descriptors
and closes its guardian writer. Native last-writer EOF retires the retained
aliases exactly once. No guest ACK bytes, SO/FIFO inferred receipt, fixed delay,
or permanent extra descriptor is permitted.

## Lifecycle and failure requirements

- Queue references must keep the guardian writer alive until consume/discard.
- Unsupported managed peek must reject before consuming the queued writer.
- Metadata, guardian and payload use one native rights group; independently
  dropping the guardian while payload remains deliverable would invalidate this
  mechanism and must be tested.
- Preserve partial positive send results; do not automatically resend rights.
- Receive enough private control storage, then deliberately translate guest zero
  capacity/truncation. Resolve native batch limits and envelope overhead.
- Plain reads may discard ancillary rights normally. Cover every managed
  recvmsg/native/direct-syscall consumer, concurrent access, and carrier aliases.
- Prepared and queued transfers count toward process/global limits. Apply
  backpressure before enqueue. Unread messages retain bounded ownership.
- CancelUnsent resolves a never-queued transfer terminally. Graceful cancellation
  terminates a carrier before releasing outstanding leases.
- Hard authority death cannot promise cancellation before kernel FD teardown.
  Require fresh post-import admission against the same incarnation; discard and
  terminally reject the managed carrier on authority loss. Do not claim survival
  across authority death or return damaged FDs as successful imports.

## Approval and acceptance

Darwin non-atomic FD creation/intake-to-CLOEXEC windows and owned child spawn
must use the same process-local inheritance guard. Never acquire it after fork,
or hold it while waiting for socket input, registration, or process completion.
Separate native/Rust libraries must explicitly share that boundary; duplicate
mutex instances do not establish consumer closure.

First prove delayed/normal consume, peek rejection without consumption, control truncation/discard,
plain-read discard, concurrent operations, sender/receiver/authority death,
capacity exhaustion, and exact retirement. Verify positive-byte exchange after
guardian retirement and bounded FD counts. Test the actual adapter translation
units and managed identity propagation, not just a standalone owner.

Only then adopt the protocol in production and re-test the correlated Root
handoff. Final Chrome acceptance is visible Example Domain heading/body with
physical input/reload and a fresh Retina capture through the unchanged
Graphite/Dawn → Vulkan → MoltenVK → Metal path. The wider Probe-removal goal
retains its source/link closure, both-flavor, warm-build and other APK gates.

This follows the Wine-style guest-contract/host-provider boundary. The private
transport is a Darwin adaptation, not copied Android policy. If the kernel
guardian invariants or complete managed-consumer coverage fail, reject this
proposal rather than enabling a partial compatibility workaround.
