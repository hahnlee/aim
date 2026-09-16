# AOSP file tree traversal

`bash tools/build-android16-ftw.sh` compiles unmodified Android 16 Bionic
`ftw.cpp`, `fts.c` and `recallocarray.c` from revision
`09a271af557444c9a6b3f3146d6d474156fd6cdb`. Source paths and hashes are in
`sources.tsv`; generated source and archive remain outside tracked code.

The provider-closure build includes this archive's objects. It preserves Android
headers/layouts while using Darwin arm64 calling conventions. Every external
dependency is named `darwin_art_ftw_import_*`, so linking cannot silently
resolve Android `stat`, directory tokens, errno, or paths to macOS libc.
Internal traversal exports use `darwin_art_aosp_*`. `bindings.c` routes imports
to explicit guest filesystem, memory and errno providers. The build compiles
this separate object and checks its exports against upstream's import set.
No unsupported call returns success.

Before installation, execute traversal through the actual admitted libc
provider and verify directory restoration and failures (including fchdir and
fstatat). The closure's traversal smoke directly calls original nftw against
an installed guest root and tests PHYS and PHYS|CHDIR. `ftw@LIBC` and
`nftw@LIBC` are selected by the filesystem namespace adapter via resolver.c.
Richer failure-path and guest ELF traversal execution tests remain unfinished.
Compilation and symbol isolation do not prove traversal or APK acceptance.
