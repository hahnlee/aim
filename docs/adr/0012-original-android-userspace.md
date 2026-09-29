# ADR 0012: Original Android userspace on a Linux syscall layer

Status: accepted (target architecture; migration tracked in #153). ADR 0013 moves the system services to native implementations; this ADR still governs the app process and every original not yet replaced.

## Context

The runtime reached Android apps by rebuilding and reimplementing Android for
Darwin:

- ART, libandroid_runtime, HWUI and libbinder are rebuilt as Mach-O and
  patched (for example 184 ART patches and 33 frameworks-base patches).
- The bionic ABI is reimplemented as about 36,000 lines of facades (39 crates).
- System services are hand-written Binder endpoints that mimic AOSP owners,
  some reading AIDL transaction codes through reflection.
- Platform libraries such as `libc.so`, `libEGL.so` and `libmediandk.so` are
  resolved by name inside the native loader. No real file backs them.

Every Android release has to be re-fitted to these patches and
reimplementations. Booting the original ActivityManagerService inside the
current bootstrap (#148) showed the cost of mixing the two: the owners assume
the original SystemServer order, the original internal interfaces
(`LocalServices`) and the original natives.

There is no VM: Android runs as ordinary processes on the Darwin kernel.

## Decision

Run the pinned Android userspace **unmodified**, the way Wine runs Windows
programs, and implement only what lies below it.

1. **Guest world, all arm64 ELF.**
   - The original binaries of the pinned image: `linker64`, bionic, ART
     (libart and the image's own boot image), framework and services,
     SurfaceFlinger, inputflinger, audioserver, servicemanager, logd, and apps.
   - They are loaded by the original Android dynamic linker, with the image's
     linker configuration. The runtime has no by-name special cases.
2. **Boundary 1: Linux syscalls.** Linux syscalls are a stable, documented
   ABI; Windows syscalls are not, which is why Wine stops at ntdll.
   - `svc #0` in guest code is redirected at load time to an in-process
     syscall layer on the host.
   - That layer implements Linux semantics on Darwin: files and the device
     view (`/proc`, `/sys`, `/dev`, path mapping), memory (including ART's
     dual-mapped JIT cache), threads (`clone`, futex over `__ulock`), signals
     with Linux contexts, sockets, and the binder driver.
3. **Boundary 2: host-call.** A narrow, versioned C ABI through which guest
   components reach host implementations.
   - **HALs** follow Wine's `.drv` pattern. A guest ELF service (Rust,
     `aarch64-linux-android`, the original Rust AIDL backend over the original
     `libbinder_ndk`) is started from `/vendor/bin/hw` like any vendor HAL. It
     calls a host-side implementation (Rust) over host-call, which uses
     CoreAudio, Metal, AppKit and IOKit.
   - The composer is not an exception (decided in P4): it is a guest HAL
     like the others. Its host module talks to a display server process,
     `aim-display`, which owns the macOS window, because AppKit needs a
     main thread and `linux-run` gives that to the guest
     ([composer.md](../composer.md)).
4. **Exceptions, kept minimal and explicit.** Where the original cannot run
   as-is, we patch or rebuild it from AOSP source with the smallest
   maintainable change, and list it as a `replace` in the overlay manifest.
   Reflection-style bypasses are never an option.
   - **ART:** macOS arm64 cannot map anything below 4 GiB (a fixed
     `__PAGEZERO`), but ART stores managed references as absolute 32-bit
     addresses. `libart` (with its compiler and dex2oat) is therefore built
     from AOSP source with the base-relative compressed-reference patches, as
     Android ELF. The boot image is regenerated with it. The patch series, its
     build and the boot image plan are in `docs/art-exception-patches.md`.
   - **SystemServer (ADR 0013):** `services.jar` without the start of each
     service a native implementation replaces (`image/native-services`):
     that one call's instructions become `nop`s in place, checked
     symbolically at build time (ADR 0013, "Steps").

5. **Derived system image.** The original archive plus a checked-in overlay
   manifest (additions, and explicitly listed replacements and removals)
   with its own identity. The original's identity is the archive's sha256,
   recorded beside the tree when it is extracted.
   - **Case-sensitive disk images** ([storage.md](../storage.md)).
     Android's filesystems are case-sensitive and the Mac's is not: an
     extraction onto the host lost 7 files of the pinned image that differ
     from others only in case. The original is extracted into a
     case-sensitive APFS volume, which becomes an uncompressed read-only
     disk image (4.7 GB for the 3.8 GB tree and its translations); the
     derived image is that image with the overlay in a shadow file. The
     guest's writable data is a sparse case-sensitive image per data
     directory. All are attached hidden by the user, without admin rights.
     The system image is a local build artefact, never distributed.
   - Our HALs, the vintf manifest and the GPU libraries live in `/vendor`,
     as a device vendor's would.
   - **GPU drivers:** additions, not replacements. The original `libEGL`
     and `libvulkan` loaders stay; they load the device's drivers, which
     are ours: `/vendor/lib64/egl/libGLES_aim.so` and
     `/vendor/lib64/hw/vulkan.aim.so`, guest ELF thunks generated from the
     Khronos registry that forward EGL/GLES to ANGLE and Vulkan to the
     pinned MoltenVK release on the host over host-call
     (`docs/gles-driver.md`, `docs/vulkan-driver.md`).
   - The Vulkan driver hands MoltenVK's dispatchable handles to the loader
     unwrapped: they already begin with the loader word and magic the
     loader expects. What Android adds to Vulkan is the driver's own:
     `VK_ANDROID_native_buffer` under the loader's swapchain,
     AHardwareBuffer memory (YUV buffers through external formats) and
     sync-fd semaphores on the GPU, with graphics buffers imported into
     Metal without a copy. MoltenVK's alike queue families (one queue
     each) are presented as one family with all their queues, as HWUI
     asks for two queues of one family.
   - The emulator's vendor HALs talk to QEMU or fake hardware. The overlay
     removes their `.rc` and vintf fragments where we replace them or where
     the Mac lacks the hardware. Hardware the device does not have is then
     not declared, so the original services take their no-HAL paths.
   - A command lists what the derived image adds to, replaces in or removes
     from the original, each with its reason.
6. **init.** aimd plays init, driven only by the original inputs:
   - it runs the `.rc` services with their sockets, users and restarts;
   - it writes the property areas in bionic's format and serves `setprop` on
     the original socket protocol;
   - APEXes are pre-flattened into the derived image;
   - SELinux is permissive.

   The original init may replace it later.
7. **Cross-process kernel state.** aimd holds it, as wineserver does:
   the binder driver core, the process table and uids.

8. **Ahead-of-time translation cache (after Rosetta 2).** Darwin owns x18
   and TPIDR_EL0 and does not honour Linux `svc #0`. The few instructions that
   depend on them are rewritten once per original file, never in the image:
   - `svc #0` becomes a branch into the syscall layer;
   - `mrs`/`msr tpidr_el0` access a per-thread slot;
   - shadow-call-stack push/pop keep their protection through a per-thread
     shadow-stack pointer held in memory, not in x18.

   Translated copies live in a cache keyed by the original file's sha256 and
   the translator version. The system image carries the cache of its own
   files, indexed by path within the image (decision 5); linux-run looks
   there first, then in the user's cache, and rewrites at load time only
   what neither has. The loader maps them file-backed, so pages are
   shared across processes, no code is patched while threads run, and
   code/data identification can use section and symbol information. Only code
   without a cache entry is rewritten when it is mapped: code created at run
   time (an app's own JIT), libraries inside APKs, and 4 KiB-aligned ELF files.

   Translation ABI:
   - **Stubs:** each translated file gets one extra R+X PT_LOAD appended
     after its last segment. It holds the moved program header table and all
     stubs, so the original linker64 reserves and maps them itself.
   - **Host state:** stubs reach it only through Darwin TSD keys 764–767, use
     `b` (never `bl`) and never touch x18.
   - **CTR_EL0:** reads return a fixed value (64-byte lines, IDC/DIC clear).
   - **What the guest sees:** `fstat` reports the translated file's size and
     inode.
   - **Integrity hashes:** files carrying a BoringSSL FIPS module get their
     integrity hash re-injected, as BoringSSL's build does (found by its
     `BORINGSSL_bcm_*` marker symbols; load-time rewriting does the same in
     memory).
   - **Measured on the full image:** 1,721 ELF files translate in 8.2 s
     (520 MiB cache). With a warm cache, starting `linker64 linkerconfig`
     takes 7.8 ms against 12.2 ms with load-time patching. Text pages are
     shared across processes (same vnode and VM object).

Performance is handled as explicit exceptions, never by patching the original
userspace. A gap is closed in the syscall layer, the binder driver, a HAL or
the graphics libraries. The goal is running Android apps with the least code
of our own.

## Consequences

- The old stack is retired once the new runtime matches today's app coverage
  (Calculator, Chromium, games):
  - the bionic facades;
  - the rebuilt and patched ART and framework natives;
  - the hand-written system service endpoints and their reflection;
  - the framework-compat class replacements;
  - the hand-written services.jar natives;
  - the loader's by-name providers.

  Until then, the current runtime stays the default. There is no feature
  flag; the new runtime is built alongside it.
- Upgrading Android becomes mostly a new image pin. What must track Linux is
  the syscall layer; what must track the vendor interfaces is the HALs.
- Risks to settle first (phase P0):
  - Does Darwin preserve x18? Android builds its platform with
    shadow-call-stack, which uses it.
  - bionic TLS (`TPIDR_EL0`) coexisting with Darwin's (`TPIDRRO_EL0`).
  - `clone`-created threads and signal delivery with Linux contexts.
  - The cost of a redirected syscall.

## P0 findings (2026-09-27)

- **x18:** Darwin zeroes x18 on every syscall, yield and signal, and on about
  1% of preemptions. A scan of the full image (2,133 arm64 ELF files, with
  `tools/android-image-extract scan-x18`) finds no shadow-call-stack code on
  our path:
  - linker64 and bionic;
  - libart, libc++, libandroid_runtime, libbinder, libhwui, libgui and
    libandroid_servers;
  - servicemanager, surfaceflinger and app_process64;
  - the boot OAT and services.odex.

  The few x18 instructions there (bionic's SCS bookkeeping and
  register-context save/restore) never rely on its value persisting.

  Three binaries are built with SCS: the Bluetooth JNI, the NFC JNI and
  ot-daemon. They stay off because that hardware is not declared. If ever
  needed, their SCS push/pop can be turned into no-ops at load time, since
  those functions also keep the return address in their stack frame.

  A few Google app libraries use x18 as an ordinary register, which is an
  app-compatibility risk the current runtime already has. The guest must not
  advertise SME, so libc's SME helper never holds a value in x18.
- **TPIDR_EL0 (bionic's thread pointer):** not preserved. Darwin writes a
  per-CPU value there on context switch. At load time, alongside `svc #0`,
  `mrs`/`msr tpidr_el0` are rewritten to load and store a per-thread slot:
  0.9 ns against 0.3 ns native.
- **Syscall redirection:** each `svc #0` becomes a `b` to a per-site stub
  within ±128 MiB. x30 is kept, as Linux does, and x18 is never used. A `brk`
  plus SIGTRAP path covers sites out of range. With the translation cache,
  simple calls (getpid, read, write and similar) take a lean path of about
  3 ns. Calls that run host code keep the full save, at about 21 ns.
- **First run:** the original `linker64` runs on the syscall layer
  (`crates/aim-linux-abi`, `linux-run`). It loads the image's libraries
  through the original linker configuration and runs the original
  `linkerconfig` binary through libc initialization and `main`.

### Platform probes (`experiments/p0/`, removed; see git history)

- **clone:** feasible. A Darwin pthread with a small host stack resumes the
  guest frame; SETTLS, PARENT_SETTID and CHILD_CLEARTID work. The time until
  the child runs is 12 µs at p50, against 13 µs for `pthread_create`. The
  layer must defer bionic's stack-teardown `munmap`.
- **futex:** feasible with an in-process waiter table for private futexes,
  covering bitsets, requeue and exact counts, with kqueue timeouts that have
  no leeway. `os_sync` SHARED is for MAP_SHARED memory. The flavour follows
  the mapping, not FUTEX_PRIVATE_FLAG. PI futexes are open.
- **Signals:** feasible. Linux frames are built for guest handlers, and
  faults are classified through the VM map to give exact SIGSEGV/SIGBUS codes
  and addresses. `rt_sigreturn` goes through a trap. A fault round trip costs
  5.3 µs. A signal queue for `sigwait` is open.
- **JIT dual mapping:** feasible. A named memory entry is mapped RW and RX
  under the hardened runtime with `allow-unsigned-executable-memory`. `mrs
  ctr_el0` traps and must be emulated; the guest must not see HWCAP_CPUID.
- **Syscall cost:** a lean integer-only path costs about 3–4 ns. The full
  save is only for clone, signals and sigreturn.
- **16 KiB pages:** match a Linux 16K kernel. Large reservations are cheap.
  Nothing maps below 4 GiB, hence the ART exception above.

## P1 binder driver (2026-09-27)

Details in [binder-driver.md](../binder-driver.md).

- **A rewrite, not the old crates.** The old binder crates drop remote
  reference counts. They never deliver scatter-gather payloads or fd arrays,
  and they do not route nested calls. `crates/aim-binder-driver`
  implements the kernel driver from the UAPI and `binder.c`. It is a library
  shaped like the file operations.
- **All binder state in aimd.** Per-process state (threads, work
  queues, the receive allocator) is written by other processes'
  transactions, so it stays with the global state in the daemon rather than
  in the guests.
- **Transport: Mach messages.** One `mach_msg(SEND|RCV)` per ioctl on the
  thread's reply port. Parked reads are completed by the waker. Fds travel
  as fileports. Receive buffers are memory entries that the daemon writes
  once.
- **Measured (M2 Pro, `experiments/p1/01-binder-transport`, removed; see git history):**
  - client → daemon → server → daemon → client over Mach: 6.5 µs p50;
  - the same path over Unix sockets: 10.3 µs p50;
  - the core in-process: 9.2 µs p50.
- **Freezing.** `BINDER_FREEZE` fails with `EINVAL`.
- **Reached (2026-09-27).**
  - The original `servicemanager` becomes the context manager under
    `linux-run`, and the original `service list`, `service check` and
    `cmd -l` get their answers from separate processes. It also receives the
    death notice when a registered service dies.
  - The host is the library `crates/aim-binder-host`, run by
    `aim-binderd` until aimd embeds it.
  - A guest binder fd is one end of a socket pair: readiness is a byte on
    it, and release is end-of-file, which also covers process death.
  - Each guest binder thread has one daemon thread; parked reads are still
    to do.
  - Measured: 26–30 µs p50 per small call, timed in the guest with the
    image's `libbinder_ndk`.

## Host-call ABI (2026-09-27)

Details in [host-call.md](../host-call.md).

- **A reserved syscall number.** A host call is `svc #0` with
  x8 = `0x4843_0000`, x0 = module, x1 = function, x2/x3 = argument block and
  length. The translator already redirects every `svc #0`, so nothing
  changes in the translation ABI. A real Linux kernel answers `ENOSYS`.
- **AAPCS64 call semantics.** Caller-saved registers may be clobbered. The
  entry keeps x19/x20, switches to the host stack and calls the Rust
  dispatcher as a C function: 4.6 ns per call against 19.6 ns for a full
  syscall.
- **Versioning.** Module ids are fixed like syscall numbers, function 0
  returns a module's version, and argument blocks are `#[repr(C)]` with an
  exact-size check. Guest pointers are host pointers, valid only during the
  call.
- **Modules.** Host modules are Rust crates linked into the syscall layer.
  The guest side is the `no_std` crate `aim-hostcall`.
- **Vendor HAL pipeline.** HALs are built from AOSP's `libbinder_rs` (vendor
  variant, pinned sources) and Rust generated by the SDK `aidl` from each
  interface's frozen API, which is checked against its `.hash`. As in
  Soong's `rust_aidl`, an interface compiles against the frozen APIs of its
  imports and its crate re-exports theirs, so every interface P4 and P5
  need builds (power, composer3, allocator, audio.core, sensors,
  bluetooth). They link the image's own `libbinder_ndk.so`. The first HAL,
  health V4 over the Mac's battery (IOKit), replaces the emulator's health
  HAL in the overlay.
- **Reached (2026-09-27).** guest-init hosts the binder driver and starts
  services with `linux-run --binder`. The health HAL registers
  `IHealth/default` with the original servicemanager, and the original
  `service list` and `service call` reach it.

## P2 ART (2026-09-28)

Details in [art-exception-patches.md](../art-exception-patches.md); the test
is `crates/aim-linux-abi/tests/art.rs`.

- **Java runs.** The image's own `dalvikvm64`, linker64, bionic, linker
  configuration and ART APEX libraries run a hello-world dex on the ART
  exception: libart, libartbase, libdexfile, libprofile and libopenjdkjvm
  rebuilt with `patches/art-android/`, replaced in the derived image. The
  fixture allocates through GC, starts a thread, takes an implicit null check
  (SIGSEGV to ART's fault handler, a `NullPointerException` caught in Java)
  and runs a hot loop, and its result is checked.
  - **Without a boot image** (`ClassLinker::InitWithoutImage`, the ART
    module's jars plus core-icu4j), interpreted and with the JIT.
  - **The JIT** uses its dual-mapped code cache over a memfd: the executable
    view lies in the heap reference window, the writable one outside it; the
    fixture's `work` is compiled and run.
  - **With the boot image** regenerated by the rebuilt `dex2oat64` on the
    syscall layer (the `boot-image` node of `cargo aim`: the full boot class path
    plus the adservices extension, about 3 s, deterministic), found at its
    default location in `/system/framework`.
- **The heap reference window (#161)** is a reservation in the syscall
  layer; its contract is in art-exception-patches.md.
- **Start-up to exit** of the fixture, best of two, while other builds were
  loading the machine:

  | | load-time rewriting | translation cache |
  | --- | --- | --- |
  | no boot image, interpreter | 0.40 s | 0.80 s |
  | no boot image, JIT | 0.34 s | 0.73 s |
  | boot image, interpreter | 0.48 s | 0.89 s |
  | boot image, JIT | 0.41 s | 0.81 s |

  With the boot image, about 80 ms go to the process and library start,
  140 ms to loading the 15 image components (validating their oat files
  included) and 155 ms to nativeloader preloading the public libraries.
  Hello world does not profit from the boot image, since it touches few
  classes.
- **Fixed on the way.**
  - ART build: Soong's `-ftrivial-auto-var-init=zero` (ART depends on it),
    BoringSSL linked statically into dex2oat64 (#163).
  - Series: the boot image extension's relocation diff (`0003`).
  - Syscall layer: userfaultfd answers `ENOSYS` without a warning (ART then
    keeps its copying collector); `setpriority`/`getpriority` accept this
    process's thread ids, as Linux does, instead of failing with `ESRCH`.
- **Open.**
  - The translation cache makes ART start-up twice as slow as load-time
    rewriting, for the libraries alone as well as with the boot image's oat
    files.
  - No tombstones: crash_dump64 cannot `ptrace` (ENOSYS) and cannot read
    another process's `/proc/<pid>/fd`.
  - `/sys/kernel/tracing/trace_marker` does not exist (libcutils trace).
  - ART's statsd metrics are stubbed until `statslog_art` is generated
    (#162).

## P3 SystemServer (2026-09-28)

Details in [boot-status.md](../boot-status.md), "Java world".

- **The original zygote and system_server run.** zygote preloads 18,367
  classes in 0.31 s and forks system_server; system_server runs its
  bootstrap, core and other services (197 binder services, 245 packages,
  first-boot dexopt through artd), and ActivityManager starts SystemUI and
  other apps through zygote. `sys.boot_completed` is not reached yet: on
  the last runs a wedged host coreaudiod kept the audio HAL from
  registering, and system_server's watchdog fired in AudioService (#217).
- **Replaced daemons are Rust** (`daemons/`, over the image's
  libbinder_ndk): vold (IVold enough for StorageManagerService: user
  storage, the emulated volume as symlink views, no FUSE yet, #221), netd
  (INetd V17 bookkeeping, fwmarkd and DnsResolver, loopback only, #223) and
  apexd (IApexService from the pre-flattened `/apex`). lmkd is not started
  (#222).
- **eBPF scope:** `bpf()` maps live in shared memory and pin into bpffs;
  programs and BTF load but never run (#224).
- **Kernel features added to the contract:** per-process mount namespaces
  as path-map entries, cgroup v2 and bpffs as path-map areas, xattrs with
  SELinux labels (from the original inodes and `genfscon`), and threads with
  their own file table (as processes).
- **Fork on Darwin:** a guest fork is not a Darwin `fork()`, whose child
  cannot reach XPC services (Metal's shader compiler among them; #233). The
  parent spawns a fresh `linux-run` and hands it a copy-on-write snapshot
  of the guest memory as Mach memory entries, plus the layer's state
  (`docs/fork.md`). This replaced a Darwin fork with the layer's locks held
  across it and `__objc_fork_ok`. Guest memory therefore lives in a fixed
  range above the host's allocations.
- **Measured:** system_server reaches `startOtherServices` 8 s after its
  fork and ActivityManager's ready phase at 15 s; system_server's RSS is
  about 220 MB, and 52 guest processes about 730 MB together.
- **`sys.boot_completed` (second part).** A first boot reaches it 25.3 s
  after guest-init starts (zygote 5.8 s, ActivityManager ready 23.3 s),
  with 277 binder services and 74 guest processes (about 2.8 GB RSS,
  shared pages counted per process; system_server 369 MB). Settings
  resumes after `am start -W` (warm, 243 ms). What it took:
  - the audio HAL registers before touching CoreAudio, which runs in a
    child process with timeouts and a null sink (#217, audio.md);
  - an lmkd: ActivityManager waits for lmkd's socket under its lock on
    every process event, so without one the boot stalled into ANRs and the
    watchdog. Ours (`daemons/lmkd`) answers the protocol; it kills on the
    Mac's memory pressure since #277;
  - `/proc/config.gz` for libvintf's kernel configuration;
  - a fault on the heap window's first page is a null-check fault (#236);
  - vold's storage views exist from `initUser0`.
- **Open after P3:** app windows render black because zygote children
  cannot reach Metal's shader compiler service (#233); app data isolation
  leaves apps without their CE directory (#234); the host's security agent
  kills the boot's `linux-run` (#232).

## P4 composer (2026-09-28)

Details in [composer.md](../composer.md).

- **The original SurfaceFlinger composes into a macOS window.** Our
  composer3 V4 HAL serves one display (id 0), the window.
  `guest-init --run` starts servicemanager, hwservicemanager, the allocator,
  the composer and SurfaceFlinger. SurfaceFlinger's RenderEngine (Skia on
  GLES) runs on ANGLE and reaches its main loop, and it starts the original
  bootanimation. That animation plays in the window at 60 fps
  (`screencapture -l`) in the runs where it survives its first second. It
  is often SIGKILLed early by an unidentified sender (composer.md, "Boot").
- **The window is in a separate host process.** `aim-display` owns the
  NSWindow, a `CAMetalLayer` and a `CVDisplayLink`, on the AppKit main
  thread. The host module `display` (id 3) in the composer's `linux-run`
  sends it each client target's memfd once, then 56-byte present records.
  The server maps the memfd, and a present is one render pass from a
  no-copy linear texture into the drawable.
- **Vsync comes from the display, not a timer.** It is taken from the
  display link's timing model (`inOutputTime` minus whole periods):
  intervals have an SD of 0.4 µs, against about 1 ms for the callback
  times. Records go to an fd the HAL reads, since host code never calls the
  guest.
- **Client composition only, and no fences.** A present returns no fence,
  and the HAL reports `PRESENT_FENCE_IS_NOT_RELIABLE`.
- **External textures are emulated in the GLES driver.** ANGLE's Metal
  backend lacks `GL_OES_EGL_image_external`, which RenderEngine requires.
  The driver maps external targets onto 2D textures on hidden texture
  units.
- **Measured (M2 Pro, 1080×1920 at 60 Hz).** A present costs 0.6–0.7 ms
  from request to GPU done, of which 0.13–0.16 ms is GPU time. The
  RenderEngine shader cache takes 12.7 s cold and 0.43 s with ANGLE's cache
  warm.

## P3–P5 acceptance (2026-09-28)

Details in [boot-status.md](../boot-status.md), "P3–P5 acceptance". On the
spawned fork, a first boot reaches `sys.boot_completed` in 50 s; Settings
resumes after `am start -W` and draws in the macOS window (#233 fixed by
the spawned fork); acore, the launcher and GMS no longer crash-loop (#234);
a tap injected through aim-display's touchscreen opens a Settings
sub-page; an AAudio stream at -90 dBFS reaches CoreAudio through
AudioFlinger and our HAL; sensorservice, thermalservice, battery,
bluetooth_manager (ON) and location (gps) show our HALs. It took
selinuxfs `policyvers` for libvintf, Linux-sized datagram buffers for
logd, SSP in the Bluetooth controller's features, context-free
`eglClientWaitSync` and a fix to the fork child's hidden fds. Open: boot
and frame times (#239).

## Phases

| Phase | Target | Status |
| --- | --- | --- |
| P0 | Load-time `svc` redirection; the original `linker64` runs an original `/system/bin` program; x18, TLS, clone, signals and syscall cost measured | reached |
| P1 | The original servicemanager with the binder driver; transaction latency measured | reached |
| P2 | The ART exception (rebuilt libart, regenerated boot image) runs Java | reached |
| P3 | The original SystemServer boots (`sys.boot_completed`) with no HALs declared | reached 2026-09-28 (with the HALs declared; 25 s first boot, 50 s with the spawned fork, #239) |
| P4 | allocator/mapper and composer HALs plus ANGLE/MoltenVK: SurfaceFlinger in a macOS window, one app draws | reached 2026-09-28: Settings draws in the window and takes injected taps |
| P5 | Input, audio, power/health, sensors and camera HALs | input, audio, health, thermal, sensors, Bluetooth and GNSS verified at app level 2026-09-28; camera verified through cameraserver with an NDK Camera2 client 2026-09-28 (test pattern: camera access not yet granted, lid closed; the Camera2 app's preview stays black) |
| P6 | Parity with the current runtime; switch and delete the old stack | old stack deleted 2026-09-28 (git history keeps it); app parity (Calculator, Chromium, games) open |

## Appendix: what we implement, and where

Only components bound to Linux kernel features or hardware are replaced, and
only at a stable, versioned interface. Everything else stays original.

### Replaced native daemons (exceptions at stable AIDL)

| Daemon | Boundary | Why |
| --- | --- | --- |
| netd | `INetd` | iptables, policy routing and eBPF; ours sets interfaces through the layer's ioctls and rtnetlink and keeps networks and routes as bookkeeping (host sockets carry the traffic). The device has one Ethernet network, `eth0`, standing for the Mac's network; Wi-Fi is stage 2 (docs/network.md, #265) |
| vold | `IVold` | mounts, loop devices, fscrypt and dm-crypt; ours mounts FUSE the way vold does and hands the fd to the original MediaProvider |
| lmkd | lmkd socket | PSI and memcg, which Darwin does not report. The guest reports the Mac's whole RAM, so ours kills on the Mac's memory pressure instead: the host-call module `memory` wakes it on each change of the Mac's level and reports free memory (docs/host-call.md). It keeps the original's protocol (ActivityManager waits on the socket under its lock, so an absent lmkd stalls it) and kill order: warn kills cached processes (oom_score_adj >= 900), critical down to perceptible (>= 200), ActivityManager's minfree levels deeper when free memory runs out; the highest oom_score_adj first, one kill per second while the pressure lasts, reported with `LMK_PROCKILL` and `LMK_STAT_KILL_OCCURRED` (#277) |
| apexd | `IApexService` | loop devices and dm-verity; the image is pre-flattened, so ours only reports the active packages (keystore2's module hash, PackageManager) and sets `apexd.status` |

The replacements are Rust (`daemons/`), built against the image's
libbinder_ndk from the AIDL at the pinned tag (the `daemon/*` nodes of
`cargo aim`, docs/build.md).
IVold is an unstable interface: its methods that take a raw
`FileDescriptor` are refused until the Rust backend can express one.

### Added system daemon: the task bridge (window mode)

`aim-windows` (`daemons/windows`, `/system_ext/bin/aim-windows`) is an
addition, not a replacement: in window mode it reports the default
display's freeform tasks to the display server, so each is a macOS window,
and carries out what the windows ask (move and resize, focus, close,
launch). It is a client of the platform's own binder interfaces
(`IActivityTaskManager`, `IWindowManager`) and implements the platform's
`ITaskStackListener`; these Java AIDL interfaces have no NDK backend, so
the few transactions are hand-written with the pinned AIDL's codes and
parcel layouts. Freeform windowing is enabled the way a device vendor
enables it, with the `android.software.freeform_window_management`
feature. No framework code is patched, and no APK is added
([windows.md](../windows.md)).

### Added system tool: the keyboard's layout

`aim-keyboard` (`daemons/keyboard`, `/system_ext/bin/aim-keyboard`) is an
addition too: init runs it with the Mac's keyboard layout, and it sets
that InputDevices layout as the built-in keyboard's layout override,
through one hand-written `IInputManager` transaction (the pinned AIDL's
code), as a device vendor's settings component would. Android's
`KeyboardLayoutManager` still picks and applies the layout; nothing in the
framework is patched ([input.md](../input.md), "Layouts").

### Kept original on the syscall layer

servicemanager, logd, installd, zygote/app_process (`fork` spawns a fresh
`linux-run` that takes over the parent, `docs/fork.md`), SurfaceFlinger,
inputflinger, audioserver, cameraserver, MediaProvider and DnsResolver.

- **Scoped storage:** the original MediaProvider FUSE daemon serves
  `/storage/emulated`. `/dev/fuse` is implemented in the syscall layer, since
  FUSE is a documented kernel protocol like binder. Bulk I/O may later get a
  passthrough exception.
- **Input:** input is not a HAL. The syscall layer exposes virtual evdev
  devices (`/dev/input/event*`) fed by AppKit: the display server's windows
  are a touchscreen, a keyboard and an absolute mouse (hover, buttons,
  scrolling where the pointer is), one Unix socket per open file,
  configured by `.idc` files in the vendor partition; the mouse's pointer
  sprite is the composer's hardware cursor, the Mac's own
  ([input.md](../input.md)).

### Vendor HALs (ours)

allocator/mapper (memfd buffers the host imports into Metal without
copying) and composer (Metal, AppKit), audio (audio.core V3 on CoreAudio
through a lock-free ring the render callback never waits on; the original
audioserver loads it, [audio.md](../audio.md)),
camera (provider V1: one LIMITED device per Mac camera, frames from
AVFoundation converted into the stream buffers by the host module, a test
pattern while macOS withholds access, [camera.md](../camera.md)), sensors
(ambient light and lid angle, IOKit HID),
health (IOKit), thermal (`NSProcessInfo` thermal state, HID temperatures),
GNSS (CoreLocation fixes, no raw measurements), and Codec2 over
VideoToolbox (a performance exception; the original software codecs also
work). Power stays the original vendor APEX's example HAL: macOS offers
nothing unprivileged for it to drive ([vendor-hals.md](../vendor-hals.md)).

- **KeyMint and Gatekeeper:** the AOSP software implementations first, with
  Keychain or the Secure Enclave later.
- **DRM:** the AOSP ClearKey HAL.
- **Bluetooth is required.** Its HAL (`IBluetoothHci` V1) is a virtual
  LE-only HCI controller over CoreBluetooth, in the host-call module
  `bluetooth` ([bluetooth.md](../bluetooth.md)). The controller reports
  BR/EDR as unsupported, scans with `CBCentralManager` into (extended)
  advertising reports, and connects a device only after discovering its
  GATT tree. An ATT server per link then answers the stack's ATT requests
  from that tree, or turns them into `CBPeripheral` reads, writes and
  subscriptions. The Generic Access and Generic Attribute services, which
  CoreBluetooth hides, are rebuilt. Advertising maps the local name and
  service UUIDs onto `CBPeripheralManager`; macOS owns pairing. A USB dongle
  path can add Classic audio. The SCS-built Bluetooth JNI loads through the
  load-time rewrite, which turns its shadow-call-stack instructions.

Undeclared, so absent: telephony, NFC, vibrator, IR, UWB and Thread, plus
`update_engine` and `snapuserd` (the image is pre-flattened).

USB: the device is a USB host (`android.hardware.usb.host`, the Mac's
ports), so UsbService runs; no USB device is passed through yet, so none
appears in `/dev/bus/usb`. It has no USB gadget, hence no accessory or
peripheral mode and no `/sys/class/android_usb`.

### Kernel features to emulate

binder, FUSE, `/proc` and `/sys` per the device contract, the netlink subset
the remaining original daemons use, evdev, and memfd/ashmem.

- **uevents:** NETLINK_KOBJECT_UEVENT sockets receive the kernel's
  uevents (`ACTION@DEVPATH`, the variables, `SEQNUM`) with the kernel's
  credentials; a modeled device's sysfs `uevent` attribute lists its
  variables and synthesizes an announcement when written. sysfs's device
  trees hold only the modeled devices.

- **Network devices** (docs/network.md): `lo` and `eth0` with the
  interface ioctls, NETLINK_ROUTE (link and address dumps, changes and
  groups) and AF_PACKET. `eth0`'s far end is a virtual router whose DHCP
  lease is the Mac's own address, gateway and DNS servers, so the original
  EthernetService and IpClient provision it; guest sockets stay host
  sockets.

- **eBPF:** the `bpf()` syscall with maps backed by shared memory. Programs
  attached to kernel hooks are not run; statistics the stack expects are
  filled in from the syscall layer, which sees every socket operation.
  Settled in P3: maps and pins work, programs and BTF load, and nothing
  runs yet (#224).
- **Mount namespaces** are per-process entries of the path map (bind,
  tmpfs, move, `umount2`); init's own binds are entries every later process
  gets. There is no propagation between processes.
- **The pid namespace** is the process table (`by-pid`, which guest-init
  and every fork write): `/proc` and every call that names a process
  (`kill` and its group and `-1` forms, `tgkill`, pidfds, `sched_*`,
  `*priority`, `getpgid`/`setpgid`/`getsid`, `capget`, `process_vm_*`)
  see its processes only, and a Mac process is ESRCH (`sys/pidns.rs`);
  a parent outside (guest-init) is 0 for `getppid`. Guest pids are host
  pids. A `linux-run` started without a table (tests,
  a debugging shell) gets a private one: a namespace of itself and its
  descendants, which die with it as with a namespace's init. A shell joins
  a boot's namespace with `--by-pid <runtime>/identity/by-pid`. Between
  guest processes the kernel's permission rules hold, on the credentials
  of the table: `kill` and pidfd signals need a matching real or saved
  uid or CAP_KILL (SIGCONT also within the session), renicing and
  rescheduling a matching uid or CAP_SYS_NICE, reading another's limits
  (`prlimit`) matching ids or CAP_SYS_RESOURCE. Another process's limits
  and memory cannot be changed or read (EPERM): Darwin reaches them only
  from inside the process.
- **cgroup v2 and bpffs** are writable areas of the path map: the
  hierarchy holds the directories libprocessgroup creates, and no
  controller acts.
- **Extended attributes** are host attributes under their own prefix;
  `security.selinux` is the original inode's label, a `genfscon` label, or
  `unlabeled`.
- **A thread with its own file table** (bionic's debuggerd pseudothread)
  runs as a process.
- **Answered "unsupported" (Android falls back):** cgroup v1 controllers,
  the other namespaces, SELinux enforcement (permissive), userfaultfd (ART
  uses its copying collector) and BINDER_FREEZE.

### Application JITs

- Code that becomes executable (`mprotect`/`mmap` with PROT_EXEC) is scanned
  and rewritten like translated files: `svc #0`, `tpidr_el0` and
  `ctr_el0`.
- RWX requests map to a `MAP_JIT` region whose per-thread write/execute state
  is switched on fault (`sys/jit.rs`). Darwin cannot place `MAP_JIT` at a
  fixed address and refuses to change the protection of RWX pages, so pages
  made RWX (V8 reserves its code range PROT_NONE, then `mprotect`s it RWX)
  are unmapped and mapped again as `MAP_JIT` at the same address, and RWX
  pages given another protection are replaced by a copy. Darwin restores
  the writable state when a signal handler returns, so the switch back to
  executable runs outside the handler, through the resume trap. A JIT that
  alternates constantly pays for two signals per switch.
- Code in RWX memory that stores into RWX memory (self-modifying code such
  as Widevine's self-decrypting code) could never finish such a store: the
  thread cannot fetch it while writable, and faults again once executable.
  The handler performs the store itself (the plain A64 stores: single and
  pair, general and SIMD&FP registers, every addressing mode, and
  store-release) and moves the thread past it, one signal per store.
- A JIT that allocates x18 as a general register is an app-compatibility
  risk handled case by case.

ART's own JIT uses dual mapping and emits no instruction that needs
rewriting: it keeps the thread in x19 and makes no raw syscalls.

### References

- **Model:** FreeBSD's Linuxulator (`sys/compat/linux`, BSD-2-Clause) is the
  reference implementation for syscall translation: futex, epoll over
  kqueue, eventfd/timerfd, linprocfs/linsysfs and netlink. Code may be
  ported with attribution.
- **Validation:** the Linux Test Project (LTP) arm64 syscall tests,
  run under `linux-run`, measure the layer's fidelity.

