# Android 35 Bionic pthread/TLS provider — first coherent slice

This module is derived from the SHA-locked NDK r28c/API 35 arm64
`libc++_shared.so`. That ELF imports 24 `pthread_*` functions from
`libc.so@LIBC`; this coherent slice owns all 24 of them:

```text
pthread_self
pthread_key_create / pthread_key_delete
pthread_getspecific / pthread_setspecific
pthread_once
pthread_mutex_init / lock / trylock / unlock / destroy
pthread_mutexattr_init / destroy / settype
pthread_cond_wait / timedwait / signal / broadcast / destroy
pthread_rwlock_rdlock / wrlock / unlock
pthread_join / pthread_detach
```

Because those last two functions require a coherent owner for `pthread_t`, the
provider additionally owns `pthread_create`. It is not counted among the 24
source-derived libc++ imports; it is the explicit lifecycle seam that issues
the only tokens accepted by join/detach. The Android ELF fixture imports and
executes all 24 libc++ functions plus this one extra owner symbol.

## Object and state boundary

Android arm64 exposes `pthread_t` as an eight-byte `long`, key and once controls
as four-byte integers, mutex storage as 40 opaque bytes, and condition storage
as 48 opaque bytes, and rwlock storage as 56 opaque bytes, all aligned to four
bytes. Darwin uses
different representations. None of these Android values is cast to a Darwin
`pthread_t`, `pthread_key_t`, `pthread_once_t`, `pthread_mutex_t`, or
`pthread_cond_t`, or `pthread_rwlock_t`.

- `pthread_self` returns a process-unique, thread-local 64-bit Android token. It
  is stable on one thread and never exposes Darwin's `pthread_t`.
- `pthread_create` creates a separate host thread but publishes only a provider-
  issued 64-bit Android token. The child waits on a startup handshake until the
  token is present in the provider table and written to the caller, preserving
  the pinned Bionic publication order. Join/detach lookup that table; an
  arbitrary or expired token returns Android `ESRCH` and is never cast to
  Darwin `pthread_t`. Each entry follows Bionic's four states: not joined,
  exited-not-joined, joined, and detached. Join claims the entry before waiting;
  detach either detaches a running host thread or joins an already-exited one
  to perform the cleanup that Bionic would do. Concurrent join-versus-detach is
  serialized so exactly one can claim the token. A detached entry is not
  removed until both host exit and successful host detach are observed, making
  provider reset a real target-quiescence check. This slice accepts only null
  create attributes; non-null storage returns `ENOTSUP` rather than being
  interpreted as a Darwin attribute object.
  Entry allocation, owner allocation, and token-map insertion all complete
  before Darwin `pthread_create` is called. Allocation failures return Android
  `ENOMEM`/`EAGAIN`; host-create failure removes the unpublished map entry and
  frees its owner. Lookup rejects unpublished entries, so no failure path can
  expose an uninitialized host handle or strand a child on the startup gate.
- TLS keys use an Android high-bit/index key and one provider-global Darwin TLS
  key. Each participating host thread owns one fixed 128-slot value table;
  Android keys and values are never reinterpreted as Darwin pthread objects.
  The single host destructor performs Bionic's four passes, clearing each value
  before its callback so callback reinsertion is observed by the next pass. Key
  deletion does not invoke user destructors. Pinned Bionic exposes only
  `KEY_VALID_FLAG | slot`; its generation stays in internal per-thread data.
  Consequently, using an integer key after deletion is POSIX-undefined and can
  alias a newly allocated key in both Bionic and this provider. Adding guest-
  visible generation bits would break Bionic's key validation ABI.
  Deleting a key that still has values on foreign live threads never calls the
  Android destructor. Its stale value occupies only the existing fixed slot;
  repeated delete/recreate overwrites that slot, so storage is bounded by 128
  entries per participating thread rather than delete count. There is no
  per-key host-key deletion and therefore no late Darwin destructor UAF or
  owning cell cycle. Concurrent application-level stale-key use remains POSIX-
  undefined, while provider state access is serialized by its table lock.
