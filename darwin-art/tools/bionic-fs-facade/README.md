# Bionic filesystem facade gate

This gate connects 29 Class-B Android arm64 libc filesystem imports
to the Darwin component-walking filesystem broker. In addition to the original
file/path/cwd/DIR set, it owns `fchmod`, `fchmodat`, `ftruncate`, `isatty`,
`link`, `mkdir`, `pathconf`, `realpath`, `remove`, `rename`, `statvfs`,
`symlink`, `truncate`, `unlinkat`, and `utimensat`. The standalone runner and
the ART adapter exercise the same provider.

## Boundary and ownership

The facade is activated with an already-open Darwin directory, mounted as an
immutable guest root (the standalone gate uses `/system`), plus a private
in-memory writable mount at `/data`. `darwin-art-prefix`
normalizes guest paths and rejects mount escape; `darwin-art-fs-broker` walks
each component relative to that directory with no-follow semantics. Symlinks,
`..` escape, special nodes, and paths outside these mounts fail closed. The
`/data` overlay contains only facade-owned directories and byte-vector regular
files; it grants no new host authority. The immutable mount, link creation, and
unimplemented mutations still end in Android `EROFS` without calling a Darwin
mutation API.

The overlay is the Wine-like guest prefix slice needed by libc++
`filesystem::copy_file`. It supports persistent in-process nodes, `mkdir`,
`O_RDONLY`, and `O_WRONLY|O_CREAT|O_TRUNC` opens, plus `fchmod`, `ftruncate`,
`fstat`, `read`, and close. The separate sendfile provider calls
`darwin_art_bionic_fs_sendfile_transfer`, which copies at most 64 KiB per call
between virtual descriptors. Null offsets advance the input position; explicit
offsets leave it unchanged and advance only the caller's `off_t`. Neither path
calls Darwin `sendfile`, exposes a host fd, or uses a raw syscall.

Guest descriptors are facade-local virtual integers beginning at 10000. A
Darwin descriptor is retained in the private Rust table and is never returned
or resolver-exposed. This range is not globally collision-free: there is no
coordination with another descriptor provider or with an application that
manufactures integers. Runtime integration must use one central descriptor
namespace (or non-overlapping tagged ranges) and reject cross-provider use.

Two exact absolute byte paths are synthetic exceptions to the `/system` mount:
`/dev/random` and `/dev/urandom`. They never enter the prefix resolver or host
path broker. `Facade` construction owns a Security.framework
`SecRandomCopyBytes` backend, and each exact `open(token, O_RDONLY)` creates a
typed random-device entry containing no host descriptor. Aliases, trailing
slashes, relative spellings, and every nonzero flag remain outside this policy.
Closed virtual numbers are reused only after their old typed entry is removed.

The descriptor mutex is intentionally simple. `read` holds the global table
lock for the entire host read, so a concurrent `close` or another descriptor
operation is serialized until that read completes. This is the current
contract, not Linux close/read concurrency emulation.

Directory streams use a second facade-owned side table. The guest `DIR*` is a
stable opaque token allocation containing only a numeric identity; Darwin
`DIR*`, its fd, and stream state remain exclusively in the table. The returned
`struct dirent*` points to a separate translated Android-only buffer. `closedir`
removes the side-table record and frees both guest allocations because POSIX
defines every later use of that `DIR*` or entry pointer as undefined. Thus
memory is proportional to concurrently live streams rather than lifetime open
count. A 512-cycle stress test requires the active table to return to zero.
As with virtual fds, another provider must not invent or accept these tokens.

## Supported calls

- `open(path, flags, ...)`: read-only access under the mount root.
- `openat(AT_FDCWD, path, flags, ...)`: equivalent to `open`; an absolute path
  also ignores `dirfd`, matching the relevant Linux rule.
- `read`, `close`, and `fstat` for facade-owned virtual descriptors. Exact-path
  `stat`/`lstat` and descriptor `fstat` return the same synthetic metadata.
  Random reads fill the complete requested buffer from the facade-owned secure
  backend; metadata uses Android character-device modes and Linux
  `makedev(1,8)`/`makedev(1,9)` identities. `close`, `isatty`, mutation
  rejection, and `fdopendir` share the same typed descriptor table.
- `stat` for admitted regular files/directories; the no-follow authorization
  policy means it deliberately does not implement Linux symlink following.
