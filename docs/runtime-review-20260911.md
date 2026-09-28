# Runtime identity and acceptance review — 2026-09-11

Historical review after diagnostic fix `3245c8ac`; not current product acceptance.
The same-day later [Blue Archive retest](bluearchive-first-frame-diagnosis-20260911.md)
supersedes this review's black-first-frame blocker. Current priorities are in
[architecture-migration.md](architecture-migration.md).

## Verified scope

- Official incremental graphics link audit passed: registrar 51; no fake
  symbols or host ICU/fmt/CoreText leakage. Log:
  `/tmp/astra-review-graphics-rebuild.log`.
- Development host preparation, min macOS 11.0 / SDK 12.0 and strict signing
  verification passed.
- Real guest signal/mask gate passed 2,000 repeated cycles. Foreign-thread
  ASan/UBSan/TSan passed; real-provider semaphore stress passed.
- HWUI JNI ownership gate passed: owned attachments detach, borrowed ones do
  not. Runtime unit suite passed 28 tests. These are focused component results,
  not game-GC repair, reusable teardown or a full APK shutdown soak.
- Reviewed shutdown order: quiesce application/graphics, join HWUI common pool,
  unload libcore/ELF, detach caller, DestroyJavaVM. Actual reusable lifetime
  safety still requires the current ledger's separate gates.

## Artifact identity

| Artifact | SHA-256 |
| --- | --- |
| Graphics dylib | `c767a68f1f8ecef797ba773754e7163ba78e86d3facd74f21386da0fff96c0a5` |
| Prepared host | `eea5168a5cb4c391eb1e286e557ec6b5d11f27cc178740061adeb32cc46dfa6d` |

These products came from the shared worktree, not a clean-HEAD build. Later
runtime identities must not inherit these results. Per-command logs, provider
hashes and generated graph digest remain in Git history.
