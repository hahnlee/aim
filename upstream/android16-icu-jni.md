# Android 16 ICU4J JNI bridge for Darwin

This gate builds the complete `libicu_jni` native module selected by the
pinned Android 16 `Android.bp`. It is the native half of classes such as
`com.android.icu.util.regex.PatternNative`; it is not a replacement native
implementation and contains no per-method stubs.

## Source contract

`tools/materialize-android16-icu-jni.sh` fetches only the Gitiles subtree
`android_icu4j/libcore_bridge/src/native` from
`platform/external/icu@f17caeafcf20bd38074a9963c31df3629b70b5f5`. The source
is kept in the separate gitless root `_aosp/external/icu-jni`, so the much
larger ICU4J tree and full ICU repository are not retained.

Although the initial dependency report called this a 14-source module, the
pinned `srcs: ["*.cpp"]` glob selects 15 C++ translation units. The lock covers
all 15 files, all five local headers, and the defining Android.bp. Omitting
`Register.cpp` would remove the module's registration behavior.

## Darwin mapping

`tools/build-android16-icu-jni-foundation.sh` preserves the Android.bp source
set, warnings, and `U_USING_ICU_NAMESPACE=0`, and consumes the existing pinned
ICU, libbase, liblog, libcutils, and libnativehelper headers/providers. The
platform header overlays supply glibc's `bswap_16`, `bswap_32`, and `bswap_64`
spellings with Clang builtins on Darwin, and the single `atomic_bool` type used
by `cutils/trace.h`. The latter avoids Xcode 26's pre-C++23 conflict between
`stdatomic.h` and libc++ `<atomic>` without changing the module's C++ standard.

A monolithic Darwin host links multiple Android JNI modules into one image,
unlike Android's separate shared libraries. To avoid a global `JNI_OnLoad`
collision, only `Register.cpp` is compiled with these mechanical symbol
renames:

```text
JNI_OnLoad   -> aim_icu_jni_on_load
JNI_OnUnload -> aim_icu_jni_on_unload
```

The bodies remain the upstream implementations. The load entry initializes
ICU and registers all 11 native class tables, including PatternNative and
MatcherNative. The declarations are in `include/aim/icu_jni.h`.

Run:

```bash
tools/materialize-android16-icu-jni.sh
tools/build-android16-icu-jni-foundation.sh
```

The outputs are:

```text
_build/icu-jni-foundation/libicu-jni-darwin.a
_build/icu-jni-foundation/libicu-jni-force-loaded.o
```

The runtime must force-load the archive (or link the force-loaded object), then
call `aim_icu_jni_on_load(Runtime::Current()->GetJavaVM(), nullptr)`
after the JavaVM exists and the core ICU classes are resolvable. Treat any
return other than `JNI_VERSION_1_6`, or a pending JNI exception, as startup
failure. Call `aim_icu_jni_on_unload` before destroying the JavaVM.

The force-loaded ICU JNI root must precede its normally extracted providers in
the final link order. In particular, place it before libnativehelper, libcutils,
liblog, libbase, libicui18n, and libicuuc. Appending it after an already reduced
graphics closure is invalid: ld64 cannot go back and extract members from
normal archives it has already passed.
