# ADR 0014: In-process framework surfaces backed natively

Status: accepted 2026-10-18. Amends ADR 0012 ("unmodified, only below the
guest") and ADR 0013 decision 2 ("the replacement unit is a binder
service").

## Context

ADR 0012 kept an app's own process original: everything it reaches is
answered below it, at the syscall layer, the binder driver or a HAL. ADR
0013 then moved system services to native implementations, but only at
their binder interface: each replacement serves the pinned AIDL with
generated transaction codes.

Surveying apps in the app host (2026-10-18, #1621-#1630) showed what that
costs where macOS is the real owner. A window, its configuration and its
display are a Mac window, its size and its screen. Yet in the app host
they travel as Android's window and display services, through
`IWindowSession`, `IWindowManager` and `IDisplayManager` and their
Configuration and DisplayInfo parcels, to be translated back to the Mac
window. Every app's first frame depends on that translation agreeing with
itself (#1621). Elsewhere the binder interface is the strong contract.
Framework internals and apps call it directly (`ActivityThread`,
`IActivityManager`, GMS through hidden APIs and its own stubs), and CTS
tests it.

## Decision

Each surface the app process reaches is served where it fits best, chosen
per subsystem and recorded:

1. **Binder interface (ADR 0013), the default.** A service whose AIDL
   framework internals or apps call directly, or whose state is shared
   across processes (activity, package, permission, settings, content,
   accounts, jobs, alarms, notifications), keeps a native implementation
   of its pinned AIDL. That covers most of the platform.
2. **In-process, natively backed.** Where macOS owns the subsystem and the
   binder round trip only translates Mac state into Android parcels and
   back (windows, their configuration and display, input delivery to a
   window), the framework classes in the app's process that front it may
   be replaced by ours. They keep the original's public and hidden API,
   and their implementation reaches the host through JNI and host-call.
   Each replacement:
   - is the smallest class set that carries the surface, with the API of
     the pinned framework (its signatures, behavior and exceptions);
   - is listed as a `replace` in `image/overlay.toml` with its reason, and
     the boot image and oat files are compiled again for it;
   - names its caller to the host by an identity the host can verify (the
     process's guest uid and pid, as binder would carry them), never by
     trust in the caller;
   - keeps the binder interface it bypasses served too, for callers that
     use it directly, unless no caller in the image does.
3. **Still never acceptable:** reflection into the guest, name
   interception, and app-specific branches. A replaced class is a
   replacement of the class, not a hook into it.

Completeness is measured on both surfaces: the pinned AIDL's methods for
binder services, and the pinned API surface files (`current.txt`,
`system-current.txt` and the hidden API lists) for in-process replacements.
CTS stays the conformance bar of each.

## Consequences

- An Android upgrade must also carry the replaced classes forward. The
  `replace` list is that work's inventory.
- The first stack's failure (ADR 0012, "Context") came from unversioned,
  reflective, all-at-once replacement. This decision allows replacement
  only per subsystem, at the pinned API, listed, and CTS-gated.
- AGENTS.md's first principle changes with this ADR.
