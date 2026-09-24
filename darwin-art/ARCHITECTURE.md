# Darwin Android compatibility architecture

## Product rule

AIM preserves every behavior that an Android application can observe and
uses macOS for the underlying mechanism. This is the same boundary Wine uses:
guest ABI and namespace above, host kernel and devices below.

```text
Android application behavior
  ART / Framework / Bionic ABI / Android resources and paths
                              ↓
Darwin compatibility boundary
  ELF loader / Bionic facade / path namespace / service bridges
                              ↓
macOS mechanisms
  Darwin VM and threads / APFS / sockets / Metal / CoreAudio / AppKit
```

An optimization is valid only if the application cannot distinguish it from
Android behavior. An Android `Button` remains a View rendered by HWUI/Skia; it
is not replaced by `NSButton`. Android `O_*`, errno, `stat`, pthread, and path
semantics remain Android/Linux values; Darwin values stay behind the boundary.

This gives every boundary an explicit owner:

| Surface | Guest-visible owner | Host mechanism |
|---|---|---|
| paths, descriptors, uid/gid, modes | Android prefix and Bionic facade | preopened Darwin directories and private stores |
| native libraries and symbol scope | Android ELF graph loader and virtual DSOs | Mach VM mappings; never dyld lookup for ELF imports |
| threads, TLS, clocks, errno | Bionic-compatible state owners | Darwin pthread, Mach clocks, and parking primitives |
| UI, graphics, audio, integration | Android Framework/HWUI-facing bridges | AppKit, Metal, IOSurface, CoreAudio, broker services |
| fonts and resources | Android resource/font configuration | pinned prefix files; optional host assets only through providers |

Host facilities implement an operation; they do not define its observable ABI.
An API with no reviewed translation stays a capability failure instead of
falling through to a similarly named Darwin symbol.

## Target process model

The runtime loads ART into a Rust-owned Darwin process. Profile-global state and
Android framework services are deliberately split so privileged host ownership
does not move into Java:

```text
darwin-artd (per-profile Rust broker)
  ├─ package and prefix ownership
  ├─ process leases and persistent child ownership
  └─ case-sensitive Android filesystem lifetime

android.system (persistent ART system_server-lite)
  ├─ Android Binder package registry
  ├─ installed package, Activity, Service, and launch-Intent resolution
  └─ future Android framework service implementations

Android application process (sandboxed Darwin process)
  ├─ ART and Android Framework
  ├─ application DEX
  ├─ custom ARM64 ELF loader
  ├─ Bionic ABI facade
  └─ Android ARM64 .so code

macOS UI process/broker
  ├─ NSWindow and NSEvent
  ├─ IOSurface and Metal
  └─ CoreAudio and other user-facing frameworks
```

Android application processes should be separate Darwin processes, matching
Android's failure and security isolation. A shared broker owns global services;
it does not execute application JNI. A Linux VM remains the planned fallback
for binaries that require real kernel behavior, but VM implementation is
explicitly later work. The current native path neither embeds a Linux kernel
nor silently crosses between VM pointers and host ART/JNI pointers.

`darwin-artd` is the privileged mechanism owner, not an Android API surface.
It starts and reaps `android.system`, stores immutable install records, and
keeps the profile mounted while processes hold leases. The ART-backed
`android.system` process exposes those records through Android `Binder` and
`Parcel` contracts. Application `PackageManager` calls therefore cross a
process boundary before resolving another installed package or component;
they do not inspect another APK or inherit its launch metadata as environment
variables. Android Service instances retain normal `Service.onCreate`,
`onBind`, Binder transaction, rebind, unbind, and process teardown semantics;
the Rust host only performs the Darwin process creation requested by that
framework lifecycle.

### Input and physical keyboards

Host mouse and keyboard events must enter Android's normal window input path:
`InputChannel -> InputEventReceiver -> ViewRootImpl -> View`. The Mac keyboard
is exposed as an external Android `FULL` keyboard, while Android's virtual
device id remains a fallback rather than an alias for the host device.