- Once controls retain Android's visible states 0/1/2, while wait ownership and
  condition state live in a side table keyed by the Android object address.
- Process-private mutexes own an independently initialized Darwin mutex
  in a side table. The Android 40-byte storage remains Android-formatted and is
  marked `0xffff` only after successful destruction, matching Bionic's visible
  destroyed state. Android arm64 mutex attributes are an eight-byte `long`:
  bits 0–3 select NORMAL=0, RECURSIVE=1, or ERRORCHECK=2; bit 4 is pshared and
  bit 5 is PI protocol. The provider preserves this guest representation but
  builds a separate Darwin mutex/attribute object. Recursive lock depth and
  errorcheck self-lock/wrong-owner behavior therefore come from the matching
  host primitive without casting either Android object. Pshared and PI bits
  return `ENOTSUP`; reserved bits return `EINVAL`. Destroy writes Bionic's `-1`
  attribute sentinel. POSIX makes subsequent use undefined; the provider
  deliberately returns `EINVAL` instead of resurrecting low type bits.
- Static process-private condition variables own independent Darwin condition
  objects in a side table. The pinned Bionic state word is preserved:
  `shared=0x1`, `monotonic=0x2`, counter step `0x4`, and destroyed marker
  `0xdeadc04d`. The zero initializer uses realtime; the monotonic initializer
  uses bit 1. `timedwait` accepts Android absolute deadlines. Realtime can be
  passed directly because both systems use the Unix epoch; monotonic deadlines
  are converted at the call boundary to Darwin's relative monotonic wait API.
  Cond and mutex lifecycle leases are acquired together before the host wait,
  so the host primitive atomically releases/reacquires the exact mutex owner
  while neither side-table entry can be destroyed. Spurious wakeups remain
  allowed and the Android fixture verifies the required predicate loop.
- Rwlocks accept only Bionic's all-zero static initializer and use a dedicated
  side-table state machine. The visible first word preserves pending-reader
  bit 0, pending-writer bit 1, reader-count step 4, and writer-owned bit 31;
  writer identity is kept separately as in Bionic. The default null-attribute
  policy is reader-preferred: concurrent readers may enter together and may
  barge while a writer is pending. The provider therefore does not promise
  starvation freedom beyond pinned Bionic semantics; the gate proves writer
  progress once finite reader churn drains. Writer-after-writer and reader-
  after-writer return Android `EDEADLK`, recursive reads are supported, wrong
  writer/unlocked release returns Android `EPERM`, and Bionic's global reader
  count means reader ownership is not tracked per thread.
  `pthread_rwlock_init/destroy` are absent from the pinned libc++ imports and
  remain absent from the closed resolver. Zero storage is initialized lazily;
  idle entries are reclaimed by provider reset. A prefixed destroy API exists
  only for lifecycle/sanitizer acceptance and returns `EBUSY` while held.

The singleton provider owns these tables for the process and must outlive every
loaded Android image. Its root state is intentionally process-lifetime so late
Darwin TLS destructors cannot access a destroyed C++ static. Participating
thread tables remain registered until their one host destructor runs. A
quiescent reset succeeds only with no owned thread handles, active Android keys,
or foreign thread table; it detaches and frees the caller's table. Mutex
tombstones and completed-once entries remain bounded
by Android object addresses until the loader's provider-reset/process-exit
boundary. Key generation prevents deleted-slot stale TLS values;
object-address tombstones prevent use-after-destroy from silently constructing
a new mutex, condition, or rwlock. Per-entry lifecycle locks serialize operations with
destroy; destroy waits for in-flight provider calls and reopens the entry if the
host reports `EBUSY`. Pthread errors are returned as Android/Linux numbers rather than
Darwin errno numbers (`EAGAIN=11`, `EDEADLK=35`, `ENOTSUP=95`,
`ETIMEDOUT=110`).

