# Bionic socket facade

This standalone Darwin provider is a closed Android arm64 socket vertical
slice. It owns the exact `libc.so`/`LIBC` symbols in `manifests/imports.tsv`:
socket creation, loopback addressing, listen/connect/accept, socket names and
options, byte and scatter-gather send/receive, `poll`/bounded `ppoll`, shutdown,
and close. A real Android AArch64 ELF drives IPv4 TCP, IPv4 and IPv6 UDP, plus
an `AF_UNIX` socketpair through the repository ELF loader. There is no dyld or
`dlsym` resolution path.

## Descriptor boundary

Guest code only sees positive, generation-tagged virtual tokens. The 512-slot
standalone table never returns a Darwin descriptor. Every admitted operation
duplicates the host descriptor under the table lock with
`F_DUPFD_CLOEXEC`, then releases the lock before entering the kernel. Closing
or resetting a token removes and closes the owned descriptor; an operation
that already borrowed a duplicate finishes safely, while later and stale-token
calls fail with Bionic `EBADF`. A wrapped generation prevents an old token from
aliasing ordinary slot reuse; as a bounded standalone prototype the generation
space is 21 bits and is not a durable cross-process identity.

This table is intentionally provider-local. Runtime integration must replace
the symbol-level `close` overlap with the central FD owner and route socket
tokens through an equivalent borrow/close seam. It must not guess from integer
ranges or unwrap descriptors in unrelated providers.

## ABI translation

The provider translates instead of forwarding Android numbers:

- `AF_UNSPEC`, `AF_UNIX`, `AF_INET`, and Android `AF_INET6=10` map to Darwin
  families. Address-bearing operations currently accept only IPv4 and IPv6;
  Unix path/abstract addresses fail `EAFNOSUPPORT`.
- Android `SOCK_STREAM`, `SOCK_DGRAM`, `SOCK_NONBLOCK=00004000`, and
  `SOCK_CLOEXEC=02000000` become a base Darwin type plus explicit `fcntl`
  state. Every hidden host descriptor is close-on-exec regardless of the guest
  flag.
- Android `sockaddr_in` (16 bytes) and `sockaddr_in6` (28 bytes) are copied into
  Darwin structures with `sin_len`/`sin6_len` and translated family bytes.
  Ports, addresses, flow info, and scope IDs retain their network/API byte
  representation. Result capacity follows the socket API's truncating-copy
  contract and reports the full Android size.
- The supported message flags are `MSG_OOB`, `MSG_PEEK`, `MSG_DONTROUTE`,
  `MSG_DONTWAIT`, `MSG_EOR`, `MSG_WAITALL`, and `MSG_NOSIGNAL`. The last is
  implemented by setting Darwin `SO_NOSIGPIPE`; its Linux bit is never passed
  to Darwin. Unknown bits fail `EOPNOTSUPP`.
- `SOL_SOCKET` options `SO_REUSEADDR`, `SO_TYPE`, `SO_ERROR`, `SO_SNDBUF`,
  `SO_RCVBUF`, and `SO_KEEPALIVE`, plus `IPPROTO_TCP/TCP_NODELAY`, have explicit
  option-number mappings. Unknown options fail `ENOPROTOOPT`.
- Host failures pass through an explicit Darwin-to-Bionic errno table. Host
  `errno` is restored on every return; success leaves the guest Bionic errno
  unchanged.

`poll` copies each 8-byte Android `pollfd`, translates requested and returned
`POLL*` bits, and borrows each valid socket token before entering Darwin
`poll`. Negative descriptors are ignored and invalid/stale tokens produce
`POLLNVAL` without changing Bionic errno. The standalone call is capped at 512
entries. `ppoll` accepts the Android arm64 16-byte `timespec`, validates it,
rounds positive sub-millisecond deadlines upward, and saturates at `INT_MAX`
milliseconds. A null signal mask preserves ordinary timeout and `EINTR`
behavior. A non-null Android signal mask fails `EOPNOTSUPP`: emulating it with
separate `pthread_sigmask` and `poll` calls would introduce a signal race and
would not be an honest atomic `ppoll` implementation.

Android arm64 `msghdr` is 56 bytes because `msg_iovlen` and `msg_controllen`
are 64-bit; Darwin's is 48 bytes with 32-bit fields. `sendmsg`/`recvmsg` copy
each field into a new Darwin structure and rebuild a bounded array of at most
1024 ABI-compatible iovecs, rejecting length sums above `SSIZE_MAX`. IPv4 and
IPv6 names use the same explicit sockaddr conversion as `sendto`/`recvfrom`.
Returned name length and `MSG_OOB`, `MSG_EOR`, `MSG_TRUNC`, and `MSG_CTRUNC`
bits are translated back to Android values. Any non-empty control buffer is
rejected before I/O with `EOPNOTSUPP`; this fail-closed rule includes
`SCM_RIGHTS`, whose guest descriptors could not safely be exposed or imported
by a provider-local socket table.

The provider does not forward ancillary data or unknown families, types,
protocols, flags, or options. `manifests/unsupported.tsv` records the closed
boundary: standalone `poll` cannot recognize other FD providers, `select`
still needs composition across all virtual-FD owners, and DNS needs Android
resolver/hosts policy. They remain explicit failures or unresolved symbols
rather than permissive stubs.

## Provenance and gate

`audit.sh` SHA-pins NDK r28c/API 35 arm64 socket headers and an exact AOSP
Bionic revision containing the LP64 syscall declarations and the Bionic
`send`/`recv`, poll, and descriptor-tracking `recvmsg` wrappers. It statically
asserts Android constants, offsets, and structure sizes, proves the Android
fixture has only the 20 versioned imports in the
manifest, rejects dynamic lookup, and runs the loaded ELF under ASan+UBSan and
TSan. Tests cover IPv4/IPv6 UDP scatter-gather, `SCM_RIGHTS` rejection, timed
polls, signal interruption mapped to Bionic `EINTR`, and an in-flight borrowed
poll that survives concurrent token close. The original concurrent `recv`
test also verifies `EBADF` and stale-token rejection. All cargo/build output
lives under a temporary target directory; the source tree remains target-clean.
