# Darwin/Android dual ARM64 PCS proof

Apple ARM64 and Android AAPCS64 agree while fixed-prototype arguments remain in
the eight general and eight floating-point argument registers. They diverge
when narrow primitive arguments spill. Android gives every spilled argument an
eight-byte slot; Darwin compacts arguments by their natural size and alignment.

The pinned signature exhausts both banks and then spills, in order:

```text
Z  B  C  S  I  J  reference  F  D
```

Its Android stack offsets are `0,8,16,24,32,40,48,56,64` in an 80-byte aligned
area. Darwin uses `0,1,2,4,8,16,24,32,40` in 48 bytes. Signed `B`, `S`, and `I`
are sign-extended by the receiving callee; `Z` and `C` are zero-extended;
`F/D` are moved by raw IEEE bits; references and `J` remain 64-bit.

`pcs_thunks.S` is compiled twice from the same source: as an NDK r28c API 35
AArch64 ELF fixture, and as Mach-O ARM64 code. The standalone native executable
proves three distinct paths:

1. A register-only mixed integer/FP call is direct and needs no thunk.
2. Darwin compact stack is repacked into Android eight-byte slots before the
   Android callee.
3. An Android caller frame is repacked into Darwin compact form before an
   indirect Mach-O C++ callback.

All paths compare deterministic digests, including negative narrow primitives,
raw float/double bits, a 64-bit reference value, and arguments beyond both
register banks. The Rust orchestrator consumes the current
`aim-elf-loader` through its public API and executes the fixture's
register-only no-argument export; it does not modify or introspect loader state.

Run:

```sh
tools/build-android-arm64-dual-pcs-proof.sh
```

This is an algorithm proof for future `NativeBridgeGetTrampoline2` and proxy
JNIEnv generation. It supports only statically known, fixed, non-variadic
signatures. It does not claim variadic ABI support, aggregate/HFA handling,
arbitrary return repacking, or a complete generated JNI function table.

