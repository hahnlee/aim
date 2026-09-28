# Blue Archive first-frame diagnosis — 2026-09-11

Historical app evidence; current acceptance scope is in
[architecture-migration.md](architecture-migration.md).

## Verified result

After thread-local sigchain mask fix `eb90d025`, unchanged Blue Archive
1.93.454564 base + arm64 split returns normally from nativeRender: 1,204 calls
and 14 scanouts in the requested 15-second window, exit 0. Final scanout shows
Notice, the 654.38 MB download prompt and Cancel/Confirm controls.

Physical-button input, Confirm/download, login and gameplay remain unverified.
The synthetic tap is consumed but the pre-input frame already shows resetting;
this run does not independently prove Cancel dismissal. Earlier Cancel evidence
is indexed in [art-jit-compatibility.md](art-jit-compatibility.md).

## Reproduction identity

| Input / artifact | SHA-256 |
| --- | --- |
| base.apk | `25479ffb2e0710285a6f11e5666273b97edb68f3d6ae16ba78fd388fd7cd45e8` |
| arm64 split | `2049eeaba16d4f6b29c51d0dc5551c1e3e8598274dcfce140c3e816d47602ee4` |
| Runtime | `8cbfe9de1038e39bcda5f1c8ed87104b7083e223ee2c2a491e86f1a35c47624d` |
| Prepared host | `eea5168a5cb4c391eb1e286e557ec6b5d11f27cc178740061adeb32cc46dfa6d` |
| Log | `22dd8166c734c64a68cd42f4f5a1fed33282c94ad4e9c2a69054383b83681954` |

Artifacts: `_build/bluearchive-current.J9Jxq6/`, final `scanout-000014.png`
(RGB mean 0.578154, std 0.227205). Hashes identify this run, not later products.

## Findings still relevant

- Before the fix, first nativeRender waited in IL2CPP GC stop/start-world
  acknowledgment (`+0x19c955c`, sem_getvalue/usleep), with black 1280x720 scanouts.
  Guest suspend/resume signals 30/24 map to Darwin 29/24. pthread_kill success
  did not establish handler execution/acknowledgment.
- Moving dispatcher `sigprocmask(previous_mask)` before special-handler return
  regressed startup and was reverted. Dispatcher-active mask is not necessarily
  interrupted ucontext mask. Preserve real nested suspend/resume and ART-handled
  fault regressions; do not manufacture GC acknowledgments.
- sem_post's mutex/map path lacks POSIX async-signal safety. This is a separate
  review finding, not an observed deadlock or established cause of that run.
- Diagnostic-only instrumentation (`a60d9674`) is opt-in via frame-prefix,
  Unity-stall and pthread-signal variables; it is not a compatibility fix or
  an unrestricted performance measurement.

A new stall needs current artifact identities plus nativeRender return, scanout
pixels and meaningful physical input evidence before changing suspension or
semaphore contracts. Unity/Vulkan startup and exit 0 alone are insufficient.
Detailed superseded traces and rejected experiments live in Git history.
