# ADR 0012: Original Android userspace on a Linux syscall layer

Status: accepted (target architecture; migration tracked in #153)

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
     CoreAudio, IOSurface/Metal, AppKit and IOKit.
   - The composer may become a host-native exception once graphics shows the
     need; that is decided in the graphics phase.
4. **Exceptions, kept minimal and explicit.** Where the original cannot run
   as-is, we patch or rebuild it from AOSP source with the smallest
   maintainable change, and list it as a `replace` in the overlay manifest.
   Reflection-style bypasses are never an option.
   - **Graphics driver libraries:** `libEGL`, `libGLESv2` and `libvulkan` are
     our implementations over ANGLE and MoltenVK. These are stable Khronos C
     APIs, the device's GPU driver seam.
   - **ART:** macOS arm64 cannot map anything below 4 GiB (a fixed
     `__PAGEZERO`), but ART stores managed references as absolute 32-bit
     addresses. `libart` (with its compiler and dex2oat) is therefore built
     from AOSP source with the base-relative compressed-reference patches, as
     Android ELF. The boot image is regenerated with it. The patch series, its
     build and the boot image plan are in `docs/art-exception-patches.md`.

5. **Derived system image.** The original archive plus a checked-in overlay
   manifest (additions, and explicitly listed replacements) with its own
   identity.
   - Our HALs, the vintf manifest and the GPU libraries live in `/vendor`,
     as a device vendor's would.
   - Hardware the device does not have is simply not declared, so the
     original services take their no-HAL paths.
   - A command lists what the derived image adds to or replaces in the
     original.
6. **init.** darwin-artd plays init, driven only by the original inputs:
   - it runs the `.rc` services with their sockets, users and restarts;
   - it writes the property areas in bionic's format and serves `setprop` on
     the original socket protocol;
   - APEXes are pre-flattened into the derived image;
   - SELinux is permissive.

   The original init may replace it later.
7. **Cross-process kernel state.** darwin-artd holds it, as wineserver does:
   the binder driver core, the process table and uids.

8. **Ahead-of-time translation cache (after Rosetta 2).** Darwin owns x18
   and TPIDR_EL0 and does not honour Linux `svc #0`. The few instructions that
   depend on them are rewritten once per original file, never in the image:
   - `svc #0` becomes a branch into the syscall layer;
   - `mrs`/`msr tpidr_el0` access a per-thread slot;
   - shadow-call-stack push/pop keep their protection through a per-thread
     shadow-stack pointer held in memory, not in x18.

   Translated copies live in a cache keyed by the original file's sha256 and
   the translator version. The loader maps them file-backed, so pages are
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
  (`crates/darwin-linux-abi`, `linux-run`). It loads the image's libraries
  through the original linker configuration and runs the original
  `linkerconfig` binary through libc initialization and `main`.

### Platform probes (`experiments/p0/`)

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
  and they do not route nested calls. `crates/darwin-binder-driver`
  implements the kernel driver from the UAPI and `binder.c`. It is a library
  shaped like the file operations.
- **All binder state in darwin-artd.** Per-process state (threads, work
  queues, the receive allocator) is written by other processes'
  transactions, so it stays with the global state in the daemon rather than
  in the guests.
- **Transport: Mach messages.** One `mach_msg(SEND|RCV)` per ioctl on the
  thread's reply port. Parked reads are completed by the waker. Fds travel
  as fileports. Receive buffers are memory entries that the daemon writes
  once.
- **Measured (M2 Pro, `experiments/p1/01-binder-transport`):**
  - client → daemon → server → daemon → client over Mach: 6.5 µs p50;
  - the same path over Unix sockets: 10.3 µs p50;
  - the core in-process: 9.2 µs p50.
- **Freezing.** `BINDER_FREEZE` fails with `EINVAL`.
- **Reached (2026-09-27).**
  - The original `servicemanager` becomes the context manager under
    `linux-run`, and the original `service list`, `service check` and
    `cmd -l` get their answers from separate processes. It also receives the
    death notice when a registered service dies.
  - The host is the library `crates/darwin-binder-host`, run by
    `darwin-binderd` until darwin-artd embeds it.
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
  The guest side is the `no_std` crate `darwin-hostcall`.
- **Vendor HAL pipeline.** HALs are built from AOSP's `libbinder_rs` (vendor
  variant, pinned sources) and Rust generated by the SDK `aidl` from each
  interface's frozen API, which is checked against its `.hash`. They link
  the image's own `libbinder_ndk.so`. The first HAL, health V4 over the
  Mac's battery (IOKit), replaces the emulator's health HAL in the overlay.
  It runs under `linux-run` up to opening `/dev/binder`.

