# AIM — Android Runtime for macOS

For normal development, start with the unified build interface:

```sh
cargo xtask status
cargo xtask check
cargo xtask build
```

Cargo owns the shared Rust workspace and Ninja owns the incremental AOSP native
graph. Package ownership and rebuild boundaries are documented in
[`docs/build-system.md`](docs/build-system.md). Individual provider manifests
and bootstrap subcommands are internal diagnostic interfaces.

Experimental Apple Silicon Android Runtime for macOS. ART remains C++/ARM64
assembly and is compiled as Mach-O. Rust owns the bootstrap tooling, Darwin
integration, launcher, and eventually the Android ELF compatibility loader.

The product boundary and post-Button roadmap are defined in
[`ARCHITECTURE.md`](ARCHITECTURE.md). The central rule is Android-observable
semantics over macOS mechanisms: a Wine-prefix-like Android filesystem,
directly mapped ARM64 ELF, and a Bionic ABI facade. Mainstream Tier 1 includes
ordinary third-party JNI `.so` libraries; Java-only execution is a bootstrap
gate, not the intended application compatibility boundary.

Shared per-profile state is owned by the Rust `darwin-artd` process. Rust
launchers carry versioned IPC leases through `exec(2)` for application and
service lifetimes while the daemon mounts their case-sensitive APFS Android
volume. The daemon also owns a persistent ART-backed `android.system` process;
cross-package metadata and component resolution use its Android Binder service
rather than per-app APK inspection or environment hardcoding. See
[`docs/profile-daemon.md`](docs/profile-daemon.md).

After one unified build, install APKs once and launch them by Android package
name without APK re-analysis or recompilation:

```sh
cargo xtask build
tools/darwin-art install path/to/app.apk
tools/darwin-art list
tools/darwin-art run com.example.app
tools/darwin-art ps
```

The AIM native macOS manager is the product owner, not a front end for a
source-tree CLI. Its AppKit bundle contains the runtime, compatibility libraries, graphics
backend, profile daemon, APK tools, framework resources, fonts, and ICU data.
It installs unchanged APKs, launches and gracefully terminates applications,
uninstalls code with an explicit keep/delete-data choice, opens private data,
and exposes profile/service management from one window. Profiles can be
created, switched, and safely deleted; installed apps use their Android label,
version, and APK icon. APKs can be selected in batches, dragged onto the
window, or opened from Finder:

```sh
tools/build-darwin-art-manager.sh
open "_build/Darwin ART Manager.app"
```

The build command above is a developer packaging step; the resulting `.app`
can be moved and launched without Cargo, the repository, or an external AIM
installation. `DARWIN_ART_PROFILE` remains an internal testing override;
normal launches use the manager-owned default profile. Managed application
output is appended to
`~/Library/Application Support/DarwinART/profiles/<profile>/managed-apps.log`
and processes are spawned by `darwin-artd`, so applications and
`android.system` remain independent of the manager window's lifetime. The
System sheet reports daemon/system-service state and allocated profile size,
and optionally registers the manager as a macOS login item.

Installed Android packages are also exposed as signed macOS application shims
under `~/Applications/Darwin ART Apps.localized`. Each `.app` carries the
Android label, version, and APK icon, so Finder, Spotlight, and Launchpad can
discover and launch it without opening the manager first. A shim contains only
the profile/package identity and a small native launcher; all applications
share the compatibility runtime embedded in the AIM manager. Non-default
profiles receive a profile suffix in the macOS app name. Refresh/install keeps
these bundles synchronized, uninstall removes stale package shims, and profile
deletion removes only bundles carrying AIM's matching ownership marker.

Physical-keyboard status and the remaining Android `.kl`/`.kcm` compatibility
work are documented in
[`docs/physical-keyboard-compatibility.md`](docs/physical-keyboard-compatibility.md).

## Current gate

