# Android 16 VirtualRefBasePtr registrar

Android framework vector drawables keep their native `VirtualLightRefBase`
trees alive through `com.android.internal.util.VirtualRefBasePtr`. Android 16
owns its complete two-method JNI table in
`com_android_internal_util_VirtualRefBasePtr.cpp`, a translation unit of
`libandroid_runtime`:

- `nIncStrong(long): void`
- `nDecStrong(long): void`

The Darwin archive is built from that unchanged, revision-locked translation
unit and uses the existing Android `libutils` `VirtualLightRefBase` provider.
It is registered once before app code can inflate a framework vector drawable.

Run `tools/build-android16-virtual-ref-base-ptr.sh`. The output is
`_build/virtual-ref-base-ptr/libandroid-virtual-ref-base-ptr-darwin.a`.
