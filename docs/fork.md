# Guest fork

A guest process is a Darwin process running `linux-run`. Linux programs
fork without exec all the time; zygote does it for every app. This page
describes how the syscall layer (`crates/aim-linux-abi/src/sys/fork/`)
makes such a fork on Darwin.

## Why not Darwin `fork()`

A child of Darwin `fork()` that does not exec cannot reach XPC services:
launchd refuses its lookups ("Connection init failed at lookup with error 3
- No such process"). Metal compiles shaders in `MTLCompilerService`, an XPC
service, so an app forked from zygote could not link a GL program ("MSL
compilation error: Unable to reach MTLCompilerService"). A shader Metal had
cached from an earlier run hid this in testing. A shader unique to the run
fails in every forked child, even when the parent never used Metal. A
`posix_spawn`ed process compiles it. launchd answers the child's lookup
with ESRCH, apparently because it has no XPC domain for a process that did
not exec. libSystem's `atfork` child handler already runs libxpc's fork
hooks, and calling `_vproc_post_fork_ping` again in the child does not
help. We found no public or private API that attaches a fork child to
such a domain.

Darwin `fork()` also needed a set of fix-ups: the layer's locks held
across the fork, `__objc_fork_ok`, re-aliasing the stub islands, and
waiting out exiting threads. Those are gone with it.

## How a fork is made

The design is the one Cygwin uses on Windows. The parent spawns a fresh
`linux-run`, and the new process becomes a copy of the parent.

1. **Guest memory lives in a known range.** Every mapping the guest can see
   is placed in `[0x80_0000_0000, 0x800_0000_0000)` (`sys/arena.rs`). This
   covers guest mmaps, the loaded program, interpreter and stack, the
   program break, binder buffers, memfd memory and stub islands. The heap
   reference window at 1 TiB is inside the range. Host allocations never
   land there: Darwin places them first-fit from low addresses, and
   libmalloc's regions sit between 0x70_0000_0000 and 0x80_0000_0000.
   A non-fixed guest mapping gets the start of the range as its hint.
2. **Snapshot.** The parent walks the VM entries of the range and makes a
   Mach memory entry for each one:
   - a copy-on-write copy (`MAP_MEM_VM_COPY`) of private memory. A copy of
     an inaccessible entry is taken after briefly making it readable;
   - the memory itself (`MAP_MEM_VM_SHARE`) of shared memory: MAP_SHARED
     files and memory, memfds, ashmem and binder buffers. The execute
     permission Darwin refuses for a shared file entry is left out;
   - nothing for an entry that was never touched. The child allocates it
     afresh.

   Copies are taken before `fork` returns, so what the parent writes
   afterwards stays its own.
3. **State.** The layer's per-process state is written into one blob
   (`sys/fork/state.rs`). Each module saves and restores its own part:
   - the fd table: each fd's kind (eventfd, timerfd, socket, epoll,
     inotify, directory stream, memfd), with dup'ed fds sharing one object,
     and the hidden fds;
   - identity and the by-pid entry, the executable, the stack areas,
     personality, prctl state, the program break, file-copy records,
     memfds, SELinux attributes and pidfds;
   - signal dispositions, and the forking thread's mask, alternate stack,
     name and scheduling attributes;
   - the translation runtime's file table, stub islands and diagnostics
     modules;
   - the working directory and the process's own mounts;
   - the GPU module's display handles.
4. **Spawn.** `posix_spawn` starts `linux-run [runtime options]
   --fork-child KQUEUES`, the same executable with the same runtime
   options. The spawn uses `POSIX_SPAWN_CLOEXEC_DEFAULT` and inherits
   every open fd except kqueues (Darwin cannot pass those). KQUEUES lists
   the guest's kqueue fd numbers (epoll, inotify, pidfd), and the child
   holds them with `/dev/null` from its first moment so nothing else takes
   them. The child gets a send right to a port of the parent's through its
   registered ports.
5. **Handover.** A host thread of the parent serves the port. The child
   sends its reply port and receives the memory entries and the blob in one
   message. If the child dies first, a no-senders notification ends the
   wait. `fork` returns once the child is spawned. A parent that exits,
   execs or dies of a signal before the handover is done waits for it
   (`wait_handovers`), since the child cannot start without it.
6. **The child** maps every entry at its address. Only the heap window is
   mapped over; any other collision is an error. It then restores the
   state and rebuilds what the state names:
   - kqueues for epolls, inotifies and pidfds on their numbers;
   - the timerfd thread;
   - its by-pid entry;
   - a new binder connection;
   - writable views of the stub islands.

   The forking thread becomes the child's main thread (tid = pid). The child
   takes over its registers, thread pointer and the used part of its shadow
   call stack, applies CLONE_SETTLS, CLONE_CHILD_SETTID, CLONE_CHILD_CLEARTID
   and a new stack, and resumes the guest with 0 in x0.

The spawned process is the parent's Darwin child, so `waitpid`, `waitid`,
pidfds, SIGCHLD, the process group and the session behave as for any
child. umask and rlimits come with `posix_spawn`.

`vfork` takes the same path. The parent waits on a close-on-exec pipe
whose write end the child holds until it execs or exits. A thread with
its own file table (debuggerd's pseudothread) is a fork too.

## What a child does not get

- Other threads, pending signals and POSIX timers. Linux does not pass
  these on either.
- ANGLE's displays and the rest of the parent's host objects. The child
  loads ANGLE and makes each display again on first use, initialized if the
  parent had initialized it (`docs/gles-driver.md`). Audio, display and
  sensors clients connect on first use.
- Knob fds (writable `/proc` files) stay plain fds, since their action is
  code. Evdev fds are also plain until reopened.
- The snapshot is taken entry by entry while other guest threads run. On
  Linux, the whole address space is copied at one instant. Zygote forks
  single-threaded.
- Guest `MAP_FIXED` mappings outside the guest range and `MADV_DONTFORK`
  are not handled.

## Cost

All measurements are on an M2 Pro with a debug build, from
`tests/process.rs` (`process_latency`,
`fork_latency_with_a_large_address_space`):

| | Darwin `fork()` | spawned fork |
|---|---|---|
| fork+exit+wait, small process | 1.0 ms | 8.5 ms |
| fork+exec+exit+wait | 73 ms | 82 ms |
| fork+exit+wait, 4,000 mappings + 300 MiB touched | not measured | 25 ms |
| `fork` in that parent | not measured | 7.4 ms |

Most of a small fork is process start. Spawning `/usr/bin/true` costs about
3 ms on this machine. A debug `linux-run` costs 7 ms, most of it dyld
loading the frameworks linux-run links. In the parent, each mapping costs
about 1.7 µs to snapshot. In the child, each mapping costs about 1.2 µs to
map.
