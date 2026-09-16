# AOSP DebugStore build inputs

This directory is a build template, not another event-store implementation.
`tools/build-android16-debugstore.sh` validates upstream source hashes from
`upstream/android16-debugstore.lock`, stages the original Rust sources under
`_build/debugstore/crate`, and uses this Cargo manifest/lock and CXX build script.
The original framework JNI source is compiled into the same native archive.

The AOSP store owns event IDs, its bounded queue, event encoding and concurrency.
The existing `android::uptimeMillis` implementation supplies the clock. Runtime
registration calls the original JNI registrar; no application-specific behavior
or alternate Java event handling is installed.

The C++ smoke test exercises the real CXX/Rust store and kernel-backed clock,
including concurrent writes and bounded snapshots. It does not substitute for
managed JNI registration, application launch or physical UI acceptance.
