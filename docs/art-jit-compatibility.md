# ART JIT compatibility status

This is the current JIT contract and verification index. Historical corpus
shards, experiments and per-run checkpoints live in Git history. Keep this
file focused on what is implemented, what the tests prove and what remains.

## Target

Host the pinned AOSP ART ARM64 JIT natively on macOS with the normal Android
application execution contract. Darwin-specific code may implement Mach-O,
unwind, W^X and signal mechanisms, but it must not add method or bytecode
admission restrictions absent from upstream ART.

Interpreter fallback is a normal ART mechanism, but it does not prove that a
missing compiled feature works. Likewise, compilation alone does not prove the
generated code executed correctly. Claims require method-level execution or an
unchanged application exercising the compiled path.

## Current state

- Production JIT is enabled by default. `-Xusejit:false` disables it through
  the normal runtime option path; contradictory host/runtime requests fail.
- Nterp and optimized ARM64 execution are linked from pinned AOSP sources.
- Darwin-only bytecode and method-shape admission gates have been removed.
- Managed references use the required compressed representation at compiled
  boundaries; native pointers remain native in JNI/runtime/deoptimization data.
- Mach-O runtime stubs, deterministic CFI/unwind providers, signed `MAP_JIT`
  W^X transitions and native JNI trampolines are implemented.
- Native-thread JNI attachment is ownership-aware: already attached threads
  are borrowed, owned attachments detach explicitly, and async workers join
  before VM/library teardown.
- Android signal-context recovery now applies guest handler changes back to
  the Darwin interrupted context, enabling V8 and generated ART code to resume.

## Verified feature matrix

| Family | Current evidence |
| --- | --- |
| Arithmetic and conversion | IJFD arithmetic, division/remainder, comparisons, shifts, narrowing and edge values execute in compiled lanes |
| Control flow | branches, packed/sparse switches, scalar loops, phi values and suspend checks |
| References and roots | quick/JNI/runtime-helper arguments and returns, class/string/method-type roots under moving GC |
| Fields and arrays | resolved/unresolved, static/instance, volatile, primitive/reference, covariance and exact failure recovery |
| Allocation and types | objects, constructors, arrays, casts, `instanceof`, class initialization and failure/concurrency |
| Invokes | static/direct/virtual/interface/super/native/range, unresolved calls, polymorphic and invoke-custom/CallSite |
| Exceptions | typed/catch-all handlers, finally, rethrow/replacement, pending state and compiled unwind |
| Synchronization | object/class monitors, synchronized methods, recursion, inflation, contention and exceptional release |
| GC | Baker/ConcurrentCopying, moving roots, allocation pressure/OOME, stack maps and post-GC compiled calls |
| Deoptimization | speculative virtual/reference and mixed-wide reconstruction, including moving-GC OSR references |
| OSR | explicit and automatic IJFD loops, exception paths, references, moving GC and deoptimization |
| Intrinsics | specialized HIR plus Unsafe, String, Math, CRC32, Memory, Reference, boxing and typed arraycopy families |
| Native boundary | JNI scalar/reference ABI, independent GP/FP state, Android stack layout, shorty/cache identity and W^X trampolines |
| Applications | Calculator arithmetic, DeskClock, Chromium/V8 rendering and navigation, and Blue Archive first-frame native execution |

Broad corpus totals are regression evidence, not proof that every method was
optimized. Upstream ART may legitimately decline individual methods. A gap is
Darwin-specific only when the same pinned upstream configuration would admit
the method but this runtime cannot execute it correctly.

## Validation rules

1. Compare interpreter and compiled results, including expected exceptions.
2. Verify that the intended method acquired and executed compiled code.
3. Exercise nulls, integer boundaries, spills, high offsets, GC during calls
   and concurrency appropriate to the feature.
4. Record runtime, boot image, native graph and test input identities; stale
   output does not count.
5. Preserve negative protection tests. The expected W^X SIGBUS is an oracle,
   not a runtime crash to suppress.
6. Do not infer universal compatibility from smoke-test counts or one app.

## Verification

The primary focused gate is:

```sh
bash tools/audit-art-jit.sh
```

Related boundary gates:

```sh
bash tools/audit-android-jni-trampoline.sh
bash tools/test-hwui-jni-attachment.sh
bash tools/bionic-process-state-facade/audit.sh
bash tools/audit-headless-artifact-identity.sh
bash tools/audit-native-graph.sh
cargo test --workspace
```

Application acceptance must use unchanged APKs and keep JIT enabled. The core
application gate is `tools/aosp-core-apps-graphics-acceptance.sh`; Chromium
acceptance additionally proves V8 generated-code signal recovery, child
processes and GPU surfaces.

## Known limits

- The existing focused matrix is not the complete upstream ART test corpus.
- Nested/irreducible OSR loops, exhaustive deoptimization materialization,
  reflection/proxy corners and larger concurrent reference-processing stress
  require broader coverage.
- Blue Archive first-frame and Cancel evidence is not login or gameplay proof.
- Sustained workload, memory growth and long-running concurrent JIT/GC behavior
  need dedicated soak gates.
- App acceptance must be repeated whenever boot image, runtime, compiler,
  trampoline, unwind or signal identities change.

## Latest verification

### 2026-09-16

- The Android signal-context regression passes: a guest handler advances PC
  after a deliberate fault and execution resumes through the Bionic facade.
- Unchanged Chromium/V8 rendered `example.com`, completed three physical tab
  transitions and retained its main/GPU/renderer processes for over seven
  minutes without a generated-code fault or runtime abort.
- The graphics runtime incremental link audit passes with no fake symbols or
  host ICU/fmt/CoreText leakage.
- Calculator `2+3=5` and DeskClock Timer pass after the Chromium run on the
  common JIT + HWUI + SurfaceFlinger + Metal runtime.

### Document maintenance

Append only new, verified changes here. Keep at most ten dated entries and
fold older facts into the feature matrix or known limits. Git history is the
archive for removed per-run diagnostics and corpus shard logs.
