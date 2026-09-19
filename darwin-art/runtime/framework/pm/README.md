# Package metadata boundary

`installed_record_source.cc` is the system PM JNI record source. Its caller
passes the profile socket explicitly; it delegates wire I/O to Rust
`darwin-art-runtime::package_records` and `darwin-art-profile::package_client`.
Only the daemon's explicit absent-package status becomes Java null. Connection,
framing and malformed-record errors become exceptions, not an invented absence.
Ordinary UTF-8 records are decoded with the real Java charset constructor rather
than JNI's modified-UTF-8 constructor. The old entry Probe socket codec has been
removed; bootstrap still supplies the socket from its launch configuration.
This transport change does not implement AOSP manifest parsing or PMS policy.

`InstalledPackageRecord` owns immutable decoding of the current profile registry
transport: package key, assigned app ID and exact base/split paths. It rejects
ambiguous singleton fields, duplicate splits and malformed record framing, and
does not resolve host paths or infer a native-library directory. Missing legacy
IDs remain unknown, not root. It does not authenticate the transport or parse an
APK; callers must obtain records through the installed-package authority.

`DexLoadReports` uses this installation identity directly. It no longer constructs
an `ApplicationInfo` or interprets SDK/debuggable/application metadata merely to
check code ownership. This also keeps future manifest parsing out of each DEX
report. `InstalledApplicationInfo` consumes the same identity and publishes the
record's split paths plus manifest-derived split names in matching order, as
required by `LoadedApk` ART profile registration. Its remaining `legacyManifestHint` mapping is
explicitly
unfinished migration debt, not original AOSP manifest parsing.

`InstalledApplicationInfo` maps an installed package record to the framework
`ApplicationInfo` DTO. It owns no application lifecycle, classloader, resources,
IPC connection or process-global environment. Callers receive a fresh DTO.
Package mismatches and corrupt numeric metadata fail rather than becoming
plausible defaults; absent legacy app IDs remain unknown (`-1`).

This is not a replacement for AOSP PackageManagerService or its manifest parser.
The current host registry supplies the record through the transport-only
`PackageRecords` native endpoint; migration of service publication and full
package metadata is unfinished. The legacy ProbePackageManager delegates
installed-record construction here for queries and APK launch. ProbeContext
receives the PackageManager copy rather than constructing a second DTO.
Launcher path/resource overlays and service emulation remain debt; this is
not yet a system-owned AOSP bind transaction. In particular, native/split paths for other
packages must come from their installation records, never the launching app's
environment. Do not fill missing metadata with another package's values.

The baseline and Button DEX builders explicitly include this runtime class.
`tools/tests/pm` supplies a test-only DTO seam for plain-JVM mapping tests; that
seam must never enter either DEX. API compilation uses the real Android SDK.
Neither test establishes ART application binding or UI acceptance.

`provider_metadata.cc` validates the encoded provider records without JNI or
environment access; `declared_providers.h` maps the validated, explicit input
to framework ProviderInfo objects. Invalid records throw rather than silently
dropping metadata. The current app bootstrap still supplies its legacy launcher
environment value explicitly. A system binding caller must supply the target
package's installed record instead, never the system process environment.
This does not yet supply all AOSP provider policy fields (process, permissions,
export policy) or execute providers; ActivityThread owns their lifecycle.
The system PM endpoint answers `getProviderInfo` for a declared provider from
the requested installed package, with `GET_META_DATA` controlling the Bundle.
This is a compatibility transaction, not an upstream PMS replacement. See
[`docs/aosp-service-migration.md`](../../../docs/aosp-service-migration.md)
for the full service inventory and replacement sequence.
