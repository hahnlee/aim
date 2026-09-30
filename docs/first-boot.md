# First boot

Status: design (#565). Implemented: item 4, the data image as a clone
(#563); item 1, the device's APK compiled in the image; item 2, the
package parser cache in a build-time template ("The template").

A first boot (a new data directory) reaches `sys.boot_completed` in about
11 s, a repeat boot of the same data in about 5.4 s. The difference is
work the original does once per device: PackageManagerService (PMS) finds
no `packages.xml`, parses every package, decompresses the compressed
system apps into `/data/app`, and ART Service compiles the packages that
have no compiled code ("first-boot" dexopt). A device's factory image
ships what depends only on the image; the device does the rest. This
document is what the original writes on a first boot, which of it can be
made at build time as the original makes it, and in what order to do it.
Sources are read at `android-16.0.0_r1`.

## What a first boot does

Two first boots of fresh data directories (fb1, fb2, host load 11.5 and
16) and a repeat boot of fb1's (rb, load 14), 2026-09-30, main a75f70cc,
device mode through `cargo aim boot`; system_server's log and events,
`dumpsys package dexopt`, PMS's XML, and every file of the data image
with its size and, for the candidates below, its sha256. Times in
seconds.

| Phase | Owner | What it writes to `/data` | fb1 / fb2 | rb |
| --- | --- | --- | --- | --- |
| `sys.boot_completed`, since guest-init's start | | | 10.90 / 10.85 | 5.37 |
| The data image is created (`diskutil image create`, attach, `newfs_apfs`, detach, attach); `mount_all` waits for it | aim-storage | the empty volume | 0.61 / 0.63 (#563) | 0.10 (attach only) |
| System package scan, 286 packages | PMS | `system/package_cache/<fingerprint>/`: 286 parcels, 4.9 MB | 0.64 / 0.70, `cached: 0` | 0.47, `cached: 286` |
| Stub decompression: `TrichromeLibrary`, `Chrome`, `WebViewGoogle` (`*.apk.gz`, 132 MB) | PMS, installd | `app/~~<random>/<pkg>-<random>/`: 155, 79 and 57 MB with code | 2.88 / 2.90 (#574) | 0.03 (the `/data/app` scan) |
| Default preferred apps, core app data, `write settings` | PMS, installd | `system/packages.xml` (0.38 MB), `packages.list`, `system/users/0/package-restrictions.xml` (62 KB), 27 app data directories | 0.25 / 0.24 | 0.09 |
| Boot dexopt (`UpdatePackagesIfNeeded`, reason `first-boot`, filter `verify`, 4 at once) | ART Service, artd, dex2oat | `dalvik-cache/arm64/` (44 files, 2.4 MB), `app/.../oat/arm64/` | 2.02 / 2.05 | none |
| The other 259 app data directories (deferred, in parallel) | PMS, installd | `user/0/<pkg>`, `user_de/0/<pkg>`, `misc/profiles/{cur/0,ref}/<pkg>` | 1.33 / 1.34 | 0.52 |
| `ANDROID_ID` and the SSAID key | SettingsProvider | `system/users/0/settings_ssaid.xml` | < 0.1 | - |
| Everything from `ams_ready` to the launcher's first draw | | | 0.58 / 0.54 | 0.39 |

In `boot_progress` terms: `pms_start` to `pms_ready` 3.78 / 3.85 s
against 0.63 s, `pms_ready` to `ams_ready` 2.57 / 2.57 s against 0.45 s.
PMS and dexopt make 5.3 s of the 5.5 s difference, the data image's
creation 0.5 s.

**Dexopt.** After the first boot (`dumpsys package dexopt`), 105 system
apps run their image odex's vdex (`[status=verify] [reason=vdex]`: ART
rejects the odex itself, #442, but the vdex serves and nothing is
compiled), 6 run the `oat` node's compiled code (`speed`,
`reason=prebuilt`), and GMS's secondary dex files, which GMS itself
writes into its data after boot, run from their APKs. First-boot dexopt
compiled 24
packages, all without an odex in the image: 21 APKs inside APEXes, the
decompressed Chrome and WebView, and the device's own
`AimNotificationPermission.apk`. A Pixel compiles the same ones, apart
from the last.

**Everything else.** 30 s after `sys.boot_completed`, `/data` held 4,603
files, 830 MB: 353 MB of app data (GMS most of it), 291 MB of
decompressed stubs, and small files of every other owner. Play Store had
already started updating itself (a 166 MB install session in
`/data/app`); the repeat boot found it installed and a 1.3 GB GMS update
being written.

## What can be made at build time

The rule for each item: the result must be the original's own output for
this image, and nothing the original makes per device as an identity, a
key or a salt may be made once and shipped.

### 1. The device's own APK: compiled in the image

`AimNotificationPermission.apk` (an `add` in `image/overlay.toml`) is the
one image APK without an odex. A device vendor preopts its APKs; the
`oat` node compiles it as it compiles `aim-services.jar`. Image only;
saves one package of first-boot dexopt.

### 2. The package parser cache

`/data/system/package_cache/<name>/<apk>-<flags>-<hash>` holds each parsed
package as a parcel (`PackageCacher`). `<name>` is
`PackagePartitions.FINGERPRINT`, a digest of `ro.build.fingerprint` and
every `ro.<partition>.build.fingerprint`
(`PackageManagerServiceUtils.preparePackageParserCache` deletes any other
directory); an entry is used only when it is newer than its APK
(`isCacheFileUpToDate`: the `st_mtime` of the APK, or of the APEX backing
an `/apex` path, below the entry's). A parse depends on the APK, the SDK
and extension versions, the aconfig flags (all from the image) and the
system features (`<uses-permission android:requiredFeature>`,
`persistentWhenFeatureAvailable`). The features include the Mac's SKU
(`androidboot.product.vendor.sku`, the `sku_<sku>` permission files), so
a cache belongs to one image and one SKU.

The 286 entries were byte-identical in fb1 and fb2. The original reads
the cache on every repeat boot; reading it on a first boot is the same
code path with the same result, and it holds no per-device value.

### 3. PMS's state: `packages.xml` and the decompressed stubs

PMS decides "first boot" by one test: `mFirstBoot = !mSettings.readLPw()`,
no `packages.xml` (nor its reserve copy). With the file present and its
`<version fingerprint>` equal to `PackagePartitions.FINGERPRINT`, the
boot is neither first nor an upgrade, and PMS does what a repeat boot
does:

- the scan runs without `SCAN_FIRST_BOOT_OR_UPGRADE` and reads the cache;
- `installSystemStubPackages` skips the stubs: their system packages are
  already disabled and replaced by the `/data/app` copies `packages.xml`
  records;
- `UserSystemPackageInstaller` does not review the user's packages, and
  `applyDefaultPreferredAppsLPw` does not run: their results are in
  `package-restrictions.xml`;
- the app metadata paths of preloaded packages are already set;
- boot dexopt does not run (`DexOptHelper.performPackageDexOptUpgradeIfNeeded`
  returns unless first boot, upgrade or a changed boot class path APEX);
  what the first boot compiled is in `dalvik-cache` and in each
  `/data/app/.../oat`;
- `fixAppsDataOnBoot` creates the app data directories that are missing,
  as on any boot, and records their inodes.

The other `isFirstBoot()` readers are `SystemServer` (boot-time metrics
are logged only when neither first boot nor upgrade) and
`WindowManagerService.main(..., !mFirstBoot, ...)`, which allows boot
messages ("Android is starting") on a boot that is not the first. Both
show only during boot.

The shipped set, all written by PMS, installd and artd:

| Path | What |
| --- | --- |
| `system/packages.xml`, its `.reservecopy`, `system/packages.list` | package settings; the uid list for native daemons |
| `system/users/0/package-restrictions.xml`, its `.reservecopy` | user 0's package state: installed, enabled, stopped, preferred activities |
| `system/package_cache/<fingerprint>/` | item 2 |
| `app/~~*/<pkg>-*/` of the three stubs | the decompressed APKs and their `oat/` |
| `dalvik-cache/arm64/` | what first-boot dexopt compiled for the APKs inside APEXes |

Each with its owner, mode, SELinux label (the xattrs of
`docs/guest-init-contract.md` section 6) and mtime, 300 MB in all, of
which the stubs are 291 MB: bytes the device's first boot writes anyway.

**App data directories stay generated on the device.**
`package-restrictions.xml` records each package's `ceDataInode` and
`deDataInode`, the inode numbers of its data directories. A copy into a
new volume gets other inodes, so the directories are not shipped:
`fixAppsDataOnBoot` makes them on the device and records their inodes,
as it does for any missing directory. That keeps about 0.2 s of core app
directories in the boot and the deferred 1.3 s in parallel.

**Not shipped, generated by the original on the device** (the device's
first boot for every owner but PMS):

| Owner | Files | Why |
| --- | --- | --- |
| UserManager | `system/users/userlist.xml`, `system/users/0.xml` | user 0 is made at the device's first boot, with the same id, so `package-restrictions.xml` applies |
| SettingsProvider | `system/users/0/settings_*.xml` | `settings_ssaid.xml` holds `ANDROID_ID` and the per-user SSAID key: identities. The owner's other files are not separated from it. |
| EntropyMixer | `system/entropy.dat` | the random seed |
| keystore2 | `misc/keystore/` | keys and per-user super keys |
| LockSettings, gatekeeper, recoverable keystore | `system_de/0/spblob/`, `system/locksettings.db`, `system/recoverablekeystore.db` | the user's synthetic password and its keys |
| odsign | `misc/odsign/` | its signing key's certificate |
| bootstat | `misc/bootstat/` | the factory reset time is the device's first boot |
| Permission module | `misc_de/0/apexdata/com.android.permission/` (runtime permissions, roles) | default grants and roles run on the device (no grant fingerprint recorded, as on a first boot) |
| AccountManager, usage stats, app ops, jobs, notifications, dropbox, logs | their files | per-device history |
| Apps | their data directories | GMS and GSF create device identifiers there |
| init, vold, apexd, aconfigd | `/metadata`, `/data/property`, their `/data/misc` directories | cheap, and partly per boot |

**Values the original makes differently on each device.** fb1 and fb2
differ in the shipped files, beyond the random names, as follows; with a
template every install of one image build has the template's values.
None is an identity, key or salt, but each is a per-device outcome of
the original:

- times: each package's last update time (`ut`) and user 0's
  `first-install-time`, and the stubs' first install time (`ft`), are the
  first boot's clock (a system package's `ft` is its APK's mtime, the
  same in both);
- app ids: 132 of 244 packages got another uid, because the parallel
  parser hands packages to the scan in the order they finish; shared
  user ids and the key set ids vary the same way;
- a race: 6 permissions (`ACCESS_ADSERVICES_*`) are declared by two
  system packages, and the one scanned first owns them (`ext.services`
  in fb1, `adservices.api` in fb2);
- random values: the stubs' code paths, `/data/app/~~<16 random bytes>/
  <pkg>-<16 random bytes>` (`getNextCodePath`, SecureRandom), visible as
  the `sourceDir` of Chrome and WebView; one domain verification set id
  per package (`DomainVerificationService.generateNewId`, a random
  UUID), which the verifier agent and the package itself can read;
- compiled code: the stubs' odex and the APEX APKs' `dalvik-cache` files
  are not byte-identical between the two boots; each is valid.

The PackageManager verifier identity (`<verifier device=...>`,
`VerifierDeviceIdentity.generate()`) is made on first request. It was in
neither `packages.xml`; the build fails if a template's has it.

Saves the stubs, most of the scan and all of dexopt: about 5 s of the
first boot.

### 4. The data image itself (#563)

A new data directory's image is made with `diskutil image create`,
attached, formatted with `newfs_apfs`, detached and attached again: 0.6 s
of `mount_all`'s wait on the first boots above, against 0.1 s for an
attach. An APFS clone (`clonefile`) of a prepared image is instant,
whatever it holds. Measured on this Mac (2 GB ASIF, empty case-sensitive
APFS volume): creating it 0.74 s; two clones 15 ms; attaching each
0.35-0.40 s; the two clones attached at once, each at its own mount
point, their writes kept apart. The clones share the volume's UUID; the
guest does not see it (`statfs`'s `f_fsid` comes from the host device
number).

This works for an empty image too, so it can land first: the data image
is a clone of an empty template, and item 3's template is later the same
mechanism with content.

## The template

### Made by a build-time first boot

The `userdata/template` node of `cargo aim` (after AOSP's data
partition image; `crates/aim-build/src/nodes/userdata.rs`) runs the
original's first boot at build time and keeps what is shipped (today
item 2's parser cache):

1. **Boot.** guest-init boots the derived image on a scratch data
   directory made from the empty image, the same boot as a user's first
   boot, in window mode (which shows only app tasks; a first-run task
   may still show a window for a moment, #607). At
   `sys.boot_completed` it shuts Android down as a device does (`svc
   power shutdown`, run with init's environment as `adb shell` runs it:
   ShutdownThread has PMS write its settings, then init's
   `sys.powerctl` ends the boot, 3 s later). Play Store starts updating itself within 30 s of
   `sys.boot_completed`; what it installs is not in the shipped set, and
   step 2 fails the build if it reached `packages.xml`.
2. **Check.** `packages.xml` lists only the image's packages (no package
   an installer added), has no verifier identity, and records the
   image's fingerprint.
3. **Copy.** A new, empty data image receives the shipped paths from the
   build boot's image, with owners, labels and mtimes. A copy into a
   fresh volume, not a deletion from the used one: the used volume's
   freed blocks would still hold its keys and seeds.
4. **Publish.** `target/aim/userdata/userdata-<image>-<sku>.asif` (the
   derived image's identity, the SKU the build boot's guest-init gave
   the device), with `.sha256`, the shipped files and their sha256.
   The node takes about 30 s of a build whose derived image changed
   (28.5 s on 2026-10-01: the boot 14.5 s, the shutdown 3 s).

The node's key is the derived image's output key (its identity and its
shadow's creation time: the APK mtimes the cache and PMS compare are the
shadow's) and the node's recipe; the template's name carries the image
and the SKU, and guest-init takes only the one of the image it boots. It does not depend on the
runtime's binaries (order-only, as the `oat` node's `linux-run`): the
content is the original's output, whatever runs it. A SKU without a
template (another Mac) boots without one until one is built.

### A data directory starts from it

`aim_storage::data::DataImage::attach` of a directory without an image
clones the template into `<data>.asif` (`clonefile`; a real copy when the
data directory is on another volume), then attaches it as a repeat boot
does; an image smaller than the host volume is grown as today. guest-init
gets the templates' directory as an argument (`--userdata DIR`, from
`cargo aim boot` and aimctl) and picks the template of the booted image's
identity and the device's SKU (`aim_storage::data::template`), else the
empty image `DIR/empty.asif`. Without either, the image is created empty.

The first boot then runs the original from that state: PMS as on a repeat
boot, every other owner as on a first boot, generating the device's own
identities, keys and seeds.

### Checks

- **Structure.** Two builds of the template must give the same shipped
  set up to the values listed in item 3: an integration test compares the
  parser caches byte for byte, the stubs' APKs byte for byte, and
  `packages.xml` and `package-restrictions.xml` with those values
  normalized (times, uids, key set ids, code path names, domain set ids,
  the owner of a permission two packages declare). A new kind of
  per-boot value in PMS's output then fails the test instead of being
  shipped unnoticed.
- **Parity.** A template first boot against an original first boot:
  `dumpsys package` (packages, flags, permissions and their flags,
  preferred activities, domain verification state), `pm list packages
  -f`, `dumpsys package dexopt`, `cmd role`, user 0's runtime
  permissions: the same up to the listed values. CTS, both ways, the same
  results: CtsContentTestCases (`android.content.pm.cts`),
  CtsDomainVerificationDeviceStandaloneTestCases, CtsOsTestCases,
  CtsBootStatsTestCases. And the app checks (the integration gate,
  Settings, Chrome, Calculator, the tracked games).

## Image changes and existing data

- **New data directories** take the template of the image they boot, so
  they always match it.
- **Existing data directories** never take a template: `DataImage::attach`
  clones only when there is no image. Users' data is not touched.
- **When the image changes under existing data**, PMS sees the same
  fingerprint (the derived image keeps the original's build properties),
  so it does not take its upgrade path (`isDeviceUpgrading`). It still
  notices changed system APKs by their mtime ("A package on the system
  image has changed"), re-parses them (their cache entries are older),
  and ART rejects compiled code whose boot image or dex checksums
  changed. That is how it works today, template or not; that a derived
  image never takes Android's upgrade path is #575.

## Expected savings

First boot to `sys.boot_completed`, from the check above (host load
11-16):

| Step | Saves |
| --- | --- |
| 4: the data image as a clone | 0.5 s (#563) |
| 1: the device's APK compiled in the image | one of 24 dexopt packages |
| 2 + 3: the template | the stubs (2.9 s), the uncached part of the scan (0.2 s), dexopt (2.0 s), most of the first-boot part of `write settings` (up to 0.15 s): about 5.2 s |

With all of it, a first boot is a repeat boot plus the device's own work
outside PMS (user 0, settings, keys, default grants) and the core app
directories: about 5.5 s at these loads (10.9 s less the 5.7 s above,
with the core directories and the rest of `ams_ready` to the first draw
still first-boot work), against 5.4 s for the repeat boot. The last step to 5 s at low load is the repeat boot's own
time (#529 and the pre-zygote work).

## Risks

- **Parity.** PMS on a state another boot made: the risk is in the owners
  that see PMS as "not first boot" while their own state is new. The
  permission module is the likely one: on an original first boot a
  package's runtime permission state (and flags such as a system app's
  restricted-permission exemptions) is made as the scan adds the package;
  from the template the packages are known, and user 0's state is read
  from a file that is not there before default grants run
  (`getDefaultPermissionGrantFingerprint` is unset). If the parity check
  shows a difference there, the permission module's user 0 state
  (`runtime-permissions.xml`, `roles.xml`: no identity) joins the
  template; nothing of the original is changed.
- **Per-device values.** Item 3's list; the structure check catches new
  ones.
- **The build boots Android.** A boot failure fails the build, the build
  takes about 30 s longer when the derived image changes, and the security
  agent may flag the boot (#232), as any boot.
- **The Mac's SKU** keys the template; another host fact that changes the
  features would have to key it too.
- **Size.** 300 MB per template and SKU; clones share its blocks until
  the guest writes them.

## Decisions

1. Whether a template may carry item 3's per-device outcomes (times, app
   ids, the permission owner race, the stubs' code path names, domain
   verification set ids), identical on every install of one image build.
   None is an identity, key or salt. Without them there is no item 3, and
   a first boot stays about 10 s (items 1, 2 and 4 save under a second).

## Order

1. The data image as a clone of an empty template (item 4, #563): no
   Android content, and the mechanism the rest uses.
2. The device's APK compiled by the `oat` node (item 1): image only.
3. The `userdata` node shipping the parser cache alone (item 2), with the
   structure check: proves the build boot, the copy with owners, labels
   and mtimes, and the keys, with an output that has no per-device value.
4. PMS's state (item 3), after the decision, with the parity check and
   CTS against an original first boot.
