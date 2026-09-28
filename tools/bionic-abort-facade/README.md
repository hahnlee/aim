# Bionic abort facade

This standalone provider closes the two pinned Android 35 arm64 libc++
imports `abort` and `android_set_abort_message` without forwarding either
symbol to Darwin or using a dynamic/global lookup fallback.

`abort` follows pinned Bionic ordering: fill the thread mask and unblock
`SIGABRT`, signal the calling thread, replace an ignored or returning handler
with `SIG_DFL`, repeat the unblock and self-signal, then `_exit(127)` if signal
delivery unexpectedly returns. Pinned Android arm64 and Darwin SDK headers are
gated to the same raw `SIGABRT == 6` value. No other Android signal number is
forwarded.

`android_set_abort_message` owns a mutex-protected process-lifetime anonymous
mapping. The first successful message wins, null becomes `(null)`, and the
allocation exactly contains Bionic's two 64-bit magic words followed by the
64-bit total size and NUL-terminated bytes. Darwin has no Linux
`prctl(PR_SET_VMA, PR_SET_VMA_ANON_NAME, ...)` equivalent in this provider, so
the mapping-name feature is an explicit capability gap; no fake name is
reported.

Run `./audit.sh`. It pins both AOSP implementation sources and exact libc++
demand, verifies Android and Darwin signal-value sources, Android ABI and a
real arm64 ELF importing exactly the two `@@LIBC` symbols, closed resolution,
exact message bytes/size/magic under 8-way contention, default/blocked/ignored/
returning-handler fork deaths, message-boundary ASan/UBSan, Rust clippy/format,
and target cleanliness.