The current gate is intentionally smaller than `dalvikvm`. It links and runs a
complete native `Runtime::Create()` probe through Android 16 boot-class-path
initialization, invokes a generated DEX `main(String[])` in ART's C++
interpreter, round-trips through JNI into Darwin, and sends Android
`System.out` through ICU and Darwin `write(2)`. A separate native-library gate
loads an Android ARM64 ELF `.so` through ART's real native-library path, runs
`JNI_OnLoad` against a proxy JavaVM, installs signature-specific JNI
trampolines, and invokes eight registered native methods. It also runs Android
16's real `Activity.attach()`, retains the
framework-created `PhoneWindow`, constructs its real `DecorView`, and executes
the platform and app `Activity.onCreate()` bodies using a minimal host context.
The app's real `Activity.setContentView(View)` call adds a normally constructed
View beneath that DecorView. The real-graphics vertical slice registers the
complete 51-class Android graphics native map and executes `DecorView ->
ViewGroup -> View.draw(android.graphics.RecordingCanvas) -> HWUI RenderNode ->
Ganesh Metal`. The resulting drawable is presented in an independent AppKit
`NSWindow`. The runtime is now a process-scoped C ABI dynamic library. A Rust
host owns its lifecycle and the persistent Metal/AppKit presentation surface:

1. verify the native host is ARM64 macOS with 16 KiB pages;
2. fetch revision-locked ART subtrees without Git metadata;
3. adapt ART's ELF-oriented assembly conventions to Mach-O;
4. call an ART-style assembly entrypoint from Rust and return through Rust;
5. compile ART's page-size-agnostic path and verify 16 KiB on Darwin;
6. build small Mach-O ARM64 `libartbase` and `libdexfile` archives;
7. build revision-locked AOSP Skia as a Mach-O ARM64 archive and verify real
   Ganesh Metal `SkSurface`, `SkCanvas`, `SkPaint`, and `SkPath` output;
8. compile Java to DEX and verify it with AOSP `DexFileVerifier`;
9. generate ART's real ARM64 ABI constants and compile context, optimized
   `__memcmp16`, plus quick/JNI/native entrypoint assembly;
10. compile the C++ switch interpreter and a 210-object Runtime/ClassLinker/GC/
   quick-entrypoint initialization spine as Mach-O archives;
11. link the real bootstrap probe with no unresolved native symbols;
12. create ART successfully with the pinned Android 16 core boot JARs;
13. load `Hello` from an app-only `PathClassLoader` and execute
    `Hello.answer()` through `ArtMethod::Invoke`;
14. register `Hello.hostPageSize()` through JNI and execute
    `Java -> JNI -> getpagesize() -> Java`, returning `42` from the wrapper;
15. register ART's complete runtime-native table plus the Darwin libcore subset
    needed to initialize `System`, then execute AOSP `System.arraycopy()`;
