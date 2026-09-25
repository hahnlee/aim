# Package metadata boundary

`installed_record_source.cc` is the system PM JNI record source. Its caller
passes the profile socket explicitly; it delegates wire I/O to Rust
`darwin-art-runtime::package_records` and `darwin-art-profile::package_client`.
Only the daemon's explicit absent-package status becomes Java null. Connection,
framing and malformed-record errors become exceptions, not an invented absence.
Ordinary UTF-8 records are decoded with the real Java charset constructor rather
than JNI's modified-UTF-8 constructor.

`InstalledPackageRecord` decodes the profile's install ledger entry: package
key, assigned app ID, native library directory and exact base/split paths. It
rejects ambiguous singleton fields, duplicate splits and malformed framing.
Missing legacy IDs remain unknown (`-1`), not root. The ledger records host
code paths; the system process reads the same files through the read-only
`/data/app` mount (bionic fs facade mount 4, `DARWIN_ART_ANDROID_PACKAGE_ROOT`),
and `guestCodePath`/`hostCodePath` translate between the two.

`InstalledPackageParser` parses the installed APKs with the original AOSP
`PackageParser2` (`ParsingPackageUtils`, `PARSE_COLLECT_CERTIFICATES`, so
`ApkSignatureVerifier` runs). Its callbacks mirror PackageManagerService's:
features and allowlists come from `SystemConfig`, compat changes from
PlatformCompat. The parsed package name, base and split set must match the
ledger entry; a parse failure is logged and thrown, never replaced by defaults.

`InstalledPackageInfos` generates every framework package DTO
(`PackageInfo`, `ApplicationInfo`, component infos) with the original
`PackageInfoCommonUtils`, honoring the caller's query flags. Only the state
PackageManagerService keeps in its `PackageSetting` (uid, code paths, data and
native directories, installed flag) comes from the ledger. It also answers the
queries system services need: service lookup (`ServiceResolver`, used by
ActiveServices and JobScheduler), package-scoped intent resolution, the
launcher activity, and the providers ActivityManagerService installs in a
process at `bindApplication` (`processProviders`, as
`generateApplicationProvidersLocked` queries them).

This is not PackageManagerService. There is no persistent parse cache,
package settings store, permission state or install path yet; booting AOSP
PMS on top of this parser is tracked in #30. Apps still receive host code paths
until they get the `/data/app` mount themselves (#35).

`DexLoadReports` uses the ledger identity directly to check code ownership.
`tools/tests/pm/installed-package-record-test.sh` covers the ledger decoding
and path translation; parser and DTO generation run only against the pinned
framework in the system process. See
[`docs/aosp-service-migration.md`](../../../docs/aosp-service-migration.md)
for the full service inventory and replacement sequence.
