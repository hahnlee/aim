# DEX report policy test

After `art-bootstrap build-button-dex`, compile `DexLoadReports.java`,
`InstalledPackageRecord.java`, and `DexLoadReportsTest.java` to a separate
output directory. Use classpath `_build/dex-probe/classes`,
`_build/package-dex-usage-runtime/package-dex-usage.jar`, and
`_build/package-dex-usage-tests/classes`; run `DexLoadReportsTest` with `-ea`
and the test output directory first on the classpath. No Android
`ApplicationInfo` DTO is required: this test exercises registry ownership
only and must remain independent of manifest/application parsing.

The registry record includes deliberately malformed unused manifest metadata to
prove DEX ownership does not reparse ApplicationInfo hints. This tests
UID/package rejection, own primary/split handling, ISA rejection, and
non-attribution of foreign/traversing paths. No storage is supplied: these
cases must not reach a usage writer. It does not prove secondary persistence,
Binder decoding, isolated processes, or registry-wide shared ownership.
