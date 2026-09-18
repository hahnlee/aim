# Native build graph transition

`darwin-art-xtask` owns the transition to a persistent Ninja graph. It keeps
the existing bootstrap command as the cold-start fallback, then promotes a
materialized runtime archive to real per-translation-unit edges using the
fingerprints and depfiles produced by `art-bootstrap`.

Generate the graph with:

```sh
cargo run -p darwin-art-xtask -- native-graph --out _build/native-graph/build.ninja
ninja -f _build/native-graph/build.ninja -n graphics-bootstrap
./tools/audit-native-graph.sh
```

`audit-native-graph.sh` is the structural gate: it requires the promoted ART
runtime, GraphicsJNI, and all 458 ICU TU edges, checks that compiler depfiles
are enabled, and requires a warm ICU target to report `no work to do`. It does
not infer cache hits from wall time alone. It also verifies that filesystem,
network, HWUI, graphics-phase, and graphics-input probe objects remain
separate warm no-op targets with narrow source edges.

On a clean checkout with no native fingerprints, the runtime target is a
single bootstrap edge. After that first successful build, graph generation
discovers the persisted runtime and graphics object commands and emits one
`native_cached_cpp` edge per object (214 objects per current ART flavor), plus
one deterministic archive edge per archive. The four independently owned
probe objects remain separate products. The old bootstrap command remains the
source of truth for the cold-start transition; the per-object graph is the
source of truth for subsequent incremental builds.

The input digest includes the graph generator version, repository paths, and
contents of the tracked production bootstrap/fixture/proxy closure. Each
digest gets a persistent `_build/native-cache/<digest>/` manifest and stamp.
Native objects, depfiles, fingerprints, and archives stay at stable
`_build/runtime-*` paths so direct bootstrap commands and Ninja share the same
cache. The graph declares the archive and every object as outputs; deleting an
object or the archive therefore causes Ninja to rebuild the missing product.
Compiler depfiles add transitive AOSP header edges after the first compile.
Unrelated acceptance probes do not invalidate the runtime archive, and each
filesystem/network/HWUI/graphics probe has its own narrow direct-input edge.
If an older persisted fingerprint has no depfile, the graph emits a one-time
preprocessing-only dependency-scan edge. It seeds `<object>.o.d` without
rewriting the object; the following graph generation promotes that TU to the
normal `deps = gcc` edge. This avoids silently accepting a legacy object whose
header closure is unknown while also avoiding a forced full recompilation just
to create metadata.
The patched ART shadow tree also has a content identity manifest. Preparation
does not rewrite an unchanged staged source or header, so a canonical fallback
after graph promotion preserves existing depfile mtimes and reuses the old
object fingerprints instead of recompiling the full ART archive.
The libartbase foundation follows the same rule: patches are applied in a
temporary candidate tree, only byte-different results are published, and its
native objects use dependency fingerprints. Repeating a DEX/probe command
therefore neither rewrites the global ART headers nor recreates unchanged
archives, which keeps the promoted runtime graph warm.

Standalone libcore Linux is also represented by explicit graph inputs for its
three native phases (`libcore_darwin_linux.cc`,
`libcore_darwin_linux_system_natives.cc`, and
`libcore_darwin_linux_syscalls.cc`) plus the shared header. The archive edge
therefore observes a system/metadata edit directly instead of treating the
shell builder script timestamp as a proxy for all of its C++ inputs.
The upstream ART runtime translation units and flavor-independent compat
adapters now use a shared `_build/runtime-common/{patched-source,objects}`
boundary. Graphics-only framework, libcore, and ICU adapters remain
flavor-local, while the roughly 200 common runtime objects plus the common
adapter set are archived into both products from one dependency-fingerprinted
cache. A cache identity stamp includes the common include set (including the
locked AOSP fmt headers); when that identity changes, graph promotion is
disabled until the canonical builder repopulates the shared cache. This avoids
silently mixing a graphics-only header ABI into the headless runtime and makes
the second flavor an adapter/link step rather than another ART cold build.
The identity is defined once in the dependency-free
`darwin-art-build-contract` crate and is also an explicit graph input, so a
post-prepare cache transition regenerates the graph instead of reusing a
pre-prepare canonical rule.
Each probe phase also has a stable content stamp under
`_build/runtime-probes/content-stamps/`. The graph generator updates a stamp
only when that phase's source/header bytes change, so a checkout that preserves
mtimes still recompiles only the changed probe object before the downstream
graphics link. The audit path regenerates the graph before its warm/no-op
check; repeated generation with unchanged bytes leaves all stamps untouched.

The Rust runtime owner is an explicit final-link edge at
`target/release/libdarwin_art_runtime.a`. Its Cargo sources are Ninja inputs,
but the archive is not a prerequisite of any C++ compile edge. A change to
`RuntimeSession`, provider lease state, or the engine ABI therefore rebuilds
the Rust owner and relinks the dylib without recompiling the ART/HWUI object
set. The graph audit checks this edge directly.

