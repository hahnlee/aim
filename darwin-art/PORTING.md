# Porting gates

## G0 — ARM64 Darwin ABI

- [x] Pin ART without storing Git object history.
- [x] Detect the real 16 KiB Darwin page size.
- [x] Validate Mach-O symbol naming and CFI with an ART-derived entrypoint.
- [x] Convert the upstream ARM64 assembly macros behind `__APPLE__` guards.
- [x] Compile quick/JNI/native ARM64 entrypoints with generated ART constants.
- [ ] Port ART's full DWARF CFI state machine to Apple's assembler; the PoC
      entrypoint artifacts currently omit CFI.

## G1 — ART foundations

- [x] Materialize only `libartbase`, `libdexfile`, and their current dependency closure.
- [x] Make `GetPageSizeSlow()` use `sysconf(_SC_PAGE_SIZE)` on Darwin.
- [x] Build the first `libartbase` Mach-O ARM64 foundation archive.
- [x] Parse a generated `classes.dex` through AOSP `StandardDexFile` on Darwin.
- [x] Verify generated and Android 16 boot-class-path DEX with AOSP `DexFileVerifier`.
- [x] Preserve macOS's mandatory 4 GiB `__PAGEZERO` and move ART's compressed
      managed-reference heap into a base-relative 4 GiB virtual window.
- [x] Reserve only a 256 MiB managed arena for the 64 MiB-Xmx bootstrap and
      activate its anonymous subranges lazily without committing the full
      representable window.

## G2 — Interpreter-only runtime

- [x] Compile ART host platform hooks (`runtime/thread/monitor`) as Mach-O ARM64.
- [x] Compile patched `Mutex` and `Monitor` core implementations on Darwin.
- [ ] Split explicit `*_darwin.cc` files when behavior diverges from host Linux hooks.
- [x] Generate upstream ARM64 ABI constants and compile `Arm64Context`, thread,
      and entrypoint initialization as Mach-O.
- [x] Compile ART's C++ switch interpreter as a Mach-O archive.
- [x] Compile `Runtime::Create`, `ClassLinker`, `Thread`, `ThreadList`, `Heap`,
      instrumentation, callbacks, intern table, OAT manager, and `JavaVMExt`.
- [x] Compile GC accounting, collectors, spaces, and quick/JNI C++ entrypoints;
      use CMS by default and keep CMC in non-userfaultfd fallback form.
- [x] Add a real `Runtime::Create()` link-closure audit; all native symbols,
      including quick/JNI/context assembly families, are resolved.
- [x] Implement Darwin ARM64 `ucontext_t` register access for crash diagnostics.
- [ ] Complete signal-driven implicit-check handling; the first `-Xint` gate
      can keep implicit checks disabled.
- [x] Compile ART's existing pthread mutex/condition-variable fallback on Darwin.
- [ ] Implement pthread wakeups for ART empty-checkpoint requests.
- [x] Replace the non-futex `LockSupport.park/unpark` no-op with a tested
      pthread mutex/condition-variable implementation on Darwin.
- [ ] Support cross-thread monitor inflation; current bootstrap enforces current-owner only.
- [x] Enter `Runtime::Create()` with `-Xint`, no zygote, JIT, AOT, or Nterp.
- [x] Complete boot-class initialization and return successfully from
      `Runtime::Create()` with the pinned Android 16 core JARs.
- [x] Execute `Hello.answer()` from DEX through the C++ switch interpreter.
- [x] Keep application DEX off the boot class path and load it through an ART
      `PathClassLoader` created by the native bootstrap probe.
- [x] Execute interpreted Java through ART's generic JNI trampoline into a
      registered Darwin function and return the result to Java.
- [x] Compile and register ART's complete runtime-native source table.
- [x] Run early root class initialization and initialize ART intrinsics in the
      Darwin minimal-start path.
- [x] Initialize ART's main `ThreadGroup`/`Thread` peer without starting daemon
      services, enabling `ThreadLocal` and Android `BlockGuard` paths.

## G3 — Android-facing runtime

- [x] Extract and verify matching Android 16 core-oj/core-libart DEX inputs.
- [x] Fetch the locked Android 16 `core-icu4j` class JAR without Git history,
      convert it to DEX locally, and verify it with AOSP `DexFileVerifier`.
- [x] Load the verified core-oj/core-libart/core-icu4j DEX files into
      `ClassLinker`.
- [x] Load a generated app DEX into its own `PathClassLoader` and execute it.
- [x] Add the first explicit Java/JNI native method (`hostPageSize`).
- [x] Initialize `java.lang.System` with a Darwin subset of libcore natives for
      primitive bit conversion, `sysconf`, user/environment/`uname`, ICU
      metadata, special properties, and standard file descriptors.
