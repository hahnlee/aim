# Android 16 full `core-icu4j` runtime artifact

This gate rejects the legacy ICU 68 artifact in `platform/prebuilts/runtime` and
accepts the complete ICU 76 Java module shipped by an API 36 ARM64 system
image. It does not synthesize or overlay individual Java classes.

## Provenance

`android-16.0.0_r1` in `platform/external/icu` is annotated tag object
`e293941d8913067665b1476c9ed607abad15ba7d`, peeled to commit
`f17caeafcf20bd38074a9963c31df3629b70b5f5`. The tag and the
`android16-release` head therefore name the same tree, not two different ICU
source revisions. The complete `android_icu4j` subtree is tree
`546482966c166a16009c77e224f8fc9f9553facc` and contains:

- 590 `src/main/java/android/icu/**/*.java` inputs for
  `core-repackaged-icu4j`;
- 45 `libcore_bridge/src/java/**/*.java` inputs for `core-icu4j`;
- 18 Java resource files (82,099 bytes);
- the generated `icu_aconfig_flags_lib`, produced from the three declarations
  in `icu.aconfig`.

The authoritative Soong composition is `core-repackaged-icu4j` followed by
`core-icu4j`. Its build-only dependencies include `unsupportedappusage`,
`framework-annotations-lib`, `icu_aconfig_flags_lib`,
`art-module-public-api-stubs-system-modules`,
`art-module-intra-core-api-stubs-system-modules`, the
`app-compat-annotations-source` filegroup, and the
`compat-changeid-annotation-processor`. `jarjar-rules.txt` removes
`android.compat.annotation.**` from the produced implementation jar.

The accepted binary was pulled from the already-installed SDK package
`system-images;android-36;google_apis_playstore_ps16k;arm64-v8a`, revision 6,
whose build fingerprint is locked in the companion lock file. It contains ICU
76.1 Java code, the complete 11-class JNI registrar target set, and
`icudt76l.dat` whose hash is identical to the source-built native ICU 76 gate.
The companion lock also records the ordinary 4 KiB-page Play Store ARM64
system image. Its compressed APEX size and hash differ from the 16 KiB image,
while the extracted `core-icu4j.jar` is byte-identical. The offline extractor's
acceptance gate recognizes only these two complete-image hashes.

The similarly tagged `platform/prebuilts/runtime` ARM64 APEX is not an Android
16 product runtime artifact. Its own `mainline/update.py` describes these APEX
files as ART chroot-test inputs and says architecture APEX updates are skipped
by default when no CI source is configured. That APEX contains ICU 68.2,
`icudt68l.dat`, 1,596 classes and 14,991 methods, and lacks `ATrace` and
`UResourceBundleNative`. The current two-source overlay produces 1,598 classes
and 14,998 methods, but cannot reconcile the remaining 633 ICU 68 source
units, resources, version constants, native contracts, or data with ICU 76.

## Materialization

The gate retains neither the multi-gigabyte system image nor the 37.6 MB APEX
in this repository. `art-bootstrap` first accepts
`AIM_CORE_ICU4J_JAR`, then the ignored local prebuilt path, and finally
extracts the artifact automatically from either locked API 36 Play Store ARM64
system image installed under the Android SDK. A nonstandard image path can be
selected with `AIM_ANDROID16_SYSTEM_IMAGE`.

Pulling from a running AVD remains an equivalent one-time route:

```sh
emulator -avd aim_api36 -no-window -no-audio -no-snapshot
adb pull /apex/com.android.i18n/javalib/core-icu4j.jar /tmp/core-icu4j-api36.jar
tools/audit-android16-core-icu4j-runtime.sh /tmp/core-icu4j-api36.jar
```

For offline extraction `art-bootstrap` invokes
`tools/super-i18n-apex-extract` followed by `tools/apex-ext2-extract`. The image is raw
GPT. In the locked 16 KiB image the `super` partition begins
at LBA 4096. LP metadata v10.0 maps logical `system` to one linear extent at
absolute image byte offset 3,145,728 for 652,832,768 bytes. That extent is
EROFS, not ext4. The APEX file appears at `/system/apex/com.android.i18n.apex`
after mounting the logical system filesystem; its payload remains ext4.

The existing std-only APEX extractor can be reused unchanged after the outer
APEX bytes are available. Direct `system.img` extraction additionally needs a
GPT reader, checked LP geometry/header/table parsing, a logical extent reader,
and an EROFS reader with LZ4 support. It is not a safe one-line generalization
of the current ext4 path walker.

Optional environment inputs make the audit also hash the large source image,
the extracted APEX, and the ICU data without requiring them for every run:

```sh
ANDROID16_SYSTEM_IMAGE=/path/to/system.img \
ANDROID16_I18N_APEX=/path/to/com.android.i18n.apex \
ANDROID16_ICU_DATA=/path/to/icudt76l.dat \
  tools/audit-android16-core-icu4j-runtime.sh /path/to/core-icu4j.jar
```

The accepted jar is DEX 039 with exactly 1,795 classes and 16,316 methods.
`VersionInfo` must expose data path `76b`, initialize ICU 76.1 and Unicode
16.0, and all eleven JNI registrar classes must occur exactly once.