16. create ART's normal main-thread peer and invoke `public static void
    main(String[])` through JNI;
17. use Android's real `PrintStream -> CharsetICU -> StreamEncoder -> IoBridge`
    path, backed by host ICU4C and Darwin `write(2)`;
18. load Android 16's real framework DEX, prepare its main `Looper`, and
    instantiate an app DEX class that directly extends `android.app.Activity`;
19. call the real `Activity.attach()` with a Darwin-backed context, retain its
    default `PhoneWindow`, and normally construct the framework `DecorView`;
20. execute the real platform `Activity.onCreate()` body plus the app override,
    including `Activity.setContentView(new ProbeView(this))`;
21. run `DecorView.measure/layout/draw()` and the nested base
    `View.draw(RecordingCanvas)` traversal against Android's real HWUI display
    list, replay it into one persistent Ganesh Metal drawable, and present it
    through `CAMetalLayer` and `NSWindow`;
22. replay persistent RenderNode frames without CPU readback or full-frame
    IOSurface upload copies;
23. stress Darwin `LockSupport.park/unpark` permits, wakeups, and timeouts;
24. load an NDK-built Android ARM64 ELF through ART, run its `JNI_OnLoad`, and
    invoke scalar/reference JNI methods—including spilled narrow, integer,
    floating-point, and reference arguments—through generated
    Darwin-to-Android PCS thunks, then call supported proxy `JNIEnv` slots;
25. shut ART down through the C ABI after every successful runtime probe.

Run it with:

```bash
cargo run -p art-bootstrap -- all
```

Expected final lines include:

```text
probe-asm: ART Darwin ARM64 assembly result: 42
probe-pagesize: ART Darwin page size: 16384
build-foundation: libartbase Darwin: 1.500ms
build-skia: Skia Darwin raster: 64x64 rowBytes=256 hash=a4bb4cdb0b4779ea Skia Android framework utils: base-canvas=same surface=same reset-clip=64x64 ...
build-dex: AOSP DEX: verified=yes version=35 classes=12 methods=315 ... corrupt=rejected
build-runtime-platform: Mach-O arm64 objects=3 archive=...
build-runtime-core: pthread monitor bootstrap objects=2 archive=...
build-runtime-arm64: generated ABI constants, Mach-O objects=10 archive=...
build-interpreter-core: AOSP C++ interpreter Mach-O objects=7 archive=...
build-runtime-bootstrap: ART runtime initialization spine Mach-O objects=210 compiled=0 cached=210 archive=...
audit-runtime-link: C ABI dylib closure complete undefined=0 exports=11
probe-runtime-dex: Activity.attach() + PhoneWindow + DecorView.draw(Canvas) -> Darwin
probe-runtime-graphics: Android Canvas 640x360 pixels=230400 colors=6 opaque=230400 hash=44d122c296a3e065
probe-park: ART Darwin park: pre-permit=yes wakeups=200 timeout=yes
```

To keep the native window visible for three seconds, run:

```bash
cargo run -p art-bootstrap -- probe-window
```

A real, no-native APK with a binary manifest and a single app-only
`classes.dex` can launch its `MAIN/LAUNCHER` Activity into the Darwin window:

```bash
./tools/fixtures/simple-apk/build.sh
cargo run -q -p art-bootstrap -- probe-runtime-apk-app-window
# Or pass another APK in the current programmatic-UI subset:
./tools/run-android-apk-app.sh path/to/app.apk 30
```

The interactive runner uses a 720x1280 Android render target, 320 dpi framework
metrics, and a 2x Retina-backed 360x640 macOS window. Headless acceptance keeps
the deterministic 360x640 portrait surface. AppKit mouse down/up events are
translated into backing-pixel hit tests on the retained Android `DecorView`;
clickable framework views run their real `performClick()` state transition and
the updated tree is immediately rerendered through HWUI/Skia.

The pinned APK loads Android 16's real `framework-res.apk`, applies its built-in
Material Light theme with a restrained Material You light-scheme palette, and creates real `TextView`, `CheckBox`, `RadioButton`,
`ToggleButton`, `SeekBar`, `ProgressBar`, and `Button` objects (not custom
`View` stand-ins). The acceptance checks every runtime type plus an opaque,
visually diverse frame rendered through Android's resource, drawable,
SystemFonts/Minikin/HarfBuzz, and HWUI/Skia paths. The fixture contains no
layout resources or native `.so`; its only app-level visual customization is
the explicit Material You palette and a software-compatible Material ripple drawable.

The lower-level graphics probe remains deliberately small: `ProbeView.onDraw()`
records the Android view tree into a real `RecordingCanvas`/`RenderNode` display
list. The persistent display list is replayed by HWUI's Ganesh Metal path into
the `CAMetalLayer` drawable. The production graphics path has no Bitmap
readback, CPU framebuffer, IOSurface upload, or per-frame full-frame copy. The
opaque surface and Metal command queue remain owned by the graphics session;
Rust owns their lifetime while the C++ boundary only performs ART/HWUI/Metal
operations.

This proves the real `Activity.onCreate() -> PhoneWindow.setContentView() ->
DecorView -> ViewGroup.dispatchDraw() -> View.draw(RecordingCanvas) ->
HWUI/Skia -> Ganesh Metal drawable` path. The APK acceptance requires all
230,400 pixels to be opaque, at least eight distinct rendered colors, all seven
framework widget types, and clean ART shutdown. Choreographer-driven
animations, `Switch`, and editable text remain explicit next boundaries rather
than fake-success paths. The DecorView also still uses a programmatic
`android.R.id.content` root plus a minimal `Resources` object. The runtime now
uses the full Android 16 resource JNI,
Android-visible OS constants, UnixFileSystem, and ART OpenJDK VM service
owners. The Button probe now passes the complete Android 16 `FileInputStream`,
`FileChannelImpl`, `FileDispatcherImpl`, `IOUtil`, and `NativeThread` owners,
the complementary ART/libcore `libcore.io.Memory` owners, the 47-entry
`UnixNativeDispatcher`, the complete libcore half of `java.lang.System`, and
Darwin `lseek`. Production OpenJDK NIO routes Android mounts and relative app
paths through the Rust-owned Bionic filesystem facade, while exact immutable
runtime files passed as launch capabilities remain host libc paths. This
preserves one guest descriptor/path namespace and keeps app bundles relocatable
without hiding ART's host-side runtime inputs. With a source-coherent
ICU76 Java/native/data set it maps the
pinned Android font data, constructs a real `android.widget.Button`, renders
through HWUI/Minikin/Skia, exports a frame, and shuts ART down cleanly.
Compiled app-resource inflation and direct GPU HWUI are live. Full Android
`MotionEvent`/gesture dispatch remains a separate boundary; the current input
adapter intentionally supports bounded clickable-view pointer semantics only.

The first Tier-1 native-library admission gate is also available:

```bash
cargo run --manifest-path tools/android-arm64-so-inspect/Cargo.toml -- path/to/libfoo.so
```

It does not map or execute untrusted code. It strictly parses Android ARM64
ELF metadata and emits a capability report covering dependencies, symbols,
TLS, RELRO, relocations, constructors, executable-stack/text-relocation
requirements, and raw `svc` instructions. Linuxulator-derived manifests are
used only as an independent semantic oracle; no FreeBSD kernel code is linked
into the Darwin runtime.

The first executable JNI slice runs with:

```bash
cargo run -p art-bootstrap -- probe-runtime-elf-jni
```

This is a real ART `JavaVMExt::LoadNativeLibrary` path, not a standalone loader
smoke. A root, child, and grandchild ELF form a recursively discovered closed
`DT_NEEDED` graph: dependency-before-dependent constructors run, the root calls
a child export through an eager relocation, and only the complete graph is
published. The fixture then reaches
`JNI_OnLoad`, proxy `FindClass` and
`RegisterNatives`, then executes eight scalar/reference methods through
write-then-execute-protected ARM64 thunks. The planner derives JNI shorties from
descriptors, tracks GP and FP banks independently, and repacks Darwin's natural
stack tail into Android's eight-byte slots. Native methods invoked after
`JNI_OnLoad` use the current ART thread's `JNIEnv`, not a retained load-time
pointer, to call proxy `GetVersion` and `FindClass`, round-trip modified UTF-8
and a byte array, transition local/global/local references, and raise, observe,
then clear an array-bounds exception. Before constructors, ART
installs a process-wide Bionic filesystem owner with an immutable preopened
root, private in-memory `/data`, and synthetic random devices. The same graph
resolves imports through a sealed 29-provider namespace that owns
all 160/160 pinned libc++ libc-family imports. The final `sendfile` route is
exercised by an Android ELF copy into the private writable `/data` overlay.
Unload composes each image's Bionic
`__cxa_finalize(dso_handle)` callbacks with its ELF finalizers, then tears down
the namespace after all in-flight leases drain. A pre-publish failure injection
verifies the same cleanup. The same acceptance builds an APK, validates its
central/local ZIP records and CRCs, extracts six `arm64-v8a` DSOs into an
atomically published read-only directory, and executes both graphs directly
from that sealed directory through `JavaVMExt`/NativeBridge.
The complementary `extractNativeLibs=false` acceptance keeps a
root/child/grandchild graph in a read-only APK as 16 KiB-aligned STORED entries,
maps those entry ranges directly, and reaches `JNI_OnLoad` and shutdown without
creating extracted copies.
Aggregates, HFA,
general variadic calls,
CriticalNative, the rest of the JNI table, hardened-runtime `MAP_JIT`,
graph-wide/imported ELF TLS, C++ thread-local destructors, and central network
FD integration remain explicit work. A standalone Bionic socket facade already
translates 20 TCP/UDP/poll/message APIs with virtual descriptors, but it is not
yet installed in the production namespace. The central descriptor broker now
passes generation-safe OFD sharing, dup allocation, per-descriptor CLOEXEC,
last-close, bounded epoll ADD/MOD/DEL, and 13 typed leased socket operations
including atomic accept publication/rollback. Exact-target `dup3` and production
provider composition remain open. A bounded local-definition AArch64
`TLSDESC` path is implemented:
it allocates aligned guest blocks per Darwin thread, preserves `TPIDR_EL0`, and
fails stop rather than unmapping an image used by another live thread.

The pinned real NDK libc++ also passes both structural and executing gates. All
4,267 supported relocations are applied through a closed resolver, GNU RELRO is
sealed, its zero-valued AArch64 BTI tag is recognized, and all 160 strong
`@LIBC` imports are observed. ART then executes a collections consumer
(`std::string`/`vector`/sort/allocation = 189) and an exception consumer linked
with pinned Android `libunwind.a` (cross-frame cleanup/catch = 73), followed by
sequential graph unload. The collections path also runs libc++
`filesystem::copy_file` from the immutable guest root into private `/data` and
checks both file sizes. The runtime publishes concurrency-stable copied PHDR
snapshots so `dl_iterate_phdr` never falls through to dyld.

Its final line is:

```text
probe-runtime-elf-jni: ART DT_NEEDED graph + JNI + reverse finalizers PASS
```

The first Android 16 HWUI host compile gate is reproducible with:

```bash
bash tools/compile-android16-hwui-canvas-gate.sh
```

It compiles the unmodified upstream `SkiaCanvas.cpp`, `hwui/Canvas.cpp`, Canvas
JNI, and Paint JNI sources as ARM64 Mach-O objects and records their direct
undefined-symbol closure. The exact source and dependency revisions are pinned
under `upstream/`; the ordered module-level closure and current
registrar closure are documented in
`upstream/android16-hwui-host-closure.md`.

Skia itself now builds with `SK_BUILD_FOR_ANDROID_FRAMEWORK`; an executable gate
calls the real `SkAndroidFrameworkUtils` surface, wrapped-canvas, and clip-reset
operations while preserving the existing raster and direct-IOSurface golden
hashes. A narrow locked patch now keeps Darwin's host platform branches while
selecting Android's `@CriticalNative` function signatures and Skia's no-RTTI
contract; representative Canvas/Paint symbols are checked to contain no
`JNIEnv*` or `jclass` parameters. The original four-object gate is now followed
by module-complete host builds: all 60 common GraphicsJNI translation units plus
the Darwin host TU, the 51-entry Layoutlib registrar in `jni_runtime.cpp`
dependency order, all 81 Darwin `libhwui_static` core/host members, the five
APEX-common graphics TUs, and the 36-member Darwin `libandroidfw` composition.
These real upstream module archives now form the strict force-loaded graphics
closure used by `probe-runtime-graphics`. The audit rejects unresolved provider
symbols, fake Paint/RenderNode implementations, host ICU/fmt, and CoreText.

The first platform foundation modules are also real Android.bp-derived ARM64
archives rather than symbol stubs:

```bash
cargo run -p art-bootstrap -- build-hwui-canvas
cargo run -p art-bootstrap -- build-android-graphics-jni
cargo run -p art-bootstrap -- build-hwui-static
cargo run -p art-bootstrap -- build-androidfw
cargo run -p art-bootstrap -- build-hostgraphics
cargo run -p art-bootstrap -- build-skia-hwui
cargo run -p art-bootstrap -- build-graphics-codec-modules
cargo run -p art-bootstrap -- build-ziparchive-incfs
cargo run -p art-bootstrap -- build-graphics-foundations
cargo run -p art-bootstrap -- build-nativehelper
cargo run -p art-bootstrap -- build-ui-types
cargo run -p art-bootstrap -- build-graphics-codecs
cargo run -p art-bootstrap -- build-harfbuzz
cargo run -p art-bootstrap -- build-minikin
cargo run -p art-bootstrap -- build-skia-text
cargo run -p art-bootstrap -- build-icu
cargo run -p art-bootstrap -- probe-minikin-shaping
cargo run -p art-bootstrap -- build-runtime-graphics-bootstrap
cargo run -p art-bootstrap -- audit-runtime-graphics-link
cargo run -p art-bootstrap -- probe-runtime-graphics
cargo run -p art-bootstrap -- probe-runtime-graphics-window
bash tools/verify-real-graphics-rust-host.sh \
  _build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib
