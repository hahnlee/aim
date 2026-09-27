# Native coverage

Every native method of the pinned, unmodified Android 16 boot and system server
jars is listed in `crates/darwin-art-native-inventory/generated/android16-natives.tsv`
(`cargo run -p art-bootstrap -- native-inventory` regenerates it from the original
jars). `darwin-artctl native-coverage` reports each native's state in the running
system server:

- **registered**: its JNI entry point is bound.
- **by-name**: unbound, but an exported `Java_...` symbol binds it on first call.
- **missing**: a call throws `UnsatisfiedLinkError`.
- **unloaded**: its class is in a standalone system server jar, which the system
  server class loader does not see.

The counts are the system server process's. Application processes register some
natives the system server does not (GL bindings, for example), so their coverage is
higher.

## By jar (2026-09-27)

| Jar | registered | by-name | missing | unloaded |
| --- | ---: | ---: | ---: | ---: |
| `/apex/com.android.appsearch/javalib/service-appsearch.jar` | 0 | 0 | 35 | 0 |
| `/apex/com.android.art/javalib/core-libart.jar` | 320 | 0 | 22 | 0 |
| `/apex/com.android.art/javalib/core-oj.jar` | 477 | 1 | 133 | 0 |
| `/apex/com.android.art/javalib/service-art.jar` | 2 | 3 | 0 | 0 |
| `/apex/com.android.configinfrastructure/javalib/framework-configinfrastructure.jar` | 0 | 0 | 2 | 0 |
| `/apex/com.android.conscrypt/javalib/conscrypt.jar` | 309 | 0 | 0 | 0 |
| `/apex/com.android.i18n/javalib/core-icu4j.jar` | 69 | 0 | 0 | 0 |
| `/apex/com.android.media/javalib/updatable-media.jar` | 0 | 0 | 1 | 0 |
| `/apex/com.android.mediaprovider/javalib/framework-pdf.jar` | 0 | 0 | 32 | 0 |
| `/apex/com.android.os.statsd/javalib/framework-statsd.jar` | 0 | 0 | 1 | 0 |
| `/apex/com.android.tethering/javalib/framework-connectivity-t.jar` | 2 | 0 | 0 | 0 |
| `/apex/com.android.tethering/javalib/framework-connectivity.jar` | 0 | 0 | 188 | 0 |
| `/apex/com.android.tethering/javalib/service-connectivity.jar` | 0 | 0 | 0 | 27 |
| `/apex/com.android.uwb/javalib/service-uwb.jar` | 0 | 0 | 0 | 34 |
| `/apex/com.android.virt/javalib/framework-virtualization.jar` | 0 | 0 | 5 | 0 |
| `/apex/com.android.wifi/javalib/service-wifi.jar` | 0 | 0 | 0 | 1 |
| `/system/framework/framework.jar` | 1608 | 0 | 2951 | 0 |
| `/system/framework/services.jar` | 33 | 0 | 377 | 0 |
| `/system/framework/voip-common.jar` | 0 | 0 | 6 | 0 |
| **total** | 2820 | 4 | 3753 | 62 |

## By package (largest first)

| Package | natives | bound | missing |
| --- | ---: | ---: | ---: |
| `android.opengl` | 953 | 18 | 935 |
| `android.graphics` | 792 | 654 | 138 |
| `android.media` | 557 | 107 | 450 |
| `com.android.server` | 443 | 38 | 367 |
| `android.os` | 442 | 228 | 214 |
| `com.google.android` | 313 | 29 | 284 |
| `com.android.org` | 309 | 309 | 0 |
| `android.view` | 278 | 126 | 152 |
| `android.net.connectivity` | 196 | 0 | 172 |
| `java.lang` | 171 | 148 | 23 |
| `libcore.io` | 161 | 161 | 0 |
| `com.android.internal` | 143 | 38 | 105 |
| `android.renderscript` | 128 | 0 | 128 |
| `dalvik.system` | 124 | 122 | 2 |
| `android.media.tv` | 103 | 0 | 103 |
| `android.filterfw.core` | 101 | 0 | 101 |
| `android.content.res` | 99 | 96 | 3 |
| `sun.nio.ch` | 93 | 30 | 63 |
| `android.graphics.drawable` | 85 | 85 | 0 |
| `android.hardware` | 82 | 44 | 38 |
| `jdk.internal.misc` | 75 | 75 | 0 |
| `sun.misc` | 64 | 60 | 4 |
| `sun.nio.fs` | 64 | 48 | 16 |
| `android.database.sqlite` | 55 | 31 | 24 |
| `android.graphics.pdf` | 49 | 0 | 49 |
| `java.lang.reflect` | 45 | 45 | 0 |
| `com.android.icu` | 44 | 44 | 0 |
| `android.graphics.text` | 40 | 12 | 28 |
| `android.hardware.camera2` | 38 | 1 | 37 |
| `android.media.audiofx` | 35 | 0 | 35 |
| `android.mtp` | 35 | 0 | 35 |
| `java.lang.invoke` | 35 | 35 | 0 |
| `android.graphics.fonts` | 30 | 15 | 15 |
| `android.app` | 28 | 10 | 18 |
| `android.util` | 28 | 21 | 7 |
| `java.io` | 27 | 20 | 7 |
| `java.util.zip` | 26 | 18 | 8 |
| `android.net` | 25 | 9 | 16 |
| `com.android.i18n` | 25 | 25 | 0 |
| `org.apache.harmony` | 25 | 5 | 20 |
