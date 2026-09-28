# Darwin arm64 native-call PCS boundary

Android's AArch64 PCS assigns every fixed stack argument an eight-byte slot.
Darwin arm64 instead packs fixed arguments at their natural size and alignment:
`Z/B=1`, `C/S=2`, `I/F=4`, and `J/D/reference=8`. ART's generic JNI
trampoline therefore cannot reuse its Android AAPCS64 stack layout on macOS.

`patches/art/0029-darwin-arm64-native-stack-pcs.patch` changes the runtime
generic-JNI frame builder to track Darwin stack offsets in bytes, align each
spilled argument, and write it at its native width. It also makes the
CriticalNative runtime frame-size calculation use the same source-order width
and alignment rules. `probes/Hello.java` and `probes/runtime_link_probe.cc`
exercise integer, floating-point, reference, 64-bit, and interleaved narrow
spills through a registered JNI method before either graphics backend runs.

This does not yet close compiler-generated native calls. The current sparse ART
materialization contains the runtime trampoline but not the complete optimizing
compiler/JNI-stub backends. Before enabling JIT/AOT or direct compiled
CriticalNative calls, materialize the pinned compiler subtrees, identify every
ARM64 outgoing-native-argument layout, apply the Darwin PCS there, and run the
same spill matrix through compiled and interpreted call sites. Until that gate
exists, this port deliberately uses the generic runtime JNI path.
