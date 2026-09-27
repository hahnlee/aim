# Android 16 JNI proxy table gate

This standalone module supplies a proxy `JavaVM`/`JNIEnv` surface for an
Android ARM64 JNI library without exposing an ART function table. Every
`JNINativeInterface` entry except the four reserved slots is populated: most
forward to the current ART environment, and the Android-ABI-sensitive ones go
through the backend (see below). `manifests/abi.tsv` records the decision for
each entry.

The table shape comes from the SHA-locked AOSP Android 16.0.0_r4
`libnativehelper/include_jni/jni.h`. `tools/generate_slots.py` parses every
declaration, locks the ordered-name digests, and generates the table sizes and
selected offsets. The gate verifies 233 `JNINativeInterface` slots and eight
`JNIInvokeInterface` slots, then also proves that the installed NDK compiler
header has the same ordered tables.

## Boundary and state

The public initializer accepts three semantic callbacks plus an optional
`current_env` accessor. Forwarding wrappers call that accessor for one operation
and use the host table internally; they never store or return it. The handles
visible to the Android ELF always begin with this module's own generated proxy
table. The backend owns class handles and native registration; the proxy owns
the pending-exception bit and the attached test environment.

The proxy is JNI 1.6. A registered native function pointer is transferred to
the backend but never invoked by the proxy; it remains Android-ABI-owned, and
the backend must call it through a signature-audited Android-to-ART bridge.
Registering it directly with ART is forbidden. A backend retaining
`JNINativeMethod` metadata must copy it while the callback is active.

The only null entries are the four reserved slots. A backend without the
optional `call_nonvirtual_method_v` makes the nonvirtual `...`/`...V` entries
return zero instead of calling ART; everything else forwards to the current
ART environment. In the standalone fake backend `ThrowNew` records only a
pending bit; the ART backend forwards through its current environment.

## ARM64 procedure-call classification

Every implemented entry, including the `JNI_OnLoad(JavaVM*, void*)` fixture
entry, has a fixed non-variadic prototype containing only pointers and integer
scalars. No call exceeds five integer registers. Android AAPCS64 and Darwin
ARM64 PCS therefore agree for this exact set and no thunk is needed. In
particular, `RegisterNatives` passes a pointer to `JNINativeMethod`; it does not
pass the three-pointer aggregate by value, and compile-time layout checks cover
the pointee.

The remaining entries fall into three shapes:

- Variadic `Call<Type>Method`, `CallStatic<Type>Method`, `NewObject` and
  `CallNonvirtual<Type>Method`: an assembly thunk (`src/aapcs64_call.S`)
  captures the AAPCS64 argument banks and caller stack before entering Darwin
  C, which builds the Android `va_list` from them (the nonvirtual form has
  four named arguments, so its variadic arguments start at x4).
- `...V` entries receive an Android `va_list`, passed by reference. The
  backend's `call_method_v` / `call_nonvirtual_method_v` decode it against the
  descriptor recorded at method lookup and call the host `...A` entry.
- `...A` entries pass a `jvalue` array; its 8-byte union layout is the same
  under both ABIs, so they forward directly. The per-entry decision is
  recorded in `manifests/abi.tsv`.

## Executable evidence

`audit.sh` fetches and hashes the pinned Android 16 header, regenerates the slot
constants, and compiles `probes/fixture.c` with the Android AArch64 clang target
as an ELF64 shared object. API 35 is the installed r28c compiler wrapper, while
the compilation uses the pinned Android 16 `jni.h`; the JNI 1.6 table ABI is
identical and independently digest-checked against the NDK header.

The ELF exports a real `JNI_OnLoad`. Its no-argument runner obtains the proxy
`JavaVM` through its sole resolved import, then Android-compiled instructions
indirectly calls `GetEnv` and the original five `JNIEnv` slots. Native-method
execution in the ART integration additionally exercises all fifteen forwarding
slots after `JNI_OnLoad`: it round-trips a byte array through region access,
transitions it through local/global/local references, and deliberately raises,
observes, and clears an array-bounds exception. The Darwin process maps and
executes that ELF through the isolated ELF loader, and a fake Darwin backend
verifies both class names, the `JNINativeMethod` tuple, and the thrown exception
message. The fixture also walks the proxy table from Android code (only the
reserved slots may be null) and makes Android-compiled variadic
(`CallLongMethod`, `CallStaticDoubleMethod`, `CallNonvirtualIntMethod`) and
`va_list` (`CallNonvirtualFloatMethodV`) calls whose integer, long, float and
double arguments the fake backend decodes and checks. Disassembly gating requires the expected indirect `blr` calls, so a
Darwin-only substitute fixture cannot satisfy the test.

Run `tools/android-jni-proxy/audit.sh`. It also checks exact exported/imported
symbols, absence of ART internals and `dlsym`, host and Android structure
offsets, that only the reserved slots are null, invalid `GetEnv` behavior, ASan/UBSan, Rust
formatting, and Clippy.

The referenced AOSP header is Apache-2.0 licensed. This repository stores only
source coordinates, hashes, derived slot numbers, and ordered-name digests; it
does not copy the upstream header.
