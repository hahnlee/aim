# Android linker 16KB application compatibility

Source reference: Bionic Android16 `09a271af557444c9a6b3f3146d6d474156fd6cdb`.
This is a loader contract, not an app-specific loading flag.

## Exact source evidence

- `linker/linker_phdr.cpp`, SHA256
  `6bfb0cd76e2fc41c4d5c6cbae792827e6b18ec3c6aa8f53b7eb9b05b5c18c76a`.
- `linker/linker_phdr_16kib_compat.cpp`, SHA256
  `56ab4266faddda3e7a163af01a1f13a3da2a37784c9e59778d3db4d1c1600b88`.
- `linker/dlfcn.cpp` holds the recursive loader mutex through the public
  `__loader_android_set_16kb_appcompat_mode` setter.

The original mode defaults to false. ElfReader reads the live Android property
`bionic.linker.16kb.app_compat.enabled` and ORs it with the API mode. Effective
compatibility requires host page size 16384 and minimum PT_LOAD alignment 4096.
On a system with pages at least 16384, smaller program alignment is rejected
without effective compatibility, including layouts with no permission overlap.

Enabled compatibility selects the RX/RW layout even if permissions happened
not to overlap before mapping. RO/RX and the RELRO prefix become RX; remaining
writable memory becomes RW. The boundary is shifted onto a host-page boundary.
This is not permission to create RWX pages or ignore unsupported layouts.

## Component integration and evidence (checkpoint1610)

NamespaceHandles owns the process mode and serializes updates with loads. Its
new-load path passes effective policy into the discovered graph before linking.
The Rust loader must carry that explicit snapshot through staging to parsing;
metadata discovery and legacy direct APIs retain their separate contracts.
Resident mappings must not be relocated when the process mode later changes.

An actual NDK ARM64 4KB-aligned fixture reads writable data through executable
code and returns47. Disabled rejection and enabled execution through the
discovered-graph ABI both passed, as did public setter/process-owner identity.
The no-permission-overlap parser regression also passed: explicit Android
policy selects compatibility even where the legacy automatic path did not.
Final Rust library run: 90 passed, 1 existing ABI-specific ignored test.
Native log: `/tmp/android-linker-page-compat.log`; preload remains18/28.
Full NativeLoader/ld-android admission and actual APK acceptance remain separate
and incomplete until their own integration gates pass.