```

For the native inner loop, use the fast validator after a targeted TU or
artifact build:

```sh
cargo run -p art-bootstrap -- audit-runtime-graphics-link-fast
```

It consumes the existing graphics closure, performs the runtime link and
symbol checks, and skips the upstream registrar build gates. The full command
above remains the release/CI acceptance gate.

`liblog-darwin.a` contains all eight Darwin-host sources and resolves the two
log functions in the current HWUI closure. `libcutils-darwin.a` contains its
base, host, non-Windows, and whole-static socket source groups (19 objects).
The nativehelper gate builds the four-object `libnativehelper_any_vm` archive
and the seven-object `libnativehelper_jvm` whole-static equivalent, including
the real JNI registration, exception, NIO, and file-descriptor helpers. The
nativehelper archive must use normal archive extraction in the final link;
force-loading it would also pull `JniInvocation`'s `JNI_CreateJavaVM` alongside
ART's VM implementation.
`libutils-darwin.a` follows its Darwin core source selection and whole-static
Binder support archive (19 objects total). `libui-types.a` adds the complete
four-source host module for Android `ColorSpace`, `Rect`, `Region`, and
`Transform` types.
The graphics codec gate builds Android's complete Darwin ARM64 selections for
zlib (19 sources), libpng (18 sources), and FreeType (26 sources). These are
the pinned raster-font dependencies needed before enabling Skia's FreeType
font manager; they do not use Homebrew libraries or replacement symbols.
The next two gates compile all 53 Android HarfBuzz translation units and all
31 Minikin translation units. Their remaining unresolved closure is kept
visible; the next required module is the pinned Android ICU host build and its
data image, not a local symbol shim.
`build-skia-text` keeps the no-font baseline untouched and produces a separate
Skia archive wired to the pinned AOSP FreeType/libpng/zlib archives. Its actual
Roboto `Click` raster is locked at five glyphs, 1,097 ink pixels, and pixel hash
`1f94df6816828ca2`; the final binary has no Homebrew or CoreText dependency.
The ICU gate sparsely materializes only the Android common/i18n/data/init
subtrees (about 49 MiB instead of the roughly 425 MiB full project), builds all
458 translation units, and stages ICU 76 data. The Minikin acceptance then
shapes Latin, Arabic bidi, Hangul, and a one-glyph `fi` GSUB ligature through
the actual FreeType → Minikin → HarfBuzz → ICU closure.

The GraphicsJNI and HWUI gates also use history-free sparse inputs. GraphicsJNI
adds about 1.9 MiB of revision-locked JPEG, UltraHDR, GUI, media-NDK, and
libhardware headers; HWUI adds about 2.2 MiB including the exact upstream
`sysprop_cpp` generator. The generated 51-class property is derived by filtering
Layoutlib's supported map through Android's authoritative `jni_runtime.cpp`
registration order, so an empty or dependency-misordered registrar cannot pass.
The runtime's atomic real-graphics mode removes the fake Paint/RenderNode
registrations and uses a real `RecordingCanvas`, persistent RenderNode, and
direct Ganesh Metal drawable replay. It links and passes end to end while the
headless ProbeCanvas path remains as a regression baseline.
The bootstrap also compiles Android 16 libcore's revision-locked `Math.c`
unchanged, including its complete 23-entry FastNative table, instead of
reimplementing those methods as Darwin wrappers.

The next provider layer is also module-complete. `libhostgraphics` supplies the
five Android host native-window/display sources. The HWUI-specific framework
Skia archive contains 523 members, consumes the pinned AOSP FreeType/libpng/zlib
archives, includes the empty font-manager and sharing serialization members
required by whole-static HWUI, and has no CoreText or Homebrew import. The codec
gate builds six image_io/JPEG/UltraHDR archives (116 members) and executes a
real ARM64 JPEG encode/decode plus UltraHDR scan. A separate six-member
`libziparchive_for_incfs` gate preserves Android.bp's callback-disabled variant
and verifies a ZIP writer/reader round trip; it is not conflated with the older
INCFS-disabled bootstrap archive.

`build-runtime-bootstrap` keeps a dependency-aware object cache. Clang emits a
depfile for every translation unit; the bootstrapper fingerprints the complete
compile command, compiler/macOS identity, and SHA-256 of every referenced source
and project header. File hashes are memoized using Darwin inode and timestamp
metadata, so an unchanged warm build avoids rereading common AOSP headers. A
source-only edit rebuilds only the objects whose depfiles contain that source.
All cache files remain under ignored `_build/` paths.

`build-dex` requires Homebrew OpenJDK 17 headers and an Android SDK build-tools
installation containing `d8`. It generates the test DEX locally; no APK or
prebuilt DEX is stored in the repository.

The runtime bootstrap uses Android's pinned JNI header rather than the host JDK
ABI. ART's existing Darwin pthread fallback is retained. The monitor patch only
supports uncontended inflation by the current thread; multithreaded execution is
blocked until empty-checkpoint wakeups and cross-thread inflation are implemented.
`LockSupport.park/unpark` now uses ART's pthread-backed mutex/condition variable
fallback on Darwin and has an executable stress probe. It shares each ART
thread's existing wait mutex and condition variable rather than adding another
pair to `Thread`; this also keeps the upstream `Thread` object layout consistent
across all runtime archives.

The three production ARM64 entrypoint assembly files now compile as Mach-O and
remove the `_art_quick_*`/`_art_jni_*` family from the Runtime link closure.
For this PoC their DWARF CFI directives are emitted into clearly named
`darwin-no-cfi` generated sources because Apple's assembler rejects parts of
ART's ELF-oriented CFI state machine. Correct Darwin unwind metadata is required
before exceptions and stack walking through these stubs can be considered safe.

`audit-runtime-link` builds `libdarwin_art_runtime.dylib`, restricts it to the
nine versioned runtime/surface C ABI exports, and builds the Rust host. The
current native closure is complete; the command fails if any undefined symbol,
unexpected export, or quick/JNI/context assembly regression appears. The host
copies the borrowed frame only after validating dimensions and stride, owns all
AppKit presentation, destroys its surface, and then requests ART shutdown on
the same native thread. `DestroyJavaVM` performs the ART thread-list teardown;
registered application DexFiles remain owned until that teardown is complete.

The linked runtime can be executed through its Rust host directly:

```bash
target/debug/darwin-art-host \
  _build/runtime-link-probe/libdarwin_art_runtime.dylib \
  _prebuilt/android-16/bootclasspath/core-oj.jar \
  _prebuilt/android-16/bootclasspath/core-libart.jar \
  _prebuilt/android-16/bootclasspath/framework.jar \
  _build/bootclasspath/core-icu4j.jar \
  _build/dex-probe/dex/classes.dex
