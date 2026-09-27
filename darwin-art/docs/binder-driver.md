# Binder driver (ADR 0012, phase P1)

The original arm64 `libbinder` talks to `/dev/binder`, `/dev/hwbinder` and
`/dev/vndbinder`. It uses ioctls and a read-only mmap'ed receive buffer.
`crates/darwin-binder-driver` is that driver. Its specification is the
Android common kernel's `include/uapi/linux/android/binder.h` (protocol 8)
and `drivers/android/binder.c`. It is a library with no dependency on the
syscall layer.

Wiring it into the syscall layer is #167; hosting it in darwin-artd is #168.

## Why a new crate

`darwin-art-binder-device` and `darwin-art-binder-process` were built for a
Darwin-rebuilt libbinder. They cannot serve the original ELF libbinder:

- **Reference counts.** Remote reference changes are dropped, so `BR_RELEASE`
  and `BR_DECREFS` never reach a node's owner across processes.
- **Buffers.** Scatter-gather payloads and fd arrays are never written to the
  receiver. `data.ptr.buffer` points into the device's own mapping, not the
  client's mmap.
- **Thread selection.** Nested calls are not routed to the waiting thread.
  There is no per-node one-way queue.
- **Missing protocol.** There is no `BR_NOOP`, no security context, no
  extended error, no node debug or node-info ioctls, and no poll.
- **Coupling.** They depend on engine-sys, the profile crate and the bionic fd
  broker, and there are two transaction engines (local and routed).

