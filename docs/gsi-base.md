# The AOSP GSI as the derived image's base (feasibility, #153)

ADR 0012 builds the derived image from an original archive, today the pinned
emulator image `arm64-v8a-playstore-ps16k-36_r07.zip` (GMS included,
`BE2A.250530.026.F3`). This note records whether that base can move to the
Android 16 arm64 Generic System Image without GMS (`aosp_arm64`), with our
own vendor (the `aim` device) supplying everything the emulator's
`/vendor` supplies now.

Short answer:

- Technically the GSI is almost a drop-in: its 64-bit system is the same
  build as the pinned one.
- Licensing is the reason to switch, and Google's GSI zip does not settle
  it. The GSI is useful as a reference for a self-built `aosp_arm64` at the
  same AOSP tag, which is what we would distribute.

## Download

| | |
| --- | --- |
| Release | Android 16 (stable), `BP2A.250605.031.A3`, build 13578795, security patch 2025-06-05 |
| AOSP tag | `android-16.0.0_r2` (source.android.com build numbers) |
| URL | `https://dl.google.com/developers/android/baklava/images/gsi/aosp_arm64-exp-BP2A.250605.031.A3-13578795-82277143.zip` |
| Size | 915,606,688 bytes |
| sha256 | `8227714351abe504eb27920d0e95c1b672722d2b7c9c9610dad2aee768624add` (verified; matches the published value) |
| Listed on | developer.android.com/about/versions/16/gsi-release-notes ("Downloads", ARM64) |
| Contents | `system.img` (raw ext4, 1.96 GB), `vbmeta.img`, `build.prop` |
| Fingerprint | `Android/aosp_arm64/generic_arm64:16/BP2A.250605.031.A3/13578795:user/release-keys` |

- The pinned system was built on the same day (2025-05-30) from a different
  branch (`BE2A`, emulator). `BP2A.250605.031.A3` is the stable GSI closest
  to it.
- Later releases:
  - QPR1 `BP3A.250905.014` (`android-16.0.0_r3`): same URL pattern,
    `aosp_arm64-exp-BP3A.250905.014-13873947-f9b599ff.zip`, 927,749,769 bytes.
  - QPR2, QPR3 and the Android 17 builds are listed on
    developer.android.com/topic/generic-system-image/releases. That page
    links Android 16 only through Flash Tool (`flash.android.com/preview/baklava-gsi`);
    its artifacts come from the authenticated Android Build API.
- ci.android.com: the legacy `androidbuildinternal` v3 API refuses
  unauthenticated listing ("migrate to Build API v4"), so we did not check
  it for `aosp_arm64` artifacts on a release branch.

### License

The zip is covered by the "Early Access Google Mobile Services and Android 16
GSI License Agreement" on the release-notes page. The GMS+GSI definitions
include the non-GMS `aosp_arm64` image. The relevant terms:

- 3.1: a license "to use GMS+GSI solely for testing applications for
  compatibility with Android 16".
- 3.8: you may not "(b) copy (except for backup purposes), modify or adapt
  any part of GMS+GSI", "(d) create derivative works from or based on
  GMS+GSI", or "(e) provide, sell, license, sublicense, lease, lend, or
  disclose GMS+GSI, or any part of GMS+GSI, to any third party".
- 3.4: "Use, reproduction and distribution of components of GMS+GSI licensed
  under an open source software license are governed solely by the terms of
  that open source software license and not the License Agreement."

So redistributing a derived image made from Google's GSI binary rests on
3.4 alone, applied component by component. That is no better than the
emulator image, which is under the Android SDK License. The binaries are
also signed with Google's release keys. This is not legal advice. The clean
route for distribution is to **build `aosp_arm64` from AOSP source at
`android-16.0.0_r2`**: the output is ours, under the AOSP licenses. The
Google GSI of that tag then serves as a reference for the self-build (see
"Same build as the pinned image" below).

## Extraction