The common event path is implemented, but the current US-QWERTY
`KeyMapGetCharacter` table is explicitly a bootstrap bridge. Android uses
per-device `.kl` and `.kcm` data for scan-code, key-code, modifier, character,
dead-key, label, and fallback semantics. The compatibility gaps, Rust-owned
replacement architecture, milestones, and completion criteria are tracked in
[`docs/physical-keyboard-compatibility.md`](docs/physical-keyboard-compatibility.md).

## Virtual Android prefix

Android code must never depend on host absolute paths. Each runtime profile has
a Wine-prefix-like Android root:

```text
~/Library/Application Support/DarwinART/prefixes/default/
  system/                       read-only runtime image
    etc/fonts.xml
    fonts/Roboto-Regular.ttf
  system_ext/                   read-only framework extension
  product/                      read-only/product overlays
    etc/fonts_customization.xml
  vendor/                       compatibility files when required
  apex/                         extracted APEX runtime assets
  data/
    app/                        installed APK state
    user/0/<package>/           private application data
    dalvik-cache/               generated runtime artifacts
    fonts/                      Android-managed/updatable fonts
  storage/emulated/0/           user-visible Android storage namespace
```

A mount table maps Android paths to providers. Resolution uses longest-prefix
matching after Android-compatible normalization:

| Android path | Provider |
|---|---|
| `/system`, `/product`, `/apex` | immutable runtime-prefix directories |
| `/data/user/0/<package>` | package-private case-sensitive storage |
| `/storage/emulated/0` | user-approved shared-folder provider |
| `/mnt/host/<name>` | explicit macOS share backed by a security-scoped bookmark |
| `/proc`, `/sys`, `/dev` | synthetic providers, never host directories |

The implemented runtime installs one process-wide filesystem owner before app
code or ELF constructors run. Immutable runtime assets and installed package
code are separate from package-private, writable `/data`. The latter resolves
through the Rust filesystem facade to the profile's case-sensitive APFS volume;
SQLite databases, journals, and atomic SharedPreferences XML therefore survive
application exit, daemon restart, and relaunch. Exact synthetic `/dev/random`
and `/dev/urandom` devices remain namespace-owned. Every operation takes a short
owner lease, and teardown stops admission and drains those leases only after
ELF finalizers and unmapping.

The private prefix must be case-sensitive even when the user's main APFS volume
is not. The preferred product representation is a sparse, case-sensitive APFS
image or an equivalently isolated case-sensitive volume. Shared host folders
retain host semantics and are identified as such to Android callers. Symlink
escape, `..` traversal, and mount crossing are checked before a host file
descriptor is opened.

The namespace owns virtual uid/gid, modes, Android ownership tags, and any
metadata macOS cannot represent directly. File descriptors contain real Darwin
descriptors after resolution, but Android-visible flags, errno, and structures
are converted at every ABI boundary.

### Shared files

The macOS broker obtains user permission with security-scoped bookmarks and
publishes a mount inside `/storage` or `/mnt/host`. Android pickers and share
intents should return content URIs or Android paths, not `/Users/...` paths.
For operations that cannot safely expose a directory, the broker passes an
already-open file descriptor.

### Fonts

The deterministic baseline uses the Android release's Roboto/Noto files,
`fonts.xml`, and product customization XML inside the prefix. Minikin,
HarfBuzz, ICU, FreeType, and Skia retain Android fallback, shaping, and raster
behavior.

CoreText may later enumerate host fonts, but it is only a provider. Approved
font files are published through synthetic `/data/fonts` entries and an
Android font configuration. Android code must not depend directly on
`/System/Library/Fonts` or CoreText layout behavior.

The same rule applies to resources: compiled APK/framework resources, locale
data, and overlays retain Android lookup and qualifier rules inside the prefix.
NSBundle paths, asset catalogs, and host font fallback are not guest-visible
substitutes.

## Native ARM64 ELF execution

Apple Silicon can execute Android ARM64 instructions directly, but dyld cannot
load Android ELF and Darwin cannot service Linux syscalls. Native support is
therefore two separate components.

### ELF loader

The target loader boundary owns:

