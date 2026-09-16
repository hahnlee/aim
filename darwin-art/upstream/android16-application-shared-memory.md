# Application shared memory owner

The adjacent lock pins the same frameworks/base revision used by the existing
Android runtime JNI owners. `tools/build-android16-application-shared-memory.sh`
verifies the original ApplicationSharedMemory and PropertyInvalidatedCache
implementations, then applies a hash-locked Darwin syscall-boundary patch,
without copying their data layout
into a Darwin structure. Source files stay in the ignored `_aosp` cache.

The archive is now linked into the graphics runtime and the two original
registrars run during framework JNI initialization. The headless link inputs
are also updated, but a fresh headless audit is outstanding. Registration is
not evidence that application binding works. The included process test
exercises original network-time and nonce operations over distinct MAP_SHARED
addresses. It does not validate system feature JNI arrays, Binder publication,
JNI registration, or Binder publication. It now uses SystemRegion's separate
read-only descriptor; a companion exec+SCM_RIGHTS test checks denied writable
mapping, denied mprotect elevation, denied pwrite/ftruncate and visibility of
later system-writer updates after the creator's descriptors have closed.

`compat/memory/system_region.*` supplies that Darwin boundary. It opens separate
O_RDWR/O_RDONLY descriptions of the same verified inode and unlinks the name
before publication. It owns two close-on-exec descriptors with RAII; exported
duplicates and mappings retain storage by kernel reference counting. This is
a read-only export capability, **not** general ashmem sealing of arbitrary old
read/write descriptors and not a same-user macOS process security sandbox.

Required integration boundaries:

- System process creates and initializes the region once. App processes receive
  the same region through their application binding, not a newly allocated copy.
- Keep upstream cache layout and atomic operations. Missing network time remains
  INVALID_NETWORK_TIME, rather than manufacturing a timestamp.
- Provide ashmem creation and read-only export at the Darwin memory boundary.
  Current `ASharedMemory_setProt` records metadata only. Forwarding that function
  as if it kernel-sealed an ordinary read/write FD would be incorrect.
  SystemRegion is now connected at create/map/unmap/duplicate through
  `compat/memory/application_memory.*`. Pending creation transfers ownership to
  mapped regions; unmapping releases the capability owner. This archive-level
  JNI connection is registered in the graphics runtime; system service
  publication remains outstanding.
- Preserve the system's existing writable mapping while preventing new writable
  app mappings. Verify the exported descriptor through a separate process.
- Invoke the upstream registrars through runtime initialization. Keep the
  CriticalNative/FastNative signatures intact.
- Replace the process-local AMS attachment proxy with system-owned binding
  transport; do not add this state to the package-registry test fixture.