`android-image-extract` handles the GSI zip unchanged: `system.img` is one
of its disk images. A raw image without GPT is read as a single `system`
filesystem, and system-as-root puts it at `OUTDIR/`. The CAPEX/APEX
payloads are flattened as usual.

- 15 s. 4,419 files, 2.48 GB, 36 APEXes. The tree is 2.3 GB, against 3.8 GB
  for the pinned image.
- 7 host name collisions: case-only duplicates under
  `/system/product/media/audio` (for example `arcturus.ogg`), on the
  case-insensitive volume.
- `vbmeta.img` and the zip's outer `build.prop` are ignored.
  `/system/build.prop` carries the same values.
- The GSI's layout differs from the emulator's:
  - `/system_ext` and `/product` are symlinks into `/system`
    (`/system/system_ext`, `/system/product`), not partitions.
  - `/vendor` and `/odm` are empty mount points.
  - There is no ramdisk.
- Fixed here: a bare `OUTDIR` name (no directory part) was rejected with
  "OUTDIR's parent directory must exist". Its parent is the empty path, which
  now means the current directory.

## ELF alignment (16 KiB)

Every `PT_LOAD` `p_align` in the extracted trees, including APEX payloads,
boot image and odex files:

| | GSI | pinned |
| --- | --- | --- |
| arm64 ELF, `p_align` 16 KiB | 1,334 | 1,718 |
| arm64 ELF, `p_align` 64 KiB | 0 | 1 |
| arm64 ELF, `p_align` 2 MiB | 2 | 2 |
| **arm64 ELF below 16 KiB** | **0** | **0** |
| 32-bit ARM ELF (4 KiB: 532, 16 KiB: 46) | 578 | 0 |

- All 64-bit code is 16 KiB-compatible. Android 16 links every 64-bit
  platform binary with a 16 KiB max page size, 4K product or not.
- The only 4 KiB files are the 32-bit userspace: `/system/lib`,
  `app_process32`, `*32` binaries and `arm` boot and odex files, 97 MB in
  `/system/lib` alone. We never run it: our vendor declares a 64-bit-only ABI
  list and `ro.zygote=zygote64`. The overlay can remove it to save space.
- `scan-x18` gives the same x18 and shadow-call-stack counts per APEX as the
  pinned image (the files are the same; see below).
- The GSI's product properties are 4K (`ro.product.page_size=4096`; pinned:
  16384). Nothing we run reads this property. The page size the guest sees is
  the syscall layer's.

## Same build as the pinned image

The pinned emulator image and the GSI share almost their entire 64-bit
system, file for file:

