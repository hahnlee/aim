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

The goal is Wine's: the platform's API implemented for each app on the
host, with no Android system behind it. Each surface the app process
reaches may be served in either of two forms, chosen per subsystem and
recorded:

1. **A native binder server (ADR 0013).** The service's pinned AIDL,
   implemented natively and registered under its original name. This is
   how services move today, and the binder interface stays served in
   every case for compatibility: framework internals and apps call it
   directly (`ActivityThread`, `IActivityManager`, GMS through hidden
   APIs and its own stubs), other processes share its state, and CTS tests
   it.
2. **In the app process, natively backed.** The framework classes in the
   app's process that front the subsystem are replaced by ours. They keep
   the original's public and hidden API, and their implementation reaches
   the host through JNI and host-call, as Wine's DLLs implement the
   Windows API in the program's process. This is the long-term direction,
   first where macOS owns the subsystem and the binder round trip only
   translates Mac state into Android parcels and back (windows, their
   configuration and display, input delivery to a window). Each
   replacement:
   - is the smallest class set that carries the surface, with the API of
     the pinned framework (its signatures, behavior and exceptions);
   - is listed as a `replace` in `image/overlay.toml` with its reason, and
     the boot image and oat files are compiled again for it;
   - names its caller to the host by an identity the host can verify (the
     process's guest uid and pid, as binder would carry them), never by
     trust in the caller;
   - keeps the binder interface it bypasses served too (form 1), with
     the same state, for callers that use it directly.
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
- A subsystem may start in form 1 and move to form 2 once its AIDL server
  is conformant. Form 1 then remains as the compatibility surface.
- The first stack's failure (ADR 0012, "Context") came from unversioned,
  reflective, all-at-once replacement. This decision allows replacement
  only per subsystem, at the pinned API, listed, and CTS-gated.
- AGENTS.md's first principle changes with this ADR.
