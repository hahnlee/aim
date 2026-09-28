# Android 16 graphics foundation host archives

`tools/build-android16-graphics-foundations.sh` follows the complete Darwin
host source selection of the Android 16 `libcutils` and `liblog` modules. It
does not add local implementations for unresolved symbols.

Pinned source manifests:

- `platform/system/core` at
  `68be0c2c0006a0740d0b1809abe4717308f90d15`, subtree `libcutils`, with
  `Android.bp` SHA-256
  `f67f553961de89e224c9d6435988d6757dac0a16701912dea15f8af61b603e88`;
- `platform/system/logging` at
  `d78b713380007d3c0dde14712cbcbec27f491ad9`, subtree `liblog`, with
  `Android.bp` SHA-256
  `cb95bc9f49bd420d1448906541d31ce9f66996668aeee5d14693f64915af8199`.

For `libcutils`, the manifest consists of its base sources, `target.host`
(`trace-host.cpp`, `ashmem-host.cpp`), `target.not_windows`, and every source in
the whole-static `libcutils_sockets` host variant. For `liblog`, it consists of
all `liblog_sources` plus `target.not_windows`'s `event_tag_map.cpp`.

The build emits Darwin arm64 static archives under
`_build/graphics-foundations`. Its final gate verifies that the real upstream
`liblog` archive defines the two liblog functions referenced by the current
HWUI Canvas object closure: `__android_log_assert` and `__android_log_print`.
The script requires the generated HWUI undefined-symbol manifest rather than
silently treating those names as an unrelated standalone symbol check.

Apple Clang diagnoses the upstream C99 array designators and SDK-deprecated
`sprintf` calls more strictly than the AOSP host toolchain. The build keeps
Android.bp's `-Werror` and narrowly disables only those two diagnostics; no
source is patched and no replacement implementation is linked.