1. ELF64/AArch64 validation and executable-segment policy;
2. `PT_LOAD`, `PT_DYNAMIC`, RELRO, and memory protection;
3. AArch64 RELA relocations, GNU/SysV hash, symbol versions, and weak symbols;
4. dependency namespaces and Android linker search rules;
5. constructors/destructors and `dlopen`/`dlsym`/`dlclose`;
6. Bionic TLS, thread-local destructors, and module IDs;
7. ELF unwind metadata and Android `libc++_shared.so` exception behavior;
8. JNI discovery and `JNI_OnLoad`/`JNI_OnUnload` lifetime.

Items in that list are responsibilities, not a claim that every one is
complete. Recursive `DT_NEEDED` graphs, eager supported RELA relocation,
per-image constructors/finalizers, GNU RELRO, and the pinned libc++'s zero-valued
`DT_AARCH64_BTI_PLT` metadata are implemented. Nonzero BTI requirements fail as
an explicit capability because Darwin execution has not yet proved their
landing-pad contract. A bounded local-definition AArch64 `TLSDESC` path now
owns per-Darwin-thread guest TLS blocks without replacing `TPIDR_EL0`.
Imported/static TLS models, graph-wide module indexes, C++ thread-local
destructors, and general ELF unwind registration remain open.

The compatibility loader maps the original ELF directly when no complete
Darwin installation artifact exists. Installation may additionally produce a
Mach-O accelerator, but that product is admitted only as a fully Darwin arm64
dependency graph: dyld loading, Darwin PCS, libSystem/pthread/TLS/unwind, and
macOS platform metadata throughout. Renaming or repacking an Android-ABI image
as `.dylib` is forbidden. Rust owns artifact admission, graph selection,
dependency policy, and the mount namespace. C/C++ and small ARM64 assembly
thunks own varargs, TLS, unwind, and ABI entrypoints where the fallback ELF
path requires exact Bionic layout.

The guest-visible identity remains Android's `lib*.so`. The installer stores
Darwin products in a separate hash- and runtime-ABI-bound namespace. Selection
is graph-atomic: either every library in the closed graph passes the complete
Darwin contract and dyld owns the graph, or the entire graph uses the Android
ELF compatibility loader. A mixed ELF/Mach-O graph is not a supported backend.
The exact install and admission contract is documented in
`docs/apk-native-artifacts.md`.

### Dual ARM64 calling conventions

Identical instruction sets do not imply an identical C ABI. Android ELF uses
the AArch64 Procedure Call Standard used by the NDK; Apple ARM64 compilers use
Darwin's stack-argument packing rules. Register-only calls commonly agree, but
spilled integer arguments do not. This project already requires Darwin packing
when ART invokes its Mach-O framework JNI implementations, while an Android ELF
JNI implementation expects Android packing.

Native integration therefore has two explicit thunk directions:

1. `ART -> Android ELF native`: ART's native-library handle is marked for the
   compatibility bridge. `NativeBridgeGetTrampoline2` uses the method shorty
   and JNI call type to select or generate a Darwin-to-Android call thunk.
2. `Android ELF -> JNIEnv/JavaVM`: the library receives proxy JNI invoke
   tables whose Android-ABI entry thunks repack arguments, substitute the real
   ART environment/VM pointer, and call the Darwin-ABI implementation. Passing
   ART's Mach-O `JNIEnv` table directly is forbidden even though simple JNI
   calls may appear to work.

The same rule applies to virtual DSO functions: public entrypoints must accept
the Android ABI and cross into prefixed Darwin implementations through a
reviewed thunk where their signatures can spill or use varargs. `JNI_OnLoad`
itself has only two pointer arguments and is a useful first execution gate, but
it does not prove general JNI calling-convention compatibility.

The concrete ART ownership and teardown seam is specified in
`docs/art-elf-native-loader-integration.md`.

### Rust ownership boundary

