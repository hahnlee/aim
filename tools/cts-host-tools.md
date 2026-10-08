# Darwin CTS extraction tools

The official staged-install `deapexer.zip` contains Linux x86_64 executables.
Issue #1166 tracks its Darwin host execution gap. This build does not replace
or edit that archive, a CTS JAR/APK, or an Android image.

`cargo aim build cts-host-tools` builds the nonboot node. Its direct recipe is
`python3 tools/cts-host-tools.py --jobs 3`. Sources and generated tools stay
under `target/aim/cts-host-tools`; no shared `_build` checkout is edited.
`upstream/cts-host-tools.lock.json` records immutable Android16 source revisions
and SHA256-pinned GNU bootstrap/XZ archives and the official universal CMake archive. The original AOSP deapexer Python
program uses its matching protobuf compiler/runtime and explicit paths to real
Darwin debugfs and fsck.erofs binaries. EROFS compression support requires LZ4,
liblzma, ZSTD and zlib; configure errors are propagated.

The host must provide Git, Apple clang/make, Perl and Python3.
GNU m4, Autoconf, Automake, Libtool and pkgconf are built in the isolated prefix.
`provenance.json` records source pins, prerequisite versions and output hashes.
The pinned EROFS autotools bootstrap and extraction source already contain
Darwin handling; this does not imply its Android Soong targets enable Darwin.

After a successful build, validate against an unchanged original APEX:

```sh
python3 tools/tests/cts-host-tools.py \
  --tools target/aim/cts-host-tools --apex PATH_TO_ORIGINAL_APEX
```

The test creates disposable ext4 and compressed EROFS payloads and checks
extracted bytes, modes and symlinks, corrupt-input rejection, and real original
APEX extraction without changing its hash. A future harness must select this
host-tool path explicitly and record the provenance; existence of the build
recipe is not a passing CTS result.