- [x] Execute AOSP's native `System.arraycopy()` from interpreted app DEX.
- [x] Invoke an app's conventional `public static void main(String[])` via JNI.
- [x] Route Android `System.out.println` through `PrintStream`, Android
      `CharsetICU`, host ICU4C conversion, `IoBridge`, and Darwin `write(2)`.
- [x] Verify Android 16's five framework DEX files, initialize the real main
      `Looper`, and instantiate an app class extending `android.app.Activity`.
- [x] Back the first Android framework natives (`MessageQueue` and
      `Log.isLoggable`) with Darwin behavior.
- [x] Back Android framework clocks and the initial `SystemProperties` surface
      with Darwin monotonic/continuous/thread clocks and an in-process table.
- [x] Attach a minimal base `Context`, `Application`, and `ActivityInfo` through
      Android 16's real `Activity.attach()`, retain its `PhoneWindow`, normally
      construct its `DecorView`, and execute the real `Activity.onCreate()`.
- [ ] Register the minimal libcore native method set needed by ordinary Java
      startup without loading Android `.so` libraries.
- [ ] Replace the null `Instrumentation` and synthetic resources/settings with
      the complete application/runtime service path.
- [x] Execute the app's real `Activity.setContentView(new ProbeView(this))`,
      traverse the actual `PhoneWindow -> DecorView -> ViewGroup` hierarchy,
      run the base `View.draw(Canvas)` plus `onDraw()`, and display it in an
      AppKit `NSWindow`.
- [x] Materialize revision-locked Android 16 AOSP Skia without Git history,
      build its upstream CPU raster core for macOS ARM64 with GPU/PDF/external
      codecs disabled, and pixel-verify real `SkSurface`, `SkCanvas`, `SkPaint`,
      and `SkPath` execution.
- [x] Replace the per-frame Core Graphics snapshot viewer with a persistent
      `NSWindow`/`CAMetalLayer`/IOSurface/Metal surface and explicit event pump.
- [x] Map the IOSurface producer memory, draw 120 frames into it with upstream
      Skia, and Metal-present them with zero staging copies.
- [x] Export ART as a one-shot process C ABI, launch it from a Rust host, and
      shut it down after the native surface is released while retaining
      registered application DexFiles through runtime teardown.
- [x] Pin the Android 16 HWUI Canvas source identity and compile the first
      upstream `SkiaCanvas`, `Canvas`, Canvas JNI, and Paint JNI objects as
      unmodified arm64 Mach-O translation units.
- [x] Rebuild Skia with `SK_BUILD_FOR_ANDROID_FRAMEWORK`, include and execute
      `SkAndroidFrameworkUtils`, and keep the raster/IOSurface golden hashes
      stable under the Android-private Canvas ABI.
- [x] Build the first HWUI objects for the Darwin host platform while selecting
      Android's `@CriticalNative` calling convention through a narrow locked
      patch, without defining `__ANDROID__`, and match Skia's no-RTTI contract.
- [x] Build Android.bp-complete Darwin host archives for `liblog`, `libcutils`,
      `libnativehelper_any_vm`, and `libnativehelper_jvm`; verify ARM64 Mach-O
      members and the real JNI/log definitions required by the graphics closure.
- [x] Build Android.bp-complete Darwin host archives for `libutils` (including
      its whole-static Binder support module) and `libui-types`.
- [x] Build Android.bp-complete Darwin ARM64 archives for zlib, libpng, and
      FreeType as the native glyph-raster dependency chain.
- [x] Build Android.bp-complete Darwin ARM64 archives for HarfBuzz (53 objects)
      and Minikin (31 objects), retaining the real ICU dependency closure.
- [x] Build a separate Android-framework Skia FreeType configuration and
      raster pinned Roboto glyphs through the AOSP FreeType/libpng/zlib chain.
- [x] Build Android ICU common/i18n/stubdata/init for Darwin and exercise the
      complete Minikin/HarfBuzz/FreeType/ICU shaping path with pinned fonts.
- [x] Build all 60 common GraphicsJNI sources plus the Darwin host source,
      generate the 51-class Layoutlib property in `jni_runtime.cpp` dependency
      order, and verify every registrar target under Android CriticalNative ABI.
- [x] Build the complete Darwin `libhwui_static` core/host selection (81
      members including upstream-generated HWUI properties) and the separate
      five-member APEX-common archive without duplicating GraphicsJNI.
- [x] Build the complete Darwin host `libandroidfw` composition: 34 common
      sources plus whole-static PathUtils and incfs map support.
- [x] Build the complete five-source Darwin `libhostgraphics` module and a
      523-member HWUI framework Skia archive with the required sharing/font
      members and no CoreText or Homebrew dependency.
- [x] Build module-complete image_io/JPEG/UltraHDR archives (116 members total)
      and execute a real JPEG encode/decode plus UltraHDR scanner smoke.