- `lstat` for admitted non-symlink nodes. `/proc/self/exe` and
  `/proc/thread-self/exe` have Android's virtual `app_process64` readlink
  identity without exposing the Darwin host executable. A final symlink rejected with host
  `ELOOP` becomes `EOPNOTSUPP`, because the broker cannot safely return symlink
  metadata. `readlink` returns `EINVAL` for securely opened regular files and
  directories, matching Linux/Bionic, and the same final-symlink capability
  rejection. Intermediate-component failures retain their broker-translated
  errno (`ENOTDIR` for the pinned Darwin no-follow walk), rather than being
  mislabeled as final-link support. Link bytes are never fabricated.
- `getcwd` copies the guest byte path into caller-owned storage. Null-buffer
  allocation is `EOPNOTSUPP` because this module does not own the Bionic
  allocator. `chdir` verifies a directory through the broker and updates one
  process-wide facade cwd shared by all activated pthreads.
- `opendir`, `fdopendir`, `readdir`, and `closedir` translate a private Darwin
  stream into the pinned Android arm64 280-byte `dirent` layout. Directory
  names remain uninterpreted bytes. Known Darwin `d_type` values are translated
  explicitly; unknown values become Android `DT_UNKNOWN`.
  `fdopendir` atomically consumes a facade-owned virtual directory descriptor
  into the stream token. A failed conversion leaves that same virtual descriptor
  open. After success, `dirfd` returns the same guest descriptor, which remains
  usable for metadata operations until `closedir` invalidates it. The native
  stream uses a duplicate sharing the same open file description; neither host
  descriptor nor host DIR pointer escapes to the guest.
- Android arm64 `struct stat` is materialized as the pinned 128-byte Bionic
  layout; it never exposes a Darwin `struct stat`.
- `isatty` recognizes facade-owned regular-file/directory descriptors and
  returns zero with Android `ENOTTY`; unknown descriptors return `EBADF`.
- `pathconf` explicitly maps all 20 Android selector numbers to Darwin semantic
  constants and calls `fpathconf` on the broker-opened descriptor. Android
  numbers are never forwarded as Darwin numbers. The audit differentially
  checks `_PC_2_SYMLINKS`, whose Android and Darwin values differ.
- `realpath` securely opens the no-follow target, then copies the normalized
  guest byte path to the caller's Android `PATH_MAX` buffer. Null allocation
  mode is `EOPNOTSUPP` because allocator ownership remains external.
- `statvfs` securely opens the object, copies common scalar fields into the
  pinned 112-byte Android arm64 layout, translates the portable Darwin flags,
  forces Android `ST_RDONLY`, and zeroes every reserved word.
- `fchmod`, `ftruncate`, and null-path `utimensat` distinguish facade-owned
  descriptors (`EROFS`) from unknown descriptors (`EBADF`). Path mutations
  first pass byte-path containment. The `*at` calls accept only reviewed Android
  flags and `AT_FDCWD`/absolute paths; no Linux flag is forwarded to Darwin.

The cwd is a verified lexical guest path. Every later relative lookup performs
a fresh broker walk from the immutable mount root, so containment survives host
renames, but exact POSIX directory-identity behavior across an external host
rename is not claimed.

`readdir` and `closedir` hold one global directory-table lock across each host
operation. They are serialized across all facade streams; a close cannot race
the corresponding host call. End-of-directory returns null without changing
Bionic errno. The translated entry buffer remains allocated but, as on Bionic,
its contents may be replaced by the next `readdir` on that stream. `closedir`
frees the entry and token after the serialized host close; concurrent or later
use is the POSIX-undefined caller case and is not made into a tombstone API.

Relative `openat` and mutation `*at` calls against a valid facade file
descriptor are explicitly unsupported and return Android `EOPNOTSUPP`; an
unknown descriptor returns `EBADF`. `lseek`, writable/create/truncate/append
open modes, socket, and every symbol absent from the closed resolver are
unsupported capabilities.
`O_DIRECT`, `O_PATH`, and unknown flags return `EOPNOTSUPP`; write intent outside
`/data` returns `EROFS`. The variadic shims do not consume a mode argument;
overlay nodes use a private default mode until the source-driven `fchmod` step
updates it.

## Errno and activation contracts

Expected Darwin failures are translated through the standalone Bionic errno
TLS provider. Darwin errno is saved/restored by every C shim and its address is
never guest-visible. An unknown Darwin errno or internal capability failure
sets the sticky `capability_failure` marker and deterministically publishes
Android `EIO` instead of leaving stale Bionic errno. The embedding boundary
must stop guest execution when that marker is observed; `EIO` is a fail-closed
diagnostic, not a claimed semantic translation.

