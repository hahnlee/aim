# Register-only Android ELF JNI fixture

This fixture is the first ART native-bridge acceptance target. It is compiled
as an Android ARM64 ELF root with a real two-level sibling graph
(`root -> child -> grandchild`) plus explicit virtual Bionic/host providers.
The root exports only `JNI_OnLoad` and `JNI_OnUnload`; the leaf is constructor-
free so the existing observable child/root lifecycle ordering stays exact.

The root constructor also opens, reads, and closes synthetic `/dev/random`.
That call sequence proves the process-wide filesystem owner is installed before
constructors and is visible without a pthread-local activation guard.

The same constructor calls the versioned `sscanf@LIBC` and `vsscanf@LIBC`
routes. It checks raw IEEE binary128 `%Lf` output and an Android 32-byte
`va_list` whose pointer arguments spill after the GP register bank while FP
distractors remain in the independent SIMD bank.

It also executes `swprintf@LIBC` with Android wchar32 output,
`ioctl@LIBC(RNDGETENTCNT)` through the process filesystem owner, and
`strftime_l@LIBC` against the runtime's deterministic fixed-offset UTC owner.
The writable `/data` overlay is exercised with `O_CREAT|O_TRUNC`, null-offset
and explicit-offset `sendfile@LIBC` calls, then close/reopen/read verification
of both destination files.

`JNI_OnLoad` exercises the reverse ABI boundary through the supplied proxy
`JavaVM` and `JNIEnv` tables: `GetEnv`, `FindClass`, and `RegisterNatives`. It
registers eight methods covering mixed scalar/reference/FP stack tails,
naturally packed narrow integer stack values, scalar/reference/FP/void returns,
and a post-load proxy `JNIEnv` call. A later ART acceptance invokes all of them
through generated Darwin-to-Android regular-JNI shorty trampolines. The fixture
must never receive ART's Mach-O JNI tables directly.

The mixed method deliberately places two 32-bit floats before a 64-bit double
after exhausting the FP register bank. Its Darwin stack offsets are compact
while Android AAPCS64 assigns an eight-byte slot to each argument, so merely
calling the ELF address directly cannot pass the gate.

`nativeUsesEnv` calls `GetVersion` and `FindClass` after `JNI_OnLoad` returns.
The backend resolves the current ART thread's `JNIEnv` for that invocation; it
does not preserve the load thread's environment pointer.

Run `tools/build-android-elf-jni-fixture.sh` to produce the locked-shape ELF in
`_build/android-elf-jni-fixture`. Building the ELF alone does not claim that
ART loading or either JNI ABI bridge is complete.