## Deliberate capability failures

Fork while once initialization is underway, robust mutexes, process-shared
mutexes/conditions/rwlocks, priority inheritance, condition attributes/explicit initialization, cancellation cleanup,
rwlock attributes/explicit initialization, timed/try rwlocks, and non-null
thread-create attributes are not implemented. Thread create/join/detach itself
is implemented only as one atomic provider-owned lifecycle module. Mutex
NORMAL/RECURSIVE/ERRORCHECK attributes are
implemented; their pshared/PI bits and pshared condition/rwlock flags return
`ENOTSUP`, while the related setter resolver symbols
are absent. POSIX makes destroying a condition with waiters undefined; this
provider deliberately returns Android `EBUSY` to avoid freeing host storage
beneath a blocked Android thread. There is no unprefixed interposition or
Darwin `dlsym` fallback.

The only Android-to-Darwin callback signatures exercised here are `void()` for
once, `void(void*)` for TLS destruction, and `void*(void*)` for a thread start
routine. All are fixed, register-only ARM64 signatures already covered by the
dual-PCS proof. This does not claim arbitrary callback ABI compatibility.

## End-to-end gate

```sh
tools/build-android35-bionic-pthread-provider.sh
```

The gate:

1. re-derives all 24 pthread imports and their `LIBC` versions from the pinned
   `libc++_shared.so`;
2. sparsely materializes ten exact Bionic implementation files without Git,
   including create, join, detach, and their state definition;
3. builds a Darwin arm64 provider archive and an NDK AArch64 ELF fixture;
4. loads that ELF with `aim-elf-loader` and an exact `libc.so@LIBC`
   resolver, then actually executes all 24 imports plus the explicit create
   lifecycle owner;
5. runs eight Darwin threads through the same loaded Android image, checks
   stable/unique thread tokens, two-pass TLS destructors, once-only execution,
   16,000 contended mutex increments, and concurrent destroy-versus-lookup;
6. proves the pinned Bionic deleted-key/same-slot reuse alias without claiming
   that POSIX-undefined stale-key use is supported application behavior;
7. runs an ASan/UBSan eight-thread delete-versus-get/set stress plus 10,000
   same-thread create/set/delete cycles. The live-value count remains one—not
   10,000—and returns to zero at quiescent reset, proving bounded fixed-slot
   ownership without per-key host destructor races or ownership cycles;
8. runs four Android ELF waiters through a predicate-loop wake, one signal,
   broadcast, monotonic absolute timeout, mutex reacquisition, pshared rejection,
   and destroy-with-waiter `EBUSY`, then runs 100 ASan/UBSan rounds with eight
   waiters racing the safe destroy boundary;
9. runs four concurrent Android ELF readers, proves a writer remains excluded
   then progresses after reader drain, checks recursive reads and wrong-owner
   `EPERM`, and uses quiescent reset as the lazy rwlock teardown boundary because
   `pthread_rwlock_init/destroy` are not imports of the pinned libc++ ELF. A
   separate 20-round ASan/UBSan gate drives 80,000 read and 10,000 write
   acquisitions, held-destroy `EBUSY`, wrong-owner unlock, and idle reset;
10. initializes NORMAL/RECURSIVE/ERRORCHECK attributes in Android ELF code,
   proves recursive depth, errorcheck self-lock `EDEADLK`, foreign unlock
   `EPERM`, destroyed-attribute `EINVAL`, and pshared/PI `ENOTSUP`. A 100-round
   ASan/UBSan stress repeats the same lifecycle including held-destroy `EBUSY`;
11. runs provider-created Android ELF thread routines through return-value join,
   running and already-exited detach, self-join `EDEADLK`, foreign-token `ESRCH`,
   and concurrent join-versus-detach. A separate 100-round ASan/UBSan stress
   proves exactly one lifecycle winner and requires a successful quiescent reset
   after every detached target exits.

Artifacts are written to `_build/bionic-pthread-provider`.