Production activation is process-wide. The embedding boundary installs one
`Arc<Facade>` from a caller-authorized, already-open directory fd; the facade
duplicates that fd before returning and never consumes caller ownership. Every
guest entrypoint and the ioctl descriptor callback acquires a short lease from
that same owner. Uninstall first stops admission, waits for all leases to drain,
then drops the broker, open descriptors, directory streams, and entropy owner.
Construction failure publishes nothing, a duplicate live install is rejected,
and calls made during drain fail closed instead of extending teardown.

The ART adapter uses the sibling graph's already-authorized parent dirfd and
mounts it at guest `/`. Concurrent graphs may share the single owner only when
their preopened directory device/inode identity matches. Each graph holds one
runtime lease; only the final graph release, after graph finalizers/unmapping,
uninstalls the facade. A graph from a different directory authority is rejected
rather than silently gaining access to the first graph's mount.

The Rust `Activation` API remains a standalone-test-only pthread override. TLS
owns an `Arc<Facade>`, so forgetting the guard leaks a reference rather than
leaving a dangling pointer. `Activation` is deliberately `!Send`/`!Sync`; a
compile-fail doctest locks the rule that a guard cannot move to a different
pthread. Nested activation restores the previous `Arc` on the same pthread.
When present it takes precedence over the process owner so isolated tests cannot
accidentally consume production state.

`darwin_art_bionic_fs_ioctl_fd_lookup` is the exact callback accepted by the
standalone ioctl provider. It classifies random, other, and closed/unknown
tokens under the descriptor lock without exposing a host fd. Because it uses
the same process-owner lease as every filesystem entrypoint, ART-created
pthreads require no per-thread activation. The ioctl provider's own
resolver/activation is a separate composition boundary; guessing from the
numeric fd is forbidden.

Only `open`, `stat`, and `lstat` claim the two exact synthetic paths. Their
`pathconf`, `realpath`, mutation, and directory-stream forms remain outside the
synthetic policy and fail through the ordinary mount-containment boundary.

## Deterministic audit

Run `./audit.sh`. It pins and hashes the NDK r28c API 35 `fcntl.h`, Linux
`fcntl.h`, `stat.h`, `statvfs.h`, `dirent.h`, `unistd.h`, `stdlib.h`, and
`stdio.h` inputs, the prefix and broker sources, the Bionic errno translator,
the OS-constant manifest, and the real libc++ Class-B import classification.
It also pins LLVM's exact random-device constructor source proving
`open(token, O_RDONLY)` and the synthetic-device policy manifest.
Compile-time probes lock every signature, the Android pathconf/`AT_*` numbers,
and the Android `stat`, `dirent`, and `statvfs` layouts. It then:

1. cross-compiles a real Android AArch64 ELF fixture whose only undefined
   symbols are `__errno` and the 29 listed facade imports;
2. loads it with a closed resolver and exercises content, Android stat/dirent
   layouts, cwd transitions, EOF errno, `ENOENT`, unsupported flags, write
   rejection, regular/final/intermediate `readlink` distinctions,
   traversal/symlink policy, virtual-dirfd rejection, pathconf/statvfs
   translation, realpath byte normalization, deterministic mutation rejection,
   virtual descriptors at or above 10000, secure nonrepeating random reads,
   random fstat/close/fdopendir/isatty semantics, and descriptor reuse;
3. differentially compares Android `_PC_2_SYMLINKS`, block size, and translated
   statvfs flags against the same securely opened Darwin object;
4. proves Darwin host errno is preserved and no unknown translation occurred;
5. stresses close against a blocked random read and ioctl lookup against close,
   requiring each lookup to observe either the live typed entry or `BAD`; it
   also verifies failed-install rollback, duplicate rejection, TLS-free pthread
   access, drain-time admission rejection, and an uninstall blocked until the
   in-flight read releases its lease;
6. repeats the complete Android ELF boundary with C ASan and UBSan, then runs
   broker/prefix tests, the activation `!Send` compile-fail test, Clippy, and
   formatting checks using temporary target directories.

The gate performs no host filesystem writes through the facade. It additionally
copies the immutable payload into `/data` through the virtual sendfile callback
and has the Android ELF reopen, read, truncate, and stat the persistent node.
All host test setup remains temporary audit-harness data.
