# NativeLoader production cutover audit

Integration inventory inspected 2026-09-12. **Production cutover is incomplete**:
component ELF loads do not prove installed-APK startup. Current product priority
and acceptance are in [architecture-migration.md](architecture-migration.md).

## Production owner to replace

- `compat/darwin_native_bridge_stubs.cc` still exports empty
  `InitializeNativeLoader` / `ResetNativeLoader`.
- `compat/darwin_runtime_native_loader.cc` remains the production library loader.
- Replace selections together in `crates/darwin-art-build-contract/src/lib.rs`
  and `crates/art-bootstrap/src/probe/apk_direct.rs`.
- `tools/build-android16-native-loader-policy.sh` builds original
  `_aosp/android16-classloader-native-state/libnativeloader/native_loader.cpp`
  and ClassLoaderFactory JNI into a separate archive; it is not installed.

## Verified component state

| Boundary | Evidence / constraint |
| --- | --- |
| Policy filesystem | Pinned original device policy stat calls use `filesystem::GuestLinkerStat` with Android layout; host stat imports are rejected. Original predicate is preserved. |
| Admission / publication | Namespace discovery retains admitted FDs and image identities; complete graphs publish before constructors, with rollback. Explicit paths reuse visible inode/device/offset identities while preserving link permissions. |
| Open / constructors | One recursive registry operation covers lookup/link/constructors; same-thread reentry returns a resident lease. Original ICU graph load, repeated dependency reuse and selected-image execution pass. Full guest caller/flags/FD-offset semantics remain incomplete. |
| Relocation | Actual namespace discovery supplies primary/secondary visibility, image-ID scopes and retained per-namespace globals through the C ABI. Real cross-namespace ARM64 fixtures select their own local/global groups. |
| Physical lifetime | Mapping groups retain original group identities and actual DT_NEEDED/relocation winners. Dependency cycles share lifetime components; destructor passes precede unmapping. Selected/global image handles retain their component rather than the whole graph. |
| External resources | Admitted provider/file keys scope retention; legacy unkeyed resources remain conservative. Ambiguous same-name global identities cannot be guessed. |
| Flags / close | Effective GLOBAL/NODELETE state is separate from visibility. Actual NOW\|GLOBAL\|NODELETE execution is still rejected by the parser capability guard; logical Android close eligibility is not implemented by Arc counts. |
| Loader native image | Typed `ld-android.so` references actual loader owners; unchanged original `libdl_android.so` DT_NEEDED load, nine wrapper imports, SDK/page policy, namespace operations, reuse and balanced close pass. Global/SDK metadata and versioned exports are incomplete. |

## Public preload blocker

Latest original public preload result is **18/28**. Hash-pinned missing roots
were materialized in an isolated APFS-cloned baseline; remaining failures are
in dependency admission, not solved by installing root files.

The binder_ndk chain now passes `ld-android`, fdsan owner query, property
iteration, `strtoimax` and `mkdirat`, then stops at **`chown@LIBC`** in libcutils.
Host credentials/stat expose host IDs and overlay ownership is not persisted.
A real fix needs trusted Android credentials plus guest inode ownership shared
with stat. The old overlay fchown success-without-change was removed;
`chown` is not newly exported. Latest full FS, numeric and process-state audits
pass after reviewed dependency pins and Clippy corrections.

Other missing dependency owners include libandroid_runtime (amidi), virtual
device AIDL (camera2ndk), libhwui (jnigraphics), bufferqueue HIDL (mediandk),
libwilhelm (OpenMAXAL/OpenSLES), libhidlbase (RS), configstore HIDL (vulkan),
and libandroidfw (webview support). Do not enable original Initialize while
waiving these failures. Property writes/shared publication/Java authority also
remain unfinished.

## Cutover sequence and acceptance

1. Complete Android logical local-group identity, root-open/dependency counts,
   GLOBAL/NODELETE eligibility and destructor reentry. Keep namespace visibility
   through callbacks; unpublish afterwards, then release dependencies. Raw lease
   drop or whole-graph finalize is not guest dlclose.
2. Complete caller/default selection, public flags, shared/isolated/exempt and
   anonymous namespace behavior, implicit-default links and FD-offset handling.
3. Install one process registry before original NativeLoader initialization;
   route `android_dlopen_ext`, close/error and system-library calls through it.
   Drain users before Reset/teardown; no host dlsym/stat escape.
4. Replace conflicting production exports/archive selection together, register
   original JNI, audit source/link identity and launch unchanged ActivityThread APKs.
5. Verify physical Chromium, Calculator and DeskClock on the new product bytes.

Preserve execution regressions for constructor self/dependency reopen, concurrent
opens, repeated roots, cross-namespace relocation, independently retained
children and exactly-once finalization/unpublication.

## Source and verification anchors

Android16 Bionic revision `09a271af557444c9a6b3f3146d6d474156fd6cdb` is pinned
in `tools/bionic-guest-libdl-runtime/upstream-sources.tsv`; use those locks for
hashes. Relevant contracts: `dlfcn.cpp` recursive operation lock;
`linker.cpp` inode reuse, namespace groups and unload ordering;
`linker_soinfo.cpp` constructor-start guards and effective flags.

```sh
tools/test-bionic-linker-config.sh ROOT --load
```

This exercises generated config, guest filesystem, namespaces and ELF graphs,
not production NativeLoader lifecycle. Latest preload/ownership evidence:
`/tmp/android-linker-native-image-final.log`,
`/tmp/android-mkdirat-preload.log`, `/tmp/android-fs-owner-audit-final.log`.
Keep current blockers here; past numbered checkpoints live in Git history.
