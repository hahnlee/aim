# Original AOSP DEX usage contract test

Run `bash tools/test-android16-package-dex-usage.sh` from the repository.
The script verifies pinned, unmodified `PackageDexUsage` and `AbstractStatsBase`
sources. These test-only platform types permit running the original in-memory
merge and reader/writer logic on the host JVM. They must never enter runtime DEX
inputs. Disk/background-clock entry points throw rather than pretending to work.
The production store build may use these types as a **compile-only classpath**;
its explicit runtime JAR allowlist excludes every platform helper and this test.
Their signatures must match the pinned Android ABI even though host test bodies
are not Android implementations (notably IoUtils takes AutoCloseable).
This does not test Android Binder authentication, guest filesystem persistence,
ART Service integration, or a published PackageManager service.