The same persisted-command promotion now covers ICU as well. The Android ICU
foundation keeps 201 common, 254 i18n, one stubdata, and two init translation
units under `_build/icu-foundation/objects/`; a complete set is emitted as
ordinary Ninja edges and a partial/interrupted set falls back to the locked
shell builder. The remaining foundation work is limited to the ART-side
bootstrap archive; it is intentionally separate from the graphics/ICU graph.

The graph now exposes that boundary as `graphics-foundation` (with
`foundation` as a short alias). It declares the stable HWUI static/APEX
archives and Android GraphicsJNI/registrar/force-loaded products. A complete
command-stamped object set is promoted to ordinary Ninja TU/archive edges;
otherwise the corresponding shell builder remains the safe cold/bootstrap
edge. The builders remain responsible for source locks, generated manifests,
ABI gates, and per-TU command stamps; changing a tracked foundation source or
lock invalidates only the foundation closure and not the ART runtime archive:

```sh
ninja -f _build/native-graph/build.ninja -n graphics-foundation
cargo run -p art-bootstrap -- build-graphics-foundation
```

GraphicsJNI promotion is active after its command/depfile materialization: 62
JNI objects plus the registrar are compiled in parallel and archived directly,
with the force-loaded object as a separate link edge. HWUI keeps the
shell-owned fallback only until its command stamps are materialized. Its source
lock remains strict: the builder verifies pristine AOSP files, applies the
tracked `patches/frameworks-base/0005-darwin-hwui-animation-pulse.patch` to a
stable shadow tree, and compiles from that tree. Ignored `_aosp` edits are
never consumed by the production graph.

The first foundation step is now landed in the two largest graphics shell
builders. `build-android16-android-graphics-jni.sh` keeps one command stamp per
registrar/JNI translation unit under `_build/android-graphics-jni/objects/`,
and `build-android16-hwui-static-foundation.sh` uses the same scheme under
`_build/hwui-static-foundation/objects/`. The stamp includes the compiler
identity-independent command line, all flags, and the source digest; an
interrupted compile cannot be reused because its object has no matching stamp.
Archives are still recreated from the cached objects, so the public archive
paths and member-count gates do not change. On the development machine the
graphics-JNI object audit was 74s cold and 11s warm (with no source changes),
while the registrar-only path was 5.0s cold and 3.4s warm. HWUI foundation
source identity remains separately locked; its 81+5 objects now use the same
cache after the tracked shadow patch is materialized, without weakening the
source pin.

ICU promotion uses the same source digest and command stamp policy. Its four
archives remain stable products, while Ninja owns the 458 translation-unit
edges once the shell builder has populated them. A warm `icu-foundation`
query is a true no-op; changing one ICU source invalidates only that object and
the affected archive.

## Current measurement

On the development machine, the materialized graphics graph currently
contains 630 cached C++ translation-unit edges (172 graphics/HWUI plus 458
ICU) and five archive edges. A clean
graph execution completed the 172-object parallel compile and archive in
75.25s (`ninja -j8`); the immediately repeated execution reported `no work to
do` in 0.01s. The remaining link/audit command is intentionally measured
separately because it includes the production dylib link and symbol gates.

The first promoted foundation slice is GraphicsJNI: after the shell builder
materialized its command stamps, Ninja rebuilt 61 JNI objects plus the
registrar in parallel in 9.69s, and the next archive query reported `no work
to do` in 0.01s. The HWUI 81+5 object set uses the source-locked shell edge
only during its first cache population.

The process probe is now split at the first stable boundary: production ABI
and installed-APK/framework environment validation lives in
`runtime/embedding/process_config.cc`, while fixture-mode validation lives in
`probes/runtime_fixture_options.cc`; both are separately cached objects. The
ART orchestration object therefore only changes when the process lifecycle or
JNI/HWUI implementation changes; adding a new fixture selector recompiles only
the fixture-options object and relinks the probe instead of recompiling the
full ART TU.

Shutdown/finalizer ordering is similarly split across production owners:
`runtime/embedding/process_shutdown.cc` coordinates the lifecycle,
`runtime/framework/app/process_shutdown.cc` quiesces framework threads, and
`runtime/art/vm_shutdown.cc` detaches/destroys ART. The probe retains only the
post-VM fixture assertions and acceptance reporting before it asks the
coordinator to complete process-state cleanup.