```

Its success lines are `ART Darwin Runtime::Create: ok`,
`ART Darwin app ClassLoader: PathClassLoader`, and
`ART Darwin DEX interpreter: Hello.answer()=42`, followed by
`ART Darwin JNI: hostPageSize()=16384 nativeRoundTrip()=42`, and
`ART runtime native: System.arraycopy()=42`, the Java-emitted
`Hello from AIM main: 안녕`,
`ART Android framework: ProbeActivity().probeValue()=42`,
`ART Android window: Activity.attach()=PhoneWindow+DecorView`,
`ART Android view: Activity.setContentView()->DecorView.draw(Canvas)=640x360`,
`ART Android lifecycle: Activity.onCreate()=43`, and
`ART Darwin launcher: main(String[])=ok`.
The generated app DEX remains separate from the boot class path. The native
probe initializes ART's unstarted-runtime
handlers, constructs a `PathClassLoader`, registers the DEX, then enables a
probe-only minimal-start gate. This gate registers ART's complete runtime-native
table, initializes intrinsics, creates the normal main `Thread` peer, and runs
root class initializers. The real-graphics flavor now owns the complete Android
16 `libcore.io.Linux`, `OsConstants`, `UnixFileSystem`, ICU JNI, resource JNI,
and ART `libopenjdkjvm` registrations needed by this path. The installed
Android `core-icu4j` artifact is converted to DEX locally and calls the pinned,
statically linked AOSP ICU 76 implementation; no host ICU dylib is used. The framework gate
uses Darwin-native `MessageQueue` wake/poll primitives and Android's default
INFO logging threshold. Darwin clocks back `System`/`SystemClock`, and an
in-process property table backs Android `SystemProperties`; its Darwin defaults
now include the ARM64 ABI properties required by `android.os.Build`. A minimal
Binder holder/finalizer supports local framework Binder stubs, and the host ICU
bridge now covers both encoder and decoder paths. The app-only compile stub for
hidden `IContentProvider` supplies a javac signature and is not emitted into the
DEX; runtime resolution uses framework.jar's real interface. Android's
libjavacore/libopenjdk shared libraries and daemon threads are still not loaded
as Android ELF DSOs. The current real `Activity.attach()` uses upstream
AssetManager/ApkAssets native owners but still constructs a programmatic probe
`Resources`, a null `Instrumentation`, and no remote services. It constructs and
retains the framework `PhoneWindow`; the launcher normally constructs its
`DecorView` and installs a programmatic content root because complete app
resource-table/layout inflation is not ported yet. The content `View` is
normally constructed and traversed through Android Skia/HWUI CPU raster. Full
decor resources, Android `MotionEvent` gesture dispatch, GPU HWUI, and
incremental frame scheduling remain deferred; the host currently implements
bounded clickable-view down/up input with redraw.

Apple ARM64 executables retain the kernel-required 4 GiB `__PAGEZERO`, so ART's
usual absolute-low-32-bit heap references cannot be used. The Darwin probe
uses a separate 4 GiB representable window beginning at 1 TiB and stores
managed references as 32-bit offsets from that base. The 1 TiB base avoids
Darwin's randomized malloc zones in the first few dozen GiB. Only a bounded
256 MiB contiguous arena inside that window is reserved for the current
64 MiB-Xmx CMS configuration; the complete 4 GiB window is never reserved.
Mach virtual reservation does not make all 256 MiB resident: physical pages are
committed as ART touches its heap spaces. Every object that inlines the
reference ABI must be compiled through the same Darwin overlay. Launchers call
`MemMap::Init()` before their first heap allocation so the arena cannot be
claimed during runtime-option construction.

The Darwin MVP defaults to concurrent mark sweep. Concurrent mark compact stays
compiled in stop-the-world fallback form because Darwin has neither Linux
`userfaultfd` nor `mremap`; it is not selected as the default collector.

Matching Android 16 `core-oj.jar`, `core-libart.jar`, and `framework.jar` remain
ignored local inputs under `_prebuilt/android-16/bootclasspath`. `sync` downloads only the
revision-locked 2.99 MB `core-icu4j` class JAR and `d8` produces its ignored DEX
JAR locally. Verify all four inputs, including all five framework DEX files,
with:

```bash
cargo run -p art-bootstrap -- verify-bootclasspath
```

The runtime compiler enables AOSP's `USE_D8_DESUGAR` configuration so native
mirror class sizing matches these D8-built boot JARs. This is a Soong build
configuration dependency, not a Darwin-specific `java.lang.String` layout.

Generated content is kept out of version control:

```text
_downloads/  locked compressed source archives
_aosp/       extracted source, no .git directory
_build/      generated assembly and probe binaries
_prebuilt/   local Android boot class path inputs
target/      Rust build output
```

The source lock currently pins the peeled `android-16.0.0_r1` ART commit.
Gitiles archives are extracted into staging and the actual upstream source is
SHA-256 checked before it is moved into `_aosp`. Existing source trees are
never automatically deleted or replaced.

## Language boundary

- C++/assembly: ART runtime, GC, interpreter, JIT, quick/JNI entrypoints.
- Rust: source materializer, patch validation, process launcher, Darwin shims,
  APK orchestration, IPC, and the future ELF/Bionic compatibility layer.
- Swift/Objective-C: only where AppKit or platform APIs are substantially easier
  to expose than through Rust FFI.

Open work, bugs and missing platform contracts are tracked as
[GitHub issues](https://github.com/hahnlee/aim/issues); see
[CONTRIBUTING.md](../CONTRIBUTING.md#tracking-work).