`darwin-art-runtime::RuntimeSession` is the owner-thread lifecycle machine.
Engine, ELF namespace, provider, network, filesystem, input, and surface
leases are concrete typed fields in the Rust owner graph and are released in
reverse dependency order; normal uninstall consumes the resource before its
lease can be reused, while `Drop` performs best-effort rollback for partial
bootstrap failures. Provider lease accounting lives in the Rust runtime crate
as `ProviderLeaseTable`.
The host's only unsafe provider code is a thin callback adapter that converts
the native function table into that table; it does not own counts or teardown
policy. `darwin-art-engine-sys` is the single POD/function-pointer ABI
definition and centralizes construction/version checks for process config and
results. The migration details and remaining boundaries are tracked in
[`docs/architecture-migration.md`](docs/architecture-migration.md).

Native build ownership follows the same boundary. `art-bootstrap` persists a
dependency fingerprint and compiler command per native object, while
`darwin-art-xtask` promotes those records to per-object Ninja edges after a
cold bootstrap. The CLI is now physically split by change domain: `source/`
contains locked materialization and foundation/Skia builders, `audit/` owns
CPU and graphics link audits, `probe/` owns runtime/graphics/fixture probe
commands, `native_probe/` owns the small C++ probe builders, and
`runtime_bootstrap/` owns staging, per-object compilation, and archive
finalization. C++/ObjC++ remains responsible for ART/JNI/HWUI/Metal ABI
operations, but Rust owns the lifetime and the graph that decides when each
native artifact is rebuilt.

The production probe is split into independently cached phase objects:
production process configuration (`runtime/embedding/process_config.cc`),
fixture selection (`probes/runtime_fixture_options.cc`), production shutdown
coordination (`runtime/embedding/process_shutdown.cc`, with ART VM and
framework app owners), and fixture finalizers (`runtime_shutdown_probe.cc`),
network acceptance
(`runtime_acceptance_phases.cc`), and graphics presentation/JNI orchestration
(`runtime_graphics_phase.cc`) and pointer/frame input dispatch
(`runtime_graphics_input.cc`). RenderNode recording/orchestration remains in
`runtime_graphics_probe.cc`, while the direct HWUI/Ganesh Metal presenter is a
separate `runtime_graphics_gpu.cc` object. Changing Activity validation, widget
assertions, pointer handling, or Metal presentation therefore invalidates only
the affected phase and the final graphics link. None of these phases owns
process lifetime; they consume snapshots and call the single Rust-owned
lifecycle boundary.

The process ABI's additive `darwin_art_lifecycle_hooks_t` table connects that
boundary to the native entrypoint for production hosts. Direct legacy callers
may omit it and retain the compatibility phase state; the Rust host always
supplies it, so the native process-state object cannot override Rust's
bootstrapping/running/shutdown decisions.

### Virtual Android DSOs

Normal NDK libraries import Bionic and Android libraries rather than issuing a
kernel instruction themselves. The loader resolves those imports to facades:

```text
libapplication.so (Android ELF)
  ├─ libc.so          Bionic-compatible libc and syscall facade
  ├─ libm.so          Android math ABI
  ├─ libdl.so         custom ELF linker API
  ├─ liblog.so        Android logging API
  ├─ libandroid.so    lifecycle, asset, window, and input bridge
  ├─ libc++_shared.so Android C++ runtime as ELF
  └─ libEGL/GLES.so   later ANGLE/Metal graphics backend
```

The facades are analogous to Wine's `ntdll` and system DLLs. They preserve the
guest ABI and call Darwin mechanisms internally.

The first real `libc++_shared.so` import census prevents the libc facade from
becoming an accidental alias of Darwin libc. Its `libc.so` dependency contains
159 function imports and the Bionic `FILE` object `__sF`. The locked manifest
classifies them by implementation boundary:

| Class | Count | Ownership |
|---|---:|---|
| A — state-free ABI leaves | 11 | prefixed implementations with differential tests |
| B — translated wrappers | 76 | Android errno/constants/layout/path conversion |
| C — Bionic state | 65 | allocator, pthread/TLS, locale, stdio, environment |
| D — loader/kernel coupling | 8 | program headers, process/auxv and Linux semantics |

All eleven A functions now pass as a closed prefixed provider: byte memory and
string operations plus the required 32-bit wide-character leaves. In
particular Android ARM64 `wchar_t` ordering is unsigned, unlike Darwin's signed
type, so `wmemcmp` implements the Bionic ordering rather than forwarding to
the host.