## Phases

| Phase | Target |
| --- | --- |
| P0 | Load-time `svc` redirection; the original `linker64` runs an original `/system/bin` program; x18, TLS, clone, signals and syscall cost measured |
| P1 | The original servicemanager with the binder driver; transaction latency measured |
| P2 | Original libart with the image's boot image runs Java |
| P3 | The original SystemServer boots (`sys.boot_completed`) with no HALs declared |
| P4 | allocator/mapper and composer HALs plus ANGLE/MoltenVK: SurfaceFlinger in a macOS window, one app draws |
| P5 | Input, audio, power/health, sensors and camera HALs |
| P6 | Parity with the current runtime; switch and delete the old stack |

## Appendix: what we implement, and where

Only components bound to Linux kernel features or hardware are replaced, and
only at a stable, versioned interface. Everything else stays original.

### Replaced native daemons (exceptions at stable AIDL)

| Daemon | Boundary | Why |
| --- | --- | --- |
| netd | `INetd` | netlink, iptables and eBPF; ours drives macOS networking. The device exposes one Ethernet-like network, not a Wi-Fi HAL |
| vold | `IVold` | mounts, loop devices, fscrypt and dm-crypt; ours mounts FUSE the way vold does and hands the fd to the original MediaProvider |
| lmkd | lmkd socket | PSI and memcg; not started at first, later backed by macOS memory-pressure events |

### Kept original on the syscall layer

servicemanager, logd, installd, zygote/app_process (`fork` maps to Darwin
`fork`), SurfaceFlinger, inputflinger, audioserver, cameraserver,
MediaProvider and DnsResolver.

- **Scoped storage:** the original MediaProvider FUSE daemon serves
  `/storage/emulated`. `/dev/fuse` is implemented in the syscall layer, since
  FUSE is a documented kernel protocol like binder. Bulk I/O may later get a
  passthrough exception.
- **Input:** input is not a HAL. The syscall layer exposes virtual evdev
  devices (`/dev/input/event*`) fed by AppKit.

### Vendor HALs (ours)

allocator/mapper and composer (IOSurface, Metal, AppKit), audio (CoreAudio),
camera (AVFoundation), sensors, power/health/thermal (IOKit), GNSS
(CoreLocation), and Codec2 over VideoToolbox (a performance exception; the
original software codecs also work).

- **KeyMint and Gatekeeper:** the AOSP software implementations first, with
  Keychain or the Secure Enclave later.
- **DRM:** the AOSP ClearKey HAL.
- **Bluetooth is required.** Its HAL is a virtual HCI controller over
  CoreBluetooth (LE first). A USB dongle path can add Classic audio. The
  SCS-built Bluetooth JNI runs through the translation cache, which keeps its
  shadow call stack.

Undeclared, so absent: telephony, NFC, vibrator, IR, UWB and Thread, plus
`update_engine`, `apexd` and `snapuserd` (the image is pre-flattened).

### Kernel features to emulate

binder, FUSE, `/proc` and `/sys` per the device contract, the netlink subset
the remaining original daemons use, evdev, and memfd/ashmem.

- **eBPF:** the `bpf()` syscall with maps backed by shared memory. Programs
  attached to kernel hooks are not run; statistics the stack expects are
  filled in from the syscall layer, which sees every socket operation. The
  scope is settled in P3.
- **Answered "unsupported" (Android falls back):** cgroups, namespaces,
  SELinux (permissive), userfaultfd (ART uses its copying collector) and
  BINDER_FREEZE.

### Application JITs

- Code that becomes executable (`mprotect`/`mmap` with PROT_EXEC) is scanned
  and rewritten like translated files: `svc #0`, `tpidr_el0` and
  `ctr_el0`.
- RWX requests map to a `MAP_JIT` region whose per-thread write/execute state
  is switched on fault. That works, but a JIT that alternates constantly
  pays about 5 µs per switch.
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

