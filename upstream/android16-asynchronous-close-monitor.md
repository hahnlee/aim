# Android 16 asynchronous-close Darwin boundary

Android 16 splits this behavior across two native modules. `libandroidio` is a
module-complete one-TU library containing `AsynchronousCloseMonitor.cpp` and
depending only on `liblog`. `libjavacore` contains the separate
`libcore_io_AsynchronousCloseMonitor.cpp` registrar, which owns exactly
`signalBlockedThreads(FileDescriptor): void` and depends on libandroidio plus
libnativehelper. `Register.cpp` registers this class before `libcore.io.Linux`.

`IoBridge.closeAndSignalBlockedThreads` first atomically releases the managed
file descriptor, calls this registrar, and closes the released descriptor
last. Blocking native calls must therefore construct an
`AsynchronousCloseMonitor` around the syscall and translate an interrupted,
signaled operation to Android's asynchronous-close exception contract.
Registering only the Java native as a no-op would make close race indefinitely
with reads, writes, poll, accept, connect, send, and receive.

Darwin does not provide `SIGRTMIN`/`SIGRTMAX`, so the upstream non-Bionic
`SIGRTMAX-2` choice cannot compile. The Darwin module reserves `SIGUSR2`, while
ART continues to own SIGQUIT and SIGUSR1. It installs a handler without
`SA_RESTART`, maintains the same intrusive fd/thread monitor list, targets every
matching thread with `pthread_kill`, and records the signal state atomically.
The runtime embedding contract must keep SIGUSR2 reserved and unblocked on
threads that perform Android blocking I/O.

Run:

```sh
tools/build-android16-asynchronous-close-monitor.sh
```

The gate sparsely materializes only the checksum-locked upstream module,
registrar, registration-order, managed-class, and `IoBridge` contract files.
It builds separate one-member Darwin arm64 backend and registrar archives,
compiles the exact upstream registrar against the Darwin ABI-compatible header,
and verifies two threads blocked on the same pipe fd both wake with `EINTR` and
`wasSignaled=true`. A managed OpenJDK smoke additionally loads the exact
registrar and invokes its `FileDescriptor` JNI signature.

Integration is intentionally separate. The registrar should be called at the
upstream `Register.cpp` position. Every supported potentially blocking method
in the Darwin `libcore.io.Linux` backend must then use the monitor RAII boundary;
adding only the registrar does not complete asynchronous-close semantics.
