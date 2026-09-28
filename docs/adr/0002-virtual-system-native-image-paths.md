# ADR 0002: Give ported system-native images canonical Android paths

Status: accepted for the current Chromium compatibility slice

## Context

Android applications may open a system library by an absolute path such as
`/system/lib64/libEGL.so`. On AOSP, Bionic opens that path and uses the
descriptor's device, inode and offset to reuse an already loaded image.

AIM implements selected Android system libraries with host-native
providers. Those providers are resident Android linker images, but no duplicate
ELF file exists in the guest filesystem and therefore no inode can represent
their identity. Requiring a placeholder file would create a second, false
source of truth and could leak the request to macOS `dyld`.

## Decision

- Each immutable ported system-native provider is published with one canonical
  Android pathname and its normal Android SONAME.
- For an explicit-path `dlopen`, the Android libdl boundary may return its
  process-resident provider handle, and the namespace linker may reuse an
  already visible provider image before guest VFS admission, only when the
  request identifies a typed system provider with no file identity and its
  pathname bytes exactly equal the published pathname.
- Namespace membership and direct-link SONAME policy remain authoritative.
  `search_links=0` still prevents transitive re-export.
- No basename matching, host `realpath`, lexical normalization, case folding or
  host-global `dlopen` fallback is allowed. Aliases must be added as explicit
  Android policy if a future unchanged APK requires one.
- Ordinary Android ELF images and host `dyld` images continue to use file
  admission and descriptor identity. A physical file cannot silently replace a
  virtual provider at the same canonical path.

## Consequences

This is a narrow Darwin representation extension rather than literal AOSP
file-loading behavior. It follows the Wine-style boundary: applications retain
the guest ABI and pathname while the compatibility layer owns the host-native
implementation. The extension does not make a virtual pathname readable or
stat-able as a guest file and does not grant new namespace visibility.

Tests must prove exact-path reuse without a VFS call and reject noncanonical,
different-directory and file-backed identities.
