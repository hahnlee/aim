# Bionic DSO lifecycle facade

This standalone provider owns the exact libc++ lifecycle imports
`__cxa_atexit` and `__cxa_finalize`. It stores the function, argument, and DSO
triple without reinterpretation, extracts an entry before invoking it, executes
matching callbacks in reverse registration order exactly once, and releases its
mutex around each callback. A callback may therefore register another callback
or recursively finalize without deadlocking; the next scan starts from the
newest live entry.

The behavior is pinned to AOSP Bionic `android-15.0.0_r1` commit
`361ba86734fb2821a6adcfdf775db8abd04e0de0`. Non-null finalize invokes a
provider-owned `__unregister_atfork` composition hook after draining. Global
`finalize(NULL)` drains every DSO plus null-handle registrations and invokes a
provider-owned stdio-cleanup hook, matching the source-pinned Bionic boundary.
No call is forwarded to Darwin `__cxa_*`, dyld, `dlopen`, or `dlsym`.

## Loader boundary

The production graph boundary keeps each mapped image executable through this
sequence:

1. After full-graph relocation, publish the image's unique mmap reservation
   before any constructor. `__dso_handle` stays local/hidden; the first
   `__cxa_atexit` lazily admits an exact non-null handle only when it lies in one
   live reservation.
2. On dependent-first graph teardown, freeze one range against new admissions,
   drain its admitted handles in callback LIFO order (including callback-created
   registrations), quiesce active callbacks, and unpublish the range.
3. Run that image's `DT_FINI_ARRAY` in reverse order and then `DT_FINI`, followed
   by unmapping. A lifecycle teardown error is fail-stop before finalizers or
   unmap; it is never silently ignored.

Independent ART graph owners may coexist. A process registry routes each call
by its disjoint live image ranges; it does not select a single current owner.
Only one standalone process-global owner may be active; multiple active owners
are reserved for range-backed image ownership.
Production image owners reject null-handle registrations, so graph teardown
never strands process-global callbacks. This registration policy is separate
from `finalize(NULL)`: global finalize drains registrations from every active
owner in process-wide registration-sequence LIFO order, including registrations
created by a callback. The standalone AOSP-semantics owner additionally accepts
null-handle registrations and owns the one post-drain stdio cleanup hook.

The Android-to-Darwin registration call contains only three pointer-class
arguments (`x0`-`x2`), and the reverse destructor call contains one (`x0`).
Those fixed-register slices are identical in Android AAPCS64 and Darwin arm64,
so no PCS thunk is required. This conclusion does not extend to variadic,
aggregate, HFA, or stack-spilled calls.

`audit.sh` builds a real Android AArch64 ELF with exactly the two imports. It
verifies exact triples, range-backed lazy handle admission, multiple concurrent
owners, null rejection for image owners, process-global inter-owner LIFO order,
recursive finalization, callback-created registrations, concurrent
register/finalize races, cleanup hooks, and rejection of unmap while an Android
callback is executing. A separate host C boundary harness runs the prefixed shim
under ASan and UBSan. The sanitizer claim covers that C ABI boundary only; the
exact-two-import Android ELF is executed directly and is not sanitizer-built.
