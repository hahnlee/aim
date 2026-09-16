# Installed-record JNI source test

Run `./tools/tests/installed-record-source/run.sh` to compile the actual
production `runtime/framework/pm/installed_record_source.cc` into a temporary
JVM JNI dylib. The only supplied native dependency is the test-only
`fixture.cc` implementation of `darwin_art_runtime_query_package_record`.

The test verifies that the explicit profile socket and package reach the FFI
unchanged, normal UTF-8 records (including a non-BMP path) decode correctly,
status `1` returns `null`, negative status raises `IOException`, and null
socket/package arguments raise their specified Java exceptions. No runtime
provider, Cargo/native shared build, profile daemon, APK, or application is
used. `fixture.cc` is test-only and must not enter any runtime source manifest.
