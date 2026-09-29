# Binder driver (ADR 0012, phase P1)

The original arm64 `libbinder` talks to `/dev/binder`, `/dev/hwbinder` and
`/dev/vndbinder`. It uses ioctls and a read-only mmap'ed receive buffer.
`crates/aim-binder-driver` is that driver. Its specification is the
Android common kernel's `include/uapi/linux/android/binder.h` (protocol 8)
and `drivers/android/binder.c`. It is a library with no dependency on the
syscall layer.

`crates/aim-binder-host` runs one driver for all guest processes in a
daemon and connects each guest's syscall layer to it (#168). The syscall
layer exposes the device nodes (#167). With both, the original
`servicemanager` of the pinned image serves the original `service` and `cmd`
clients in other processes.

## Why a new crate

`aim-binder-device` and `aim-binder-process` were built for a
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
process mapping, its receive mapping and its Mach ports. One lock guards
the core. Thread wakes are delivered after it is released, so a woken reader
does not block on the lock it was woken under. That brought the in-process
p50 from 13.5 to 9.2 µs.

## Driver–process path

`aim-binder-host` has two halves. The `server` module is the daemon side,
a library that any host process can embed; `aim-binderd --service NAME`
runs it alone. The `client` module is what the syscall layer calls
(`crates/aim-linux-abi/src/sys/binder.rs`, enabled by
`linux-run --binder NAME`). The daemon is found by its bootstrap name.

- **Open.** `openat("/dev/binder")` creates a Unix socket pair. The guest's
  fd is one end, and the other end goes to the daemon as a fileport. The
  daemon answers with a **file port** that stands for the open file.
- **Threads.** The first ioctl of a guest thread asks the file port for a
  **thread port**. A daemon thread serves it.
- **Ioctls.** Each guest binder ioctl is one `mach_msg(SEND|RCV)` from the
  calling thread to its thread port, answered through a send-once right.
  The daemon thread runs `Driver::ioctl` and may block in it, as the guest
  thread blocks in the kernel. It sends that answer in the same `mach_msg`
  that waits for the next request.
- **Guest memory.** The daemon never reads guest memory. The shim sends the
  argument structure and gathers what the driver will read from the write
  stream: each `BC_TRANSACTION`/`BC_REPLY`'s data and offsets, and for
  `_SG` commands every `BINDER_TYPE_PTR` payload. The daemon's
  `GuestProcess` serves `copy_from_user` from those segments. It records
  `copy_to_user` as writes that the shim applies when the ioctl returns.
- **Readiness.** epoll and poll need no binder support, because the guest
  fd is a real socket. The daemon writes one byte to its end while a read by
  the polling thread would find work. Every ioctl answer says whether the
  shim must drain that byte, which keeps it level-triggered like
  `binder_poll`. The polling thread is registered from `epoll_ctl` and
  `ppoll`, the paths from which Linux calls `binder_poll`.

A call and its reply cross four Mach hops: client → daemon → server, then
server → daemon → client. The daemon adds two thread wakes: the client's
daemon thread wakes the server's, and the other way back.

Transport alone, measured in `experiments/p1/01-binder-transport` (removed;
see git history) on an M2 Pro:

| path | p50 | p99 |
| --- | --- | --- |
| relayed Mach, 4 hops | 6.5–6.7 µs | 19 µs |
| direct Mach, 2 hops | 6.2–6.6 µs | 14 µs |
| relayed Unix sockets | 10.3–10.5 µs | 16 µs |
| the core in-process (two threads, condvar) | 9.2 µs | 17 µs |

Other options:

- **Shared memory with futex-style wakes.** This would put the core's data
  structures in memory every guest can corrupt.
- **Unix sockets.** About 4 µs slower at p50.

End to end, between original guest processes (M2 Pro, release build,
20,000 calls after 2,000 warm-up, timed in the guest with the image's
`libbinder_ndk`; `crates/aim-linux-abi/tests/servicemanager.rs`):

| call | p50 | p90 | p99 |
| --- | --- | --- | --- |
| `AIBinder_ping` of servicemanager (epoll `Looper`, 3 server ioctls) | 27–30 µs | 32–44 µs | 41–64 µs |
| a service method, server blocked in `joinThreadPool` | 26–27 µs | 30–46 µs | 40–90 µs |

Linux takes about 10–30 µs per transaction. Each ioctl here is a Mach round
trip to another thread (about 4 µs per hand-off). The two daemon-internal
wakes per call are what parked reads remove: a read with no work would not
block its daemon thread but keep the guest's send-once right, and whoever
queues work for it would run its read pass and answer it directly.

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
  pre-reserved fd number. Each thread keeps 8 of them; a transaction with
  more files stays queued, the read returns `want_fds`, and the shim reads
  again with that many (`GuestProcess::can_install`). Linux has no such
  limit, and Chrome passes its child processes a dozen or more (#256).
- **Credentials.** They come from `Credentials`, which the process registry
  supplies at open:
  - the guest-visible pid;
  - the Android uid (`sender_euid`);
  - the SELinux label, for nodes with `FLAT_BINDER_FLAG_TXN_SECURITY_CTX`,
    such as servicemanager, which reads `getCallingSid`.

  The host uid is never used. Through the daemon, the pid comes from the
  Mach audit token of the open. The uid and label are what the guest's
  syscall layer reports (`getuid`, `linux-run --seclabel`) until the
  daemon's process registry supplies them.

## Lifetimes

- **Process release** (last close or death): the daemon's end of the
  readiness socket reads EOF once every guest fd for the file is closed,
  including at process exit and in forked children. The daemon then
  releases the process, and its file and thread ports die with it:
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
- **Signals.** `Driver::interrupt` ends a blocked read with `EINTR`. Guest
  signal delivery does not reach it through the daemon yet.

## A binder process on the host

`aim_binder_host::local::LocalProcess` is a binder process of the driver
inside the host process that runs it (guest-init): the native system
services of ADR 0013 (docs/system-services.md). It plays libbinder's
`ProcessState` and `IPCThreadState` over `Driver::ioctl` directly, with no
Mach hop: host threads are its binder threads (a looper, then more on
`BR_SPAWN_LOOPER`), addresses are host addresses and its receive buffer is
heap memory. It serves `Service` objects as nodes (answering
`INTERFACE_TRANSACTION` and `PING` itself), calls other nodes and serves
calls routed back to the waiting thread meanwhile, takes strong references
to handles it keeps (`BC_INCREFS`/`BC_ACQUIRE` before the buffer that
carried them is freed) and gets death notices. Parcels are libbinder's
kernel format (`aim_binder_host::parcel`): strings with length and NUL,
binders followed by their stability, the interface token and Java's
exception replies. Its nodes do not accept file descriptors (#433).

## Tracing

`Driver::start_trace` records every transaction (`TraceRecord`): sender pid
and euid, target pid, the interface token at the start of the data (AIDL's
descriptor or HIDL's interface name), code, one-way or not, and for a
synchronous call the time from the driver taking `BC_TRANSACTION` to it
taking the target's `BC_REPLY`. guest-init `--binder-trace FILE` appends
them to a file once a second; `tools/binder-trace-report.py` names the
services and methods (docs/system-services.md, "Inventory").

## Tests

`cargo test -p aim-binder-driver` runs an in-process harness
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

Across processes:

- **`cargo test -p aim-binder-host`** (`tests/transport.rs`): the daemon
  and two binder files in one host process. It covers a transaction with
  data, an fd and a security context, readiness and its drain, the reply,
  and release on the last close, seen as `BR_DEAD_BINDER`.
- **`cargo test -p aim-linux-abi --release --test servicemanager --
  --nocapture`** runs the pinned image and is skipped when it is absent. The
  test plays the minimum of init: property areas and the property service
  (`aim-guest-init`), `--path-map`, and the binder host in the test
  process. It checks that:
  - the original servicemanager becomes the context manager and sets
    `servicemanager.ready`;
  - `service list`, `service check` and `cmd -l` get their answers;
  - a third process (`tests/fixtures/binder_ping.c`, built with the pinned
    NDK) adds a service, a client calls it, and the service's death removes
    it from servicemanager;
  - the latencies above are printed.