The generated namespace owns all 160/160 imports with no duplicate owner:
A 11/11, B 76/76, C 65/65, and D 8/8. This includes allocator and
Bionic errno ownership, the immutable-root/private-`/data` filesystem and
virtual-FD slice, clocks and sleep, all 24 pthread imports, immutable
environment/property/auxv state, loader-owned program-header iteration,
provider-owned `FILE`, formatted and wide stdio, locale/multibyte and ICU wide
classification, narrow/wide/binary128 conversion, exact libc++ syscall and
ioctl dispatch, and Bionic-owned `__cxa_atexit`/`__cxa_finalize` state.
The provider namespace now composes all owners into one exact
SONAME/symbol/version router with shared quiescent teardown. ART's ELF graph
resolver uses that namespace and has executed real `__errno` and `strlen`
relocations. The final `sendfile` route copies between central virtual
descriptors without exposing a Darwin fd or calling Darwin `sendfile`; an
actual Android ELF graph has copied an immutable input into private writable
`/data` through that path. Unknown imports remain hard capability failures even
though this pinned libc++ census is complete.

## Prior art and the boundary we adopt

This design reuses lessons from existing foreign-ABI runtimes rather than
assuming that Linux-on-Darwin is unexplored:

| Project | Mechanism | What we adopt | What we do not adopt |
|---|---|---|---|
| [Noah](https://github.com/linux-noah/noah) | macOS Hypervisor.framework traps Linux syscalls, translates them to Darwin, and loads ELF | proof that a Darwin-hosted ELF/syscall personality is possible; a possible fallback execution domain for raw `svc` | translating the complete Linux userspace ABI as the Tier 1 fast path; the project is archived and reports broad incompatibility from missing syscalls |
| [gVisor](https://gvisor.dev/docs/architecture_guide/intro/) | a userspace application kernel implements Linux-visible processes, filesystems, signals, memory, and networking | guest-visible state must be owned explicitly; host calls are implementation details rather than direct syscall passthrough | its complete Linux process/kernel model and Linux-host interception mechanisms |
| [FreeBSD Linuxulator](https://docs.freebsd.org/en/articles/linux-emulation/) | the host kernel selects a Linux ABI personality with syscall, errno, signal, and structure translation tables | source-derived semantic references, constants, translation tables, and differential test ideas | its code or kernel-resident implementation; macOS exposes no supported `sysentvec`-like product API |
| [Blink](https://github.com/jart/blink) | CPU emulation plus a portable subset of the Linux syscall ABI | explicit compatibility manifests and precise unsupported results | CPU instruction emulation on same-ISA Apple Silicon |

Noah is the closest direct precedent, but its result also explains why the
mainstream path starts one level above raw syscalls. Android NDK libraries
normally reach the kernel through Bionic imports. Resolving those imports to
our virtual DSOs keeps ordinary ARM64 instructions native and limits semantic
translation to named ABI entrypoints.

Raw `svc #0` is a separate capability. A future Noah-like Hypervisor.framework
domain may trap it without running a full Linux kernel, but that execution
model cannot be silently mixed with host ART/JNI pointers. It requires an
explicit shared-memory/RPC ABI or moving the relevant runtime into the same
guest address space. Until that boundary is proven, raw-syscall libraries use
an explicit unsupported result. A VM fallback is the intended later backend,
not part of the current implementation plan.

## Syscall policy

The runtime does not try to translate every `svc` instruction dynamically.
The normal fast path intercepts calls at the Bionic API boundary:

```text
Android .so -> Bionic open() -> path resolver -> flag/stat conversion -> Darwin open()
```

Initial syscall families are grouped by semantics:

| Family | Darwin implementation strategy |
|---|---|
| file I/O | direct Darwin calls after namespace/constant conversion |
| virtual memory | owned interval mappings; rebuild guest flags, preserve W^X, and translate advice semantics before Darwin VM calls |
| sockets | Darwin sockets with Android structures, options, and errno conversion |
| pthread/TLS | Bionic memory layout backed by a Darwin parking-lot/side table |
| futex | user-space wait/wake parking lot; never reinterpret Bionic mutex memory |
| epoll/eventfd/timerfd | kqueue-backed Android objects with edge/oneshot fidelity |
| signals | explicit Android signal model and reserved runtime signals |
| properties/Binder/ashmem | runtime broker objects rather than host syscalls |
| `/proc`, `/sys`, devices, ioctl | synthetic or allow-listed providers |

An Android binary may contain a raw Linux sequence such as `svc #0`. Executing
it against XNU is unsafe because the syscall ABI and numbers are unrelated.
Install-time inspection scans executable code and dependency provenance. The
native backend rejects raw syscall binaries with a precise diagnostic. A later
backend may select a Linux VM. Rewriting raw syscall instructions is a later
experiment, not the compatibility foundation. Anti-cheat, DRM, self-modifying
code, and kernel-specific ioctl use remain VM-fallback candidates.

## Capability tiers

The presence of `.so` does not define a high tier. Most ordinary Android apps
include native analytics, database, compression, crypto, image, or vendor SDKs.
Tier 1 must support those libraries.

| Tier | Required capability |
|---|---|
| 0 — bootstrap | framework/ART bring-up probes; Java-only is acceptable only here |
| 1 — mainstream apps | DEX plus ordinary ARM64 JNI `.so`; ELF relocation, Bionic file/memory/thread/TLS/socket basics, libc++, no raw syscall requirement |
| 2 — native-heavy apps | broader pthread/futex/epoll, SQLite/crypto/media CPU paths, complex `dlopen`, multiple native SDKs |
| 3 — games | EGL/GLES/Vulkan translation, native audio, frame pacing, controllers, high-throughput shared surfaces |
| 4 — kernel-coupled | direct syscalls, anti-cheat, DRM, proprietary drivers and kernel ioctls; VM fallback or explicit unsupported result |

An APK classifier records ABI, `DT_NEEDED`, imports, TLS, relocations, executable
memory requirements, raw `svc` instructions, graphics APIs, and known restricted
SDKs. Backend selection is capability-based: a complete installed Darwin graph
is preferred, otherwise the original Android ELF graph uses the compatibility
loader when its required surface is implemented, and a Linux VM is the later
fallback for kernel-coupled binaries.

## Current state

Completed foundations include the Darwin ARM64 ART interpreter/runtime,
Android framework boot, real `Activity`/`PhoneWindow`/`DecorView`, Android 16
GraphicsJNI/HWUI/Skia software rendering, IOSurface/Metal presentation, ICU,
Minikin/HarfBuzz/FreeType text foundations, resource JNI, AndroidRuntime
ownership, all 568 Android-visible `OsConstants`, the complete 12-entry
`java.io.UnixFileSystem` owner, ART's complete `libopenjdkjvm` provider, and a
complete registered `libcore.io.Linux` method table with an incrementally
implemented Darwin backend. Complete Android 16 owners for `FileDescriptor`,
`FileInputStream`, the read-only NIO mapping path, and the ART/libcore split
`libcore.io.Memory` table are integrated as well. The NIO port restores the
process-global signal disposition during runtime shutdown.

The real `android.widget.Button` vertical slice now parses the pinned Android
font configuration and passes the complete Android 16 `FileInputStream`,
`IOUtil`, `FileChannelImpl`, `FileDispatcherImpl`, and `NativeThread` owners.
It also passes the complete scalar/bulk `libcore.io.Memory` owner, the full
`UnixNativeDispatcher` and libcore `System` owners, real Darwin `lseek`, and a
coherent ICU76 Java/native/data runtime. The result is a real Button frame and
clean ART shutdown. The Android ARM64 `ET_DYN` loader now maps directly into
the Darwin process, applies checked `R_AARCH64_RELATIVE`, `ABS64`, `GLOB_DAT`,
and `JUMP_SLOT` relocations, runs initializer arrays in order, and executes
imported data, function-pointer, and function references through a
caller-supplied closed resolver. Undefined weak symbols become zero while
unknown strong symbols, SONAME/version mismatches, lazy PLT, imported/static
TLS, W+X mappings, and unsupported relocations fail as capabilities rather
than falling through to dyld. One validated `PT_GNU_RELRO` range is relocated while writable and
sealed read-only before constructors or publication. The loader also accepts
the pinned libc++'s zero-valued AArch64 BTI dynamic tag while rejecting a
nonzero requirement or duplicate tag. The inspection classifier additionally
reports dependencies, versioned imports/exports, TLS, RELRO,
executable-stack/text-relocation requirements, and raw `svc`.

Recursive sibling ELF graphs now travel through ART's real native-library
ownership path. Its root eagerly resolves a child export, runs child-before-root
constructors, and publishes only after the full graph succeeds. ART then runs
Android `JNI_OnLoad` with a proxy JavaVM, accepts its exact
eight-entry `RegisterNatives` table, and installs only W-to-RX Darwin-entry
thunks—never raw Android function pointers. The regular-JNI shorty planner
supports Z/B/C/S/I/J/F/D/L/V scalar/reference arguments and returns, tracks GP
and FP register banks independently, and repacks Darwin's naturally aligned
stack tail into Android eight-byte slots. Actual ELF calls cover mixed FP,
narrow integer stack values, reference/FP/void returns, and post-load proxy
`GetVersion` plus `FindClass`. A separate generic registered native exercises
the proxy's modified-UTF-8, byte-array region, local/global reference, and
exception observation/clearing subset. Every forwarded call obtains the current
ART thread's JNIEnv instead of retaining the load-time pointer. DestroyJavaVM closes the
graph, runs root-before-child finalizers, and returns the executable-page live
count to zero. Per-image Bionic `__cxa_finalize(dso_handle)` is composed with
ELF `DT_FINI_ARRAY`/`DT_FINI`: each dependent drains its callbacks and ELF
finalizers before its mapping is released, then dependencies follow.
Hardened-runtime JIT
policy, CriticalNative, aggregate/HFA/varargs calls, broader JNI proxy tables,
and graph-wide/imported ELF TLS plus language-level TLS destructors remain
explicit gates. The
standalone loader runs `DT_FINI_ARRAY` in reverse order followed by `DT_FINI`,
exactly once after successful initialization, and a recursive graph finalizes
dependents before dependencies when its last owner closes.

The virtual DSO namespace is closed: unknown SONAMEs, symbols, and GNU versions
cannot fall back to Darwin globals. A standalone owner executes the first five
guest `libdl` APIs with same-handle refcounts, TLS `dlerror`, and constructor/
finalizer ordering, but production still lacks dynamic sibling insertion and a
lease that survives use of a `dlsym` result. All 18 API-35 `liblog` exports
resolve to and execute the pinned AOSP Darwin liblog implementation. Bionic
libc has an exact 160-import
`libc++_shared.so` census, 11 state-free leaf functions, four allocator
entrypoints with an explicit Android errno result seam, and four bit-exact
libm leaf functions. Bionic errno is pthread-local and host-isolated. The
filesystem facade owns byte paths, virtual descriptors, Android `stat` and
`dirent` layouts; the time facade owns explicit clock/sysconf mappings and
interruptible sleep. The pthread provider now owns all 24 libc++ imports and a
coherent create/join/detach token lifecycle without exposing Darwin
`pthread_t`. Process state comes from an immutable Android environment,
property, and auxv snapshot rather than host globals. The binary stdio slice
owns Android `FILE` tokens, permanent `__sF`, fixed-register binary I/O,
bounded formatted varargs, and wide I/O without exposing Darwin `FILE*`. The
locale provider adds 31 fixed-register functions without using
Darwin's process-global locale or reinterpreting its `wchar_t`; wide
classification and case conversion use pinned static Android ICU 76.1. The
integer parser owns the six `strto*` imports with AOSP base, prefix, overflow,
locale-ignore, and Bionic errno behavior. AOSP gdtoa and explicit AAPCS64 q0
entries cover binary32/binary64/binary128 conversion. Together the providers
cover all 160/160 libc imports with
duplicate ownership rejected by
`tools/audit-android35-libcxx-provider-coverage.sh`. The composed namespace is
linked into ART and resolves the fixture's `__errno` and `strlen` imports.
The filesystem owner is installed process-wide before constructors, combines
an immutable preopened root with a private in-memory `/data` overlay, and owns
synthetic random devices backed by Security.framework entropy. `sendfile` is
implemented over the same virtual descriptors, and the actual ART ELF probe
copies immutable input into the private writable `/data` overlay. Persistent
writable prefix storage, production composition of every provider into one
descriptor owner, general varargs formats, sockets/networking, and
kernel-coupled calls remain open. A standalone central descriptor broker now
models generation-tagged tokens over refcounted open-file descriptions: offset
and status flags are shared across dup references, `FD_CLOEXEC` is per
descriptor, owner close runs after the last descriptor and active lease, and a
bounded level-triggered epoll gate covers ADD/MOD/DEL socket readiness.
ABI v3 additionally leases 13 typed socket operations and atomically publishes
or rolls back an accepted child without exposing an owner object. Exact-target
`dup2`/`dup3`, SCM_RIGHTS, blocking/edge/oneshot epoll, and production adapter
composition remain fail-closed.

The Bionic DSO lifecycle provider owns destructor registration independently
of Darwin's C++ runtime. It preserves each function/argument/DSO triple,
drains per-DSO and process-global entries in LIFO order exactly once, and
supports reentrant registration while callbacks run. Its loader contract is
publish, finalize until quiescent, run the image's ELF finalizers, unpublish,
then unmap. ART composes that seam per image and preserves
dependent-before-dependency teardown without a process-global
`__cxa_finalize(NULL)` shortcut.

The hash-pinned real NDK libc++ ELF first passes a non-executing structural
acceptance: 4,267 supported RELA entries are applied, all 160 strong `@LIBC`
imports are observed, RELRO is sealed, and an export is found without running
initializers. ART then loads that same 9 MiB image in two closed sibling graphs.
One executes `std::string`, `vector`, allocation, and sorting and returns 189.
The other links the pinned Android `libunwind.a`, throws across a nontrivial
string-cleanup frame, catches the exception, and returns 73. Both unload
sequentially through the per-image PHDR and finalizer registries without Darwin
C++ runtime fallback.

A separate virtual-memory facade serves DSOs beyond the pinned libc++ import
set. Its first closed slice owns anonymous private whole mappings for
`mmap/mmap64`, `munmap`, `mprotect`, and `madvise`. It never forwards Linux flag
numbers to Darwin, rejects W+X, executes an AArch64 RW-to-RX transition with an
instruction-cache flush, and translates Linux `MADV_DONTNEED` to Darwin's
zero-fill advice. Fixed, shared, file-backed, partial-range, and ashmem mappings
remain capability failures until the namespace FD and interval owners exist.

## Ordered implementation after the Button gate

1. [Complete] Execute the real libc++ collections graph with its complete
   virtual `libc.so`/`libm.so`/`libdl.so` closure and the Android-libunwind
   throw/catch fixture without Darwin unwind fallback.
2. [Complete] Exercise libc++ `filesystem::copy_file` itself over the accepted
   `sendfile` route and private `/data` overlay, including source/destination
   size verification inside the actual ART-loaded graph.
3. [Complete for local `TLSDESC`] Allocate aligned guest TLS per Darwin thread,
   preserve the host thread pointer, and reject live-thread unload. Add
   graph-wide/imported TLS models and thread-local destructor integration.
4. Persist the private `/data` overlay into the case-sensitive product prefix;
   add contained Android symlinks and a unified open-file-description table.
5. Expand JNI proxy coverage and native-call generation beyond regular
   scalar/reference shorties. CriticalNative, aggregates/HFA, and unreviewed
   varargs remain fail-closed until separately accepted.
6. Implement socket, signal, futex/epoll, and networking semantics from real
   application manifests, using Bionic/Linux UAPI as authority and Linuxulator
   only as a source-derived differential reference.
7. Expand graphics/audio/input for games only after mainstream native SDKs pass
   the Tier-1 boundary.
8. Add the Linux VM backend and classifier handoff for raw-syscall and
   kernel-coupled binaries. This is deliberately later than the native path.

This order makes Tier 1 include realistic third-party native libraries before
the graphics stack expands to games.
