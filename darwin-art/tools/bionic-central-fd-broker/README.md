# Bionic central descriptor broker

This standalone gate specifies the one process-wide guest descriptor namespace
that the AIM Bionic providers must eventually share. It does not modify
or integrate the current filesystem, stdio, socket, sendfile, or ioctl providers.

Each published descriptor is a positive 31-bit token with a broker marker, a
20-bit generation, and a 10-bit slot. Closing increments the generation before
the slot is reused; generation exhaustion permanently retires the slot. A stale
token therefore cannot name a later object in the same slot. These tokens never
contain or expose a Darwin descriptor.

Run the standalone acceptance gate with:

```sh
./tools/bionic-central-fd-broker/audit.sh
```

## C ABI and ownership contract

- A provider installs an ABI-v3 callback table and receives an opaque owner
  handle. Prefix-sized ABI-v1 and ABI-v2 tables remain accepted without reading
  or invoking absent offset or socket-operation callbacks. Multiple owners,
  including multiple owners of the same descriptor kind, share the central slot
  allocator without overlapping token ranges.
- The provider kinds are filesystem file, filesystem random device, stdio, and
  socket; epoll is an internal broker-owned kind. A published slot records both
  the exact owner instance and kind; typed operations reject a foreign owner or
  wrong kind before invoking a callback.
- A slot points to a refcounted open-file description. `dup`,
  `duplicate_with_flags`, and `F_DUPFD_CLOEXEC` allocate distinct
  generation-tagged slots referencing that same description. File status flags
  and offset are shared and serialized across all references; `FD_CLOEXEC` is
  per descriptor. Plain dup clears it, while `duplicate_with_flags(CLOEXEC)`
  and F_DUPFD_CLOEXEC set it. F_DUPFD_CLOEXEC selects the numerically lowest
  available encoded token at or above the requested nonnegative minimum. The
  owner close callback runs exactly once after the last descriptor and active
  lease.
- Read, write, poll, ioctl, and sendfile dispatch borrow a generation-checked
  lease. Callbacks execute without the broker mutex. Close atomically marks the
  slot closing, rejects new leases, waits for every active lease, invokes the
  owner close callback, and only then advances the generation and makes the slot
  reusable.
- Sendfile acquires input and output leases atomically. Its input must be a
  filesystem file and its output must be a filesystem file or socket. The
  standalone copy loop demonstrates cross-owner dispatch; production offset,
  partial-write, and signal behavior remains owned by the existing sendfile/FS
  semantic boundary.
- Poll ignores negative descriptors and reports Android `POLLNVAL` (`0x20`) for
  invalid or stale positive tokens. A mixed poll set is routed entry-by-entry to
  the recorded owners; no provider probes another provider's private range.
- `epoll_create1` creates a broker-owned descriptor. `epoll_ctl` supports
  ADD/MOD/DEL for socket descriptions, rejects a duplicate registration key,
  and stores a weak OFD reference plus event data. Distinct dup tokens may be
  registered separately, and readiness remains attached to the OFD after one
  token closes. `epoll_wait(..., timeout=0)` fans out to owner poll callbacks;
  blocking waits use one owner `poll_many` transaction when every watched OFD
  shares that owner (the Android eventfd/timerfd/pipe case). Mixed-owner
  blocking waits fail closed until the broker gains a cross-owner wake queue.
- ABI-v3 adds one tagged socket-operation callback covering bind, connect,
  listen, shutdown, send, recv, sendto, recvfrom, get/setsockopt,
  getpeername, getsockname, and accept4. The fixed-width request explicitly
  separates input address length from output address capacity and actual or
  required length, and does the same for option output. The broker checks the
  exact generation, socket kind, and expected owner before invoking it. The
  callback runs without the broker mutex while a descriptor, OFD, and owner
  lease prevents close or uninstall from invalidating its object. Partial I/O
  and failure errno are preserved through the ordinary I/O result; no callback
  or API unwraps a host descriptor. On a negative accept callback return,
  ownership has not transferred and the broker ignores all accept-result bytes;
  the provider remains responsible for any object it may have created.