- [x] Build Android.bp's six-member `libziparchive_for_incfs` variant separately
      from the older INCFS-disabled ART bootstrap archive.
- [x] Close the complete upstream Canvas/Paint registrar dependency graph with
      GraphicsJNI, software `libhwui_static`, Minikin, HarfBuzz, FreeType, ICU,
      androidfw, and native utility modules; do not substitute per-symbol stubs.
- [x] Make the runtime's real-graphics mode atomically exclude Darwin
      Paint/RenderNode handles, configure the source-derived registrar only
      after minimal System initialization, and prepare the Bitmap-backed Canvas
      render path without changing the default probe backend.
- [x] Register the complete graphics native map atomically after libcore/System
      initialization, then replace `ProbeCanvas` with a real `Bitmap`-backed
      `Canvas` before attempting direct IOSurface backing.
- [x] Port Generic JNI and CriticalNative frame sizing to Darwin ARM64's native
      stack PCS (`Z/B=1`, `C/S=2`, `I/F=4`, `J/D/reference=8`), verify mixed
      register spills, and run upstream `Bitmap.getPixels()` without a graphics
      workaround.
- [x] Replace the temporary Darwin `java.lang.Math` wrappers with Android 16
      libcore's unchanged `Math.c` and complete 23-entry FastNative table.
- [x] Replace the Java `int[]` test scene with ordinary Android `Paint` and
      upstream `Canvas.drawColor()`/`drawRect()` raster while preserving the
      exact six-color frame hash.
- [ ] Replace the programmatic Paint scene and Bitmap readback with ordinary
      Android text widgets/Button and direct IOSurface backing; replace the
      programmatic content root/minimal interpolator parser with complete
      compiled framework-resource support.

## G4 — Android namespace and mainstream native libraries

- [x] Replace partial Android constant/filesystem owners with complete,
      source-derived Android 16 registrations while keeping Android/Linux
      numeric values visible above the Darwin syscall boundary.
- [x] Render a real `android.widget.Button` using the pinned Android font
      configuration, Darwin filesystem/NIO providers, HWUI/Minikin, and Skia.
- [ ] Create a Wine-prefix-like, case-sensitive Android root for `/system`,
      `/product`, `/apex`, package-private `/data`, and brokered shared storage.
- [ ] Route every supported libcore path operation through one mount-aware path
      resolver; never expose host `/Users/...` paths to Android code.
- [x] Add a strict inspection-only Android ARM64 ELF gate for dependencies,
      symbols, relocations, TLS, executable-memory requirements, constructors,
      and raw `svc`; APK-wide aggregation and loader execution remain pending.
- [x] Map Android ARM64 ELF `.so` directly on Apple Silicon, run constructors,
      and execute exports. The loader applies checked `R_AARCH64_RELATIVE`,
      `ABS64`, `GLOB_DAT`, and `JUMP_SLOT` relocations through a closed,
      GNU-version-aware resolver with no Darwin global-symbol fallback.
- [ ] Generate both ARM64 PCS boundaries: AIM calls into Android-ABI JNI
      methods, and Android code calls a proxy `JNIEnv`/`JavaVM` table rather
      than the incompatible Mach-O function table directly.
- [ ] Provide coherent virtual `libdl`, `liblog`, and Bionic `libc` facades for
      file, memory, string, errno, and Android-prefix path behavior. The closed
      `libdl` policy and executable 18-symbol AOSP `liblog` provider pass; libc
      has a complete 160-import classification, 11 passing state-free leaf
      functions, four allocator entrypoints with an explicit Android errno
      seam, and four bit-exact libm leaf functions. Translated/stateful libc and
      ART loader composition remain pending.
- [ ] Execute a realistic Tier-1 JNI library through `Java -> JNI -> Android
      ELF -> Bionic facade -> Darwin`, including pthread and TLS behavior.
- [ ] Add capability-based VM fallback for direct Linux syscalls,
      kernel-specific ioctls, anti-cheat, DRM, and other kernel-coupled code.

Tier 1 is not Java-only. Ordinary applications commonly contain native
analytics, database, compression, crypto, image, or vendor SDKs. The presence
of `.so` files is therefore normal; tiering is based on required ABI and kernel
capabilities. See `ARCHITECTURE.md` for the filesystem, ELF loader, Bionic
facade, syscall policy, and complete tier definitions.

## Deferred performance work

- Darwin `MAP_JIT` and write-protect transitions.
- ARM64 quick compiler/JIT and signal-driven implicit checks.
- Apply the Darwin ARM64 native stack PCS to ART compiler/JIT/AOT JNI call
  backends; the current sparse port validates the interpreter/generic runtime
  path and CriticalNative frame sizing.
- GPU-native game paths beyond the Tier-1 ELF/Bionic work tracked in G4.