The runtime adapter uses the same per-TU cache boundary. Platform-only
compatibility shims are compiled as `darwin_runtime_platform_stubs.cc`, while
the graph/provider/JNI adapter remains `darwin_runtime_adapters.cc`. The
archive cache records both source names and command fingerprints; if a new
required TU is absent from a persisted archive, the builder falls back to the
canonical compile path instead of silently reusing an incomplete archive.
Runtime and graphics bootstrap cache promotion uses flavor-specific required
TU sets (`darwin_icu_natives.cc` versus `darwin_icu_jni_bridge.cc`), so a
headless archive no longer falls back merely because the graphics-only ICU
bridge is absent. Promoted depfiles are read from the canonical `<object>.o.d`
name, avoiding the legacy rule for every object. On the reference machine the
first Ninja promotion after this fix rebuilt 221 objects in 159.28s; the next
graph query reported `ninja: no work to do` in 0.03s.
The process-wide NativeBridge/NativeLoader hooks are a third independent
object, `darwin_native_bridge_stubs.cc`. The filename is historical: this
translation unit now compiles the pinned Android 16 libnativebridge state
machine and adds only the Darwin host wrappers needed to prefer translated
Android ELF/dylib trampolines. Per-image ELF trampoline selection remains in
the graph-aware adapter, and NativeLoader retains Android's bridge fallback
after the native Darwin load attempt.
Descriptor-to-shorty planning is a fourth cached object,
`darwin_jni_shorty.cc`, so changing the regular JNI proxy does not rebuild the
parser and vice versa.

Proxy environment/class lookup is a fifth cached object,
`darwin_jni_proxy_lookup.cc`. Its command includes the internal graph-state
header and provider/DSO ABI headers, so changes to lookup policy invalidate only
that object and the final runtime link; RegisterNatives and trampoline code
remain independently cacheable.

The small exception bridge is a sixth cached object,
`darwin_jni_proxy_registration.cc`. It shares the internal state header but not
the registration transaction, keeping ThrowNew-only changes out of the
trampoline/rollback object.

The remaining graph-aware adapter is now partitioned into four more cached
objects. `darwin_runtime_elf_lifecycle.cc` owns image publish/finalize and
provider teardown, `darwin_runtime_elf_resolver.cc` owns closed provider symbol
resolution and fixture route accounting, `darwin_runtime_native_loader.cc`
owns trusted-directory discovery plus NativeLoader open/close, and
`darwin_runtime_jni_registration.cc` owns RegisterNatives and trampoline
publication. A change to provider routing, loader discovery, teardown policy,
or JNI registration therefore invalidates only that object plus the final link.

After these boundaries were promoted, warm runtime and graphics bootstrap
commands both reached the native `ninja: no work to do` path in about 3 seconds
including the Cargo/xtask wrapper. Runtime link closure remained
`undefined=0`, and recursive ELF/JNI/TLS acceptance continued to pass after
each split.

The runtime link now also force-loads the small Rust `darwin-art-runtime`
static library. Its opaque native-owner symbols are exported and audited in
both CPU and graphics dylibs; the Rust archive is built incrementally by Cargo
and does not participate in the large AOSP/HWUI object promotion.

The Darwin sigchain adapter remains part of the shared runtime-common object
boundary. Its user-handler dispatch now restores the fault context's signal
mask after the Mach SIGBUS-to-SIGSEGV bridge calls an application action as an
ordinary function, recreating the kernel `sigreturn` boundary used by AOSP.
The incremental graph rebuilt only `darwin_sigchain.cc.o`, its shared archive,
the JIT edge, and the graphics link/audit closure. The link audit, the focused
direct-dispatch mask smoke, and unchanged `004-SignalTest` interpreter/JIT
execution all pass without a test-specific shutdown path.

The `compat/darwin_art_abi_layout.cc` TU is a deliberately tiny native ABI
gate. It statically asserts the C process/result/surface layouts exported by
`include/darwin_art/darwin_art.h` and `compat/darwin_surface_bridge.h`; the
Rust `darwin-art-engine-sys` crate asserts the same offsets independently.
This keeps the raw FFI declarations narrow without making C++ layout drift a
runtime-only failure.

Graphics presentation now follows the same boundary. The JNI/widget
validation and `present_content`/interactive-root orchestration live in
`probes/runtime_graphics_phase.cc`, while clickable hit-testing, pointer/frame
dispatch, and input state live in `probes/runtime_graphics_input.cc`.
RenderNode/Metal replay is owned by `probes/runtime_hwui_probe.cc`; the
remaining graphics probe owns Java recording/orchestration state. The linker
consumes all phase objects, so a change to framework validation or input
handling does not recompile the HWUI/Skia implementation and a graphics
implementation change does not rebuild the orchestration phases.

NativeBridge run-test staging keeps canonical compiled dylibs as shared build
outputs, then copies the declared bridge and native-test fixture namespace into
each mode's disposable action root. This lets unchanged typed `run.py` actions
create their process-visible aliases without granting write authority to the
shared output directory or weakening path confinement. The production runtime
still exercises the pinned NativeBridge state machine, namespace, trampoline,
JNI lifecycle, and signal callbacks; the staging copy adds no alternate loader
or test-specific runtime path. The focused state-machine/control-flow audit and
unchanged `115-native-bridge` interpreter/JIT exact-output run pass.