| Tree | GSI files | identical to pinned | differ | GSI only |
| --- | --- | --- | --- | --- |
| `/system/lib64` | 682 | 679 | 2 (`libhwui.so`, `libGLESv2_angle.so`) | 1 (`android.hardware.health@2.1.so`) |
| `/system/bin` | 190 | 180 | 2 (`surfaceflinger`, `cameraserver`) | 8: 32-bit variants, `charger`, and `drmserver64`/`mediaserver64` (the pinned image's `drmserver`/`mediaserver` under their multilib names) |
| `/system/etc/vintf` | 19 | 18 | 0 | 1 (`virtualizationservice.xml`) |
| `/system/framework` (without `.fsv_meta`) | | `services.jar` and all jars but one | `framework.jar` (one dex, `classes4.dex`), `framework-res.apk` (signature only) and the boot image and odex compiled from them | 32-bit `arm` boot image and odex |

- APEXes: the GSI has the same 35 system APEXes, same versions (360499999
  for the mainline ones). They are the AOSP-flavoured modules: the APKs are
  AOSP ones (`PermissionController.apk` where the pinned image has
  `GooglePermissionController.apk`). Their native code is the same:
  - `com.android.art`: 62 of 91 files are byte-identical, among them every
    64-bit library and binary. The other 29 exist only in the GSI: 32-bit
    additions (`lib/`, `dex2oat32`, `dalvikvm32`).
  - `com.android.runtime` (bionic, `linker64`): identical except the 32-bit
    `linker`.
  - `com.android.virt`: the GSI's carries the whole virtualization
    framework, which the pinned one lacks: `crosvm`, `virtualizationservice`,
    `virtmgr`, microdroid images, `VmTerminalApp`.
- The pinned image's 13 vendor APEXes (`com.android.hardware.*`, Widevine)
  are, of course, absent.
- Framework compatibility matrices: identical files. No HAL is
  `optional="false"` at levels 8, 202404 and 202504.

### ART

The `art` node of `cargo aim` builds `patches/art-android/` on
`android-16.0.0_r1` ART (`ed6c006b`) and links against the pinned image's
libraries. For the GSI:

- ART at `android-16.0.0_r2` is the same commit (`ed6c006b`).
- Every library ART links against is byte-identical: the ART APEX's own 64-bit
  libraries (`libbase`, `libc++`, `libartpalette`, ...), bionic and the
  `/system/lib64` LLNDK libraries.

The existing ART exception build is therefore compatible without a rebuild.
Only the boot image must be regenerated, because `framework.jar` differs; the
ART exception regenerates it anyway (`docs/art-exception-patches.md`). The
build script would only need `--image` pointed at the GSI tree, and its
`sources.lock` tag could move to `android-16.0.0_r2`, with no source change.

## GMS and Google leftovers

- GSI: 133 APKs, none named Google/GMS; no permission or sysconfig XML names
  `com.google`. AOSP apps replace the Google ones: Launcher3QuickStep,
  Settings, SystemUI, Browser2, Contacts, Dialer, Calendar, DeskClock,
  Gallery2, LatinIME, messaging, Music, QuickSearchBox, AOSP `webview.apk`,
  and GSI overlays (`gsi_overlay_framework`, `gsi_overlay_systemui`).
- Pinned: 246 APKs, 73 of them Google: GmsCore, Velvet,
  GoogleServicesFramework, Chrome and WebView stubs, YouTube, Maps, Photos, Gmail,
  `*Google` module APKs, and Google RROs.
- Leftovers in the GSI:
  - Google as the build signer (release-keys).
  - `init.gsi.rc`, which imports
    `/system/system_ext/etc/gsi/init.vndk-${ro.vndk.version:-nodef}.rc`.
  - `/system/system_ext/etc/init/hwservicemanager.rc` and the HIDL
    allocator.
  - The product properties with generic names (`aosp_arm64`,
    `generic_arm64`).
  - None of these is Google-proprietary.

## What the `aim` vendor must provide

With the GSI, `/vendor` holds only what we ship. Today the overlay removes the
emulator HALs that we replace or that have no Mac hardware, and keeps the rest.
Everything it keeps must now come from us: built from AOSP source (the same
AOSP code as the emulator's copies) or written by us.

Partition basics:

- `/vendor/build.prop`: `ro.board.api_level=202504` and
  `ro.board.first_api_level=202504`, from which init derives
  `ro.vendor.api_level`.
  - Also `ro.product.vendor.*`, and the 64-bit-only ABI lists
    (`ro.vendor.product.cpu.abilist*`), which keep the GSI's 32-bit half
    unused.
  - Also `ro.zygote=zygote64`, the `dalvik.vm.*` heap and ISA properties,
    `ro.bionic.*` (including an empty `ro.bionic.2nd_arch`: init.rc expands
    it), and `ro.hardware.egl=aim`.
- `/vendor/etc/vintf/manifest.xml`: `target-level="202504"` and
  `<sepolicy><version>202504</version>`, plus a fragment per HAL (ours
  exist).
- `/vendor/etc/init/hw/init.aim.rc`, with `androidboot.hardware=aim`.
  It replaces what `init.ranchu.rc` does today: `ro.hardware.egl`, and the
  emulator's property and permission setup that we still need.
  guest-init's default `androidboot.hardware=ranchu` changes with it.
- sepolicy: we run permissive, but the contexts files are inputs:
  - `vendor_property_contexts` types `vendor.*`, `ro.vendor.*` and
    `ro.hardware.*`; guest-init's property service reads it.
  - `vendor_service_contexts`, `vendor_hwservice_contexts`,
    `vendor_seapp_contexts` and `vendor_file_contexts`.
  - `plat_pub_versioned.cil` and `vendor_sepolicy.cil` (or
    `precompiled_sepolicy`) only once policy is loaded.
  - The GSI carries the platform side, including
    `plat_sepolicy_genfs_202504.cil` and the 202504 mapping.
- Vendor linker config: none needed. Android 16 has no VNDK. Our HALs need
  only LLNDK libraries: `libbinder_ndk`, `liblog`, `libnativewindow`, `libc`,
  `libdl`. An optional `/vendor/etc/linker.config.pb` adds vendor public
  libraries.
- `/vendor/etc/permissions`: the device's feature XMLs. The emulator's
  `handheld_core_hardware.xml` and friends go; ours declare only what the Mac
  backs. Also `/vendor/etc/{passwd,group,fs_config_*}` if vendor AIDs are
  used.
- Device configuration now in the emulator's vendor:
  - `audio_policy_configuration.xml` (with the audio HAL, P5),
    `media_codecs*.xml` (software codecs) and `ueventd.rc` (unused by
    guest-init).
  - Framework RROs for device config: telephony off, display, and what the
    `framework-res__sdk_gphone16k` RRO sets today.

HALs whose absence blocks or breaks system services:

| HAL | Why it is needed | Emulator source today | For `aim` |
| --- | --- | --- | --- |
| `health` `IHealth/default` | BatteryService, charger props | removed; ours | ours (done) |
| `graphics.allocator` + mapper | every buffer | removed; ours | ours (done) |
| GLES driver `libGLES_aim.so` | libEGL | removed; ours | ours (done) |
| `graphics.composer3` | SurfaceFlinger waits for `IComposer/default` | removed (P4) | ours (P4) |
| `security.keymint` + `secureclock` + `sharedsecret` | keystore2; LockSettings and synthetic passwords need keys, so user 0's CE storage stays locked without it | kept: software KeyMint | build AOSP's software KeyMint (`hardware/interfaces/security/keymint/aidl/default`); a Mac Keychain/SEP-backed one later |
| `gatekeeper` | gatekeeperd, credential verification | kept: `com.android.hardware.gatekeeper.nonsecure` | build the AOSP nonsecure/software gatekeeper |
| `audio.core` | audioserver/AudioPolicyManager | removed (P5) | ours (P5) |
| `power` | optional (PowerManager tolerates absence) | kept: vendor APEX | AOSP example or ours |
| `thermal` | optional | kept: vendor APEX | AOSP example or none |
| `identity`, `authsecret`, `drm` (ClearKey), `cas`, `dumpstate`, `neuralnetworks` | optional | kept | build from AOSP only where a feature needs it |
| Widevine | proprietary | kept | not redistributable; drop |

Also for the overlay:

- Remove the GSI's `com.android.virt` services
  (`virtualizationservice.xml`, its `.rc`, `vfio_handler`, `vmnic`). The
  device has no hypervisor, and ADR 0012 forbids one.
- Optionally remove the 32-bit userspace.

## End-to-end check on the GSI

Done in scratch; no shared tree was touched.

1. `android-image-extract` of the zip. Identity
   `8227714351abe504eb27920d0e95c1b672722d2b7c9c9610dad2aee768624add`,
   recorded beside the tree.
2. `android-image assemble` with a 10-entry overlay (all `add`, derived
   identity `657f0224…`):
   - `/vendor/build.prop` as above;
   - a `target-level="202504"` vendor manifest;
   - from main: the health HAL, graphics allocator, mapper and
     `libGLES_aim.so`, with their `.rc` files and vintf fragments.
3. `guest-init --run --only servicemanager,vendor.health-aim --androidboot
   hardware=aim --timeout 40`:
   - The original `servicemanager` finds `IHealth/default` in the vendor
     VINTF manifest, and the HAL registers it.
   - The original `service list` shows
     `android.hardware.health.IHealth/default` and `manager`.
   - `service call android.hardware.health.IHealth/default 7` (getCapacity)
     returns `0x50` = 80, the Mac's battery.
   - This is the same result as on the pinned image (`docs/host-call.md`).
     `hwservicemanager` and `vndservicemanager` are undeclared and not
     needed.

A dry run of the whole boot parses 95 services.

The run also showed one guest-init gap: `ImageRoot` resolves guest paths by
joining them to the host root. An absolute in-image symlink therefore
resolves on the host, so `/system_ext/etc/init` and `/product/etc/init` read
as missing. On the pinned image these are real partitions. On the GSI,
guest-init misses:

- `/system_ext` and `/product` `.rc` files (`hwservicemanager.rc`,
  `init.gsi.rc`);
- `build.prop` files (`ro.product.product.*`,
  `ro.control_privapp_permissions`);
- `*_property_contexts`.

`linux-run` resolves the same symlinks correctly: `ls /system_ext/etc/init`
lists them.

## Recommendation

Do not switch the pinned base now. Switch **before distribution**, to a
self-built `aosp_arm64` system (AOSP `android-16.0.0_r2` or the then-current
tag). Prepare the vendor side at P3.

- Switching now gains no redistribution right: Google's GSI zip is under the
  GSI license (see [License](#license)). It costs the emulator's software
  HALs we still keep, above all KeyMint and gatekeeper.
- Moving later is cheap. The pinned system and the GSI are the same 64-bit
  build: the same ART, bionic, `services.jar` and 679 of 682 libraries, all
  16 KiB-aligned. The existing ART exception build carries over unchanged.
  The derived-image pipeline and the vendor HALs already work on the GSI.
- At P3, as system_server comes up, make the `aim` vendor independent of
  the emulator's:
  - build the software KeyMint and gatekeeper from AOSP source as vendor
    additions;
  - write `init.aim.rc`, the vendor `build.prop`, contexts files and
    feature XMLs;
  - replace the `remove` list with a positive vendor manifest.

  On the emulator base this is a no-op for the system. It makes the base swap
  a pin change.
- Before distribution, build the system image from AOSP source. The Google
  GSI of the same tag is a reference for that build: compare file hashes to
  find anything non-reproducible.

Blockers and work items:

1. Licensing: redistribution needs a self-built system image. Building
   AOSP takes about 300 GB of disk and several CPU-hours, which this machine
   cannot provide now (46 GB free).
2. guest-init `ImageRoot` must resolve absolute symlinks inside the image
   root (the GSI's `/system_ext` and `/product`).
3. The `aim` vendor must supply KeyMint (with secure clock and shared
   secret) and gatekeeper, plus the partition basics above.
4. The default profile was migrated to the pinned image (2026-09-26). A new
   base changes:
   - the platform certificate (dev-keys to release-keys; `framework-res.apk`
     is signed differently);
   - the package names of the Google module APKs (for example
     `GooglePermissionController` to `PermissionController`);
   - GMS, which disappears.

   Switching needs a fresh profile or a tested migration on a disposable
   profile, never the real one.
5. Overlay: remove the GSI's `com.android.virt` virtualization services;
   optionally the 32-bit userspace.

## Distribution option: an image of the GSI's open-source components

Section 3.4 of the GSI agreement states that components under an open-source licence are governed only by that licence. Most of the GSI is Apache 2.0 object code, which may be redistributed. A redistributable base could therefore be derived from the GSI without building AOSP, which needs an x86-64 Linux host and about 300 GB. The steps:

- Classify every file by the licence recorded in the image's `NOTICE.xml.gz`, and keep only open-source components.
- Remove or replace trademarked assets (the boot animation, logos, fonts).
- Re-sign the APKs and APEXes with our keys.
- Publish the corresponding source for the GPL/LGPL components (the AOSP tag).

This depends on reading 3.4 this way, and needs legal review before any public distribution.