- A successful accept callback transfers one opaque child object and its kind
  and flags to the broker. While the parent lease is still held, the broker
  either publishes a fresh same-owner socket token under its mutex or calls the
  same owner's close callback exactly once to roll the child back. Invalid child
  metadata, owner draining, and slot exhaustion never expose an object or guest
  token. The rollback callback is also covered by the owner lease.
- Owner uninstall first enters draining state, which rejects publication. It
  returns busy while any live descriptor, active lease, or close callback
  remains. Only a later quiescent call removes the owner. Broker destruction is
  likewise rejected until all owners are uninstalled.

The host probe covers four provider kinds and simultaneous owners, including
three file owners and prefix-sized ABI-v1 and ABI-v2 owners; namespace collision freedom;
foreign-owner and wrong-kind rejection;
double close; stale access; same-slot generation-safe reuse; close racing a
blocked read; uninstall racing a blocked close callback; typed ioctl; mixed
poll; shared dup offset/status versus independent descriptor flags; last-close
refcounting; duplicate epoll registration; file-to-socket sendfile; all thirteen
socket opcodes; callback errors and partial I/O; close versus a blocked socket
callback; accept publication, malformed-child rollback, allocation-exhaustion
rollback, and draining-owner rollback. It runs
the same state machine under ASan, UBSan, and TSan and rejects host descriptor
or dynamic-loader imports. A real NDK r28c API-35 Android AArch64 ELF separately
locks the descriptor, epoll, and socket-control LIBC import surfaces and Android
constant/layout ABI. The C probe separately freezes every ABI-v1/v2 prefix and
ABI-v3 request/result size and selected field offsets on arm64.

## Integration blockers

- The current filesystem and socket facades each allocate their own guest
  integer tokens, while stdio owns separate stream state. Their allocation,
  lookup, close, and test-only reset paths must be replaced together; switching
  only one provider would leave ambiguous descriptors at the libc entry points.
- Process-wide libc `close`, `read`, `write`, `poll`/`ppoll`, `ioctl`, and
  `sendfile` must enter the broker first. Provider-specific close functions may
  use `close_owned` during migration, but the final public `close` route must be
  generic and owner-dispatched.
- Guest descriptors `0`, `1`, and `2` need an explicit bootstrap policy. This
  prototype intentionally emits only marked high tokens; it does not decide
  whether standard streams are fixed broker slots, inherited capabilities, or
  unavailable in an app sandbox.
- Exact-target `dup2`/`dup3` remains unresolved. A generation-safe encoded token
  cannot simultaneously equal an arbitrary caller-selected integer and reject
  stale copies after replacement. The broker deliberately exposes the
  non-Linux-named `duplicate_with_flags` allocation primitive instead. The
  Android fixture pins the real `dup3` import as a required translation seam;
  a libc shim must reject it until a separate guest-fd indirection policy exists
  and must never silently route it to `duplicate_with_flags`.
- `fcntl(F_DUPFD)` without CLOEXEC, `SCM_RIGHTS`, directory handles, and epoll
  nesting are still fail-closed. SCM_RIGHTS in particular must transfer a new
  OFD reference, never copy a token integer across broker instances.
- `sendmsg` and `recvmsg` remain outside ABI-v3. Their scatter/gather payloads
  and ancillary data require a separate bounded request ABI; especially,
  `SCM_RIGHTS` cannot be represented as ordinary bytes or guest token integers.
- Real blocking poll/epoll requires a central wakeup/fan-in mechanism across
  virtual files, sockets, close events, timers, and signals. The current epoll
  callback demonstrates generation-safe level readiness only; it does not claim
  timeout, signal-mask, EPOLLET, EPOLLONESHOT, or fairness parity.
- Socket creation and socketpair remain provider entry points because they begin
  without an existing guest descriptor. They must publish every created opaque
  socket object through this broker and close it on publication failure. All
  later descriptor-taking socket operations must use the ABI-v3 typed dispatch;
  `ioctl` is not an acceptable control tunnel.
- Provider callbacks must treat the broker object key as opaque and maintain
  their own object lifetime until close returns. They must never unwrap another
  owner's key or publish a host descriptor as a guest token.