What was sound there (the command codec, object parsing, the best-fit
allocator) is small. The new crate re-derives it from the UAPI instead of
importing that coupling. The old crates retire with the old stack (#153).

## The core

- **Entry points.** `Driver::open`, `mmap`, `ioctl` / `ioctl_user`, `poll`,
  `set_notifier`, `interrupt` and `release` mirror the file operations. The
  argument of `ioctl` is the `_IOC_SIZE(cmd)` bytes of the ioctl structure.
- **The calling process.** It is reached through `GuestProcess`: user copies,
  plus `get_file`, `install_file` and `close_fd`.
- **Receive buffers.** They are written through `ReceiveMemory`.
- **Implemented protocol:**
  - `BINDER_WRITE_READ` with every BC command and BR return that Linux
    implements for protocol 8, except freezing (below);
  - `BINDER_SET_CONTEXT_MGR(_EXT)`, `BINDER_VERSION` and
    `BINDER_SET_MAX_THREADS`;
  - `BINDER_THREAD_EXIT` and `BINDER_ENABLE_ONEWAY_SPAM_DETECTION`;
  - `BINDER_GET_NODE_DEBUG_INFO`, `BINDER_GET_NODE_INFO_FOR_REF` and
    `BINDER_GET_EXTENDED_ERROR`.
- **Semantics that follow `binder.c`:**
  - deferred `BR_TRANSACTION_COMPLETE` for synchronous calls;
  - `BR_SPAWN_LOOPER` replacing the leading `BR_NOOP`;
  - one outstanding one-way transaction per node until `BC_FREE_BUFFER`;
  - nested calls routed to the thread of the target process waiting in the
    caller's chain;
  - `sender_pid` 0 for one-way calls and replies;
  - the write stream stops while a return error is pending.
- **Freezing.** There is no cgroup freezer on the device:
  - `BINDER_FREEZE` and `BINDER_GET_FROZEN_INFO` fail with `EINVAL`;
  - freeze-notification commands fail with `EINVAL`;
  - binderfs must not advertise `freeze_notification`, so libbinder never
    sends them.
- **Not modelled:**
  - scheduler priority inheritance (the node priority bits and `INHERIT_RT`);
  - `TF_UPDATE_TXN`, which only matters for frozen targets;
  - SELinux transfer hooks (SELinux is permissive);
  - binder debugfs.

## Where state lives

As in Linux, global state and per-process state are both kernel state:

| scope | state |
| --- | --- |
| global | contexts (one per device) and their context manager and its uid; nodes; transactions; death notifications |
| process | threads; nodes by user pointer; handles (lowest free descriptor, 0 for the context manager); the process work queue; waiting threads; delivered deaths; the receive allocator; `max_threads` and the spawn counters |
| thread | looper state; the work queue with deferred items; the transaction stack; return/reply error slots; the extended error |

All of it has one owner. Other processes' transactions write a target's
per-process state:

- they queue work on the target's threads;
- they allocate in its buffer;
- they walk its transaction stacks for nested calls.

Splitting that state across host processes would need cross-process
locking. The owner is the daemon (#168). A guest keeps only its fd → driver
process mapping, its receive mapping and its reply ports. One lock guards
the core. Thread wakes are delivered after it is released, so a woken reader
does not block on the lock it was woken under. That brought the in-process
p50 from 13.5 to 9.2 µs.

## Driver–process path

Each guest binder ioctl is one `mach_msg(SEND|RCV)` from the calling thread
to darwin-artd, on the thread's own reply port. A read with no work parks the
thread's send-once reply right. Whoever queues work for it completes the
read and replies. A call and its reply therefore cross four Mach hops:
client → daemon → server, then server → daemon → client.

Measured in `experiments/p1/01-binder-transport` (M2 Pro):

| path | p50 | p99 |
| --- | --- | --- |
| relayed Mach, 4 hops | 6.5–6.7 µs | 19 µs |
| direct Mach, 2 hops | 6.2–6.6 µs | 14 µs |
| relayed Unix sockets | 10.3–10.5 µs | 16 µs |
| the core in-process (two threads, condvar) | 9.2 µs | 17 µs |

Linux takes about 10–30 µs per transaction. A daemon-hosted driver should
land near 10 µs. Other options:

- **Shared memory with futex-style wakes.** This would put the core's data
  structures in memory every guest can corrupt.
- **Unix sockets.** About 4 µs slower at p50.

The core still blocks a reader on a condvar. The parked-read mode that the
Mach path needs is the first item of #168.

## Receive buffers

A guest's binder mmap is a Mach memory entry. The guest maps it read-only,
the daemon maps it read-write, and the daemon implements `ReceiveMemory` over
it. Transactions are built and written into the target's mapping once, as
Linux's `binder_alloc_copy_user_to_buffer` does. The layout is:

1. data;
2. offsets at `ALIGN(data_size, 8)`;
3. scatter-gather payloads after the offsets;
4. the security context at the end.

The sender's shim gathers data, offsets and `BINDER_TYPE_PTR` payloads into
its message. Payloads are inline when small and out-of-line
(copy-on-write) when large.

We did not map every target's buffer into its senders. Objects must be
translated by the owner of the handle tables anyway. For the small parcels
that dominate, the copies cost nanoseconds and the wakes cost microseconds.

## Object translation

- **Binders and handles.** `BINDER`/`WEAK_BINDER` become handles in the
  target. A new node's `BR_INCREFS` and `BR_ACQUIRE` are queued, deferred, on
  the sending thread. `HANDLE`/`WEAK_HANDLE` become the node's own
  `ptr`/`cookie` when they reach the owner, and a handle in the target
  otherwise.
- **Buffers.** `PTR` payloads are copied and their parent pointers are
  fixed up. Fixup order is validated as `binder_validate_fixup` does.
- **Fds.** `FD` and `FDA` require `ACCEPTS_FDS` on the node, or
  `TF_ACCEPT_FDS` on the call for a reply. Files are installed in the
  reader's context, as `binder_apply_fd_fixups` does. Fds from arrays are
  closed when the buffer is freed.
- **Fd transport.** The core's `File` is opaque. In-process it is a host fd.
  Through the daemon it is a fileport (`fileport_makeport` /
  `fileport_makefd`) carried in the same Mach message: SCM_RIGHTS through the
  daemon without a second channel. The receiving shim installs it at a
  pre-reserved fd number.
- **Credentials.** They come from `Credentials`, which the process registry
  supplies at open:
  - the guest-visible pid;
  - the Android uid (`sender_euid`);
  - the SELinux label, for nodes with `FLAT_BINDER_FLAG_TXN_SECURITY_CTX`,
    such as servicemanager, which reads `getCallingSid`.

  The host uid is never used.

## Lifetimes

- **Process release** (last close or death: a no-senders notification on the
  process port in the daemon):
  - the process's nodes die;
  - each holder with a death registration gets `BR_DEAD_BINDER`;
  - callers waiting on the process's threads get `BR_DEAD_REPLY`;
  - queued transactions fail back to their senders;
  - its references are dropped, which sends `BR_RELEASE`/`BR_DECREFS` to the
    owners.
- **Death notifications** follow `binder.c`'s states: clear before death,
  clear after delivery (`DEAD_BINDER_AND_CLEAR`, completed by
  `BC_DEAD_BINDER_DONE`), and registration on an already dead node.
- **Thread exit.** `BINDER_THREAD_EXIT` sends `BR_DEAD_REPLY` to the caller
  of any transaction the thread was serving.
- **Signals.** `Driver::interrupt` ends a blocked read with `EINTR`.

## Tests

`cargo test -p darwin-binder-driver` runs an in-process harness
(`tests/support`). Each guest process has its own mapping and fd table, and
threads are OS threads. Commands are built and returns parsed byte by byte,
as `IPCThreadState` does.

- **`tests/servicemanager.rs`** replays the sequence read from libbinder and
  servicemanager sources and compares the BR transcript:
  - servicemanager start (`BINDER_SET_CONTEXT_MGR_EXT` with security
    contexts, `setupPolling`, epoll-driven `handlePolledCommands`);
  - `defaultServiceManager()` (PING, `BC_INCREFS 0`, `BC_ACQUIRE 0`);
  - `addService`, `getService`, a direct call, and the service's death.
- **`tests/protocol.rs`** covers:
  - reference counting end to end, and the handle-0 and context-manager
    rules;
  - write-stream errors and the ioctl surface, including freezing;
  - one-way ordering and spam detection;
  - fds and fd arrays, and scatter-gather fixups;
  - nested-call routing;
  - death states, and peer death in both directions;
  - security contexts, poll, non-blocking reads and `EINTR`.
- **`tests/latency.rs`** measures a small call and reply:
  `cargo test --release --test latency -- --nocapture`.
