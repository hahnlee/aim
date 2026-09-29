# Working on AIM

Read [ADR 0012](docs/adr/0012-original-android-userspace.md) and
[ADR 0013](docs/adr/0013-native-system-services.md) before work, and
[docs/boot-status.md](docs/boot-status.md) for how far the image boots. Keep
boot-status.md current with every boot-relevant change: replace superseded
facts instead of appending a log.

## Principles

- An app's own process (ART, the boot classpath, bionic, its native
  libraries) runs original and unmodified. Implement what lies below it:
  Linux syscall semantics, the binder driver, host-call and the HALs. A gap
  there is closed there, never by patching or bypassing the guest.
- System services move, one at a time, from the original SystemServer to
  native implementations backed by macOS (ADR 0013): a replacement
  implements the service's AIDL at the pinned version with generated
  transaction codes, registers under the original name, and replaces the
  original only once it passes that service's CTS module and the app
  checks. Until then the original runs.
- Other exceptions (a rebuild from AOSP source, a replaced daemon, a
  SystemServer change) must be minimal, maintainable and explicit: a
  `replace` in `image/overlay.toml` with its reason, and a note in the ADR.
  Reflection, name interception and by-name special cases in the runtime
  are never acceptable.
- Validate against Linux semantics (man pages, LTP-style tests), not against
  what one guest program happens to need. Port from FreeBSD's Linuxulator
  where useful and attribute it.
- Rust first. Use C, C++ or assembly only where unavoidable.
- No VM or hypervisor: everything runs as Darwin processes.
- No environment-variable feature flags. Configuration comes from the image,
  CLI arguments or code. Breaking migrations happen on a branch.
- Replace missing behavior at its owner, the way a Linux kernel or a device
  vendor would provide it. Do not add success-returning stubs, swallowed
  errors or app-specific branches.
- Keep code minimal and match the surrounding style and comment density. No
  dead code, and no TODO without an issue.
- Treat Android pixels/density and macOS points/backing scale as separate
  coordinate systems; a 2x scanout must not be an upscale of a 1x raster.

## Safety

- Never modify APKs, the original image or its extracted tree. Tools that
  take the original (`android-image assemble --original`) get its real path,
  never a symlink.
- Never touch a user's real data directory; use a disposable one.
- Never circumvent Play Integrity or DRM, and never spoof real device
  identities.
- Kill only processes you started, by pid, and leave none behind.
- Never commit an absolute user path (`/Users/...`); tests locate their
  inputs with `aim_paths` (docs/build.md), scripts relative to the
  repository root.

## Work tracking

- Track open work, bugs and follow-ups as GitHub issues (CONTRIBUTING.md,
  "Tracking work"), never as TODO lists or checklists in documents. File an
  issue for each gap you find, even outside the current task.
- Integration tests that skip because an input is missing count as not run.
