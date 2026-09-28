# Android 16 libbase Darwin provider

This gate replaces the earlier seven-member compatibility archive at
`_build/libbase-foundation/libandroid-base-darwin.a`. It parses Android 16
`libbase_defaults` and its Darwin target branch, yielding all 18 production
translation units, including `posix_strerror_r.cpp` and `errors_unix.cpp`.
Android.bp's `whole_static_libs: ["fmtlib"]` is preserved by adding the exact
`fmtlib` `src/format.cc` object to the same provider archive. `liblog` remains
a real shared-module dependency and is linked into the executable acceptance;
it is not replaced by logging stubs.

`tools/sync-android16-libbase-foundation.sh` reuses the current sparse source
when present and otherwise materializes only the immutable source files,
public include subtrees, and private `logging_splitters.h` through Gitiles. It
creates no repository history.
That fresh sparse closure is 846,787 logical bytes: 54 libbase files/268,307
bytes and 16 fmtlib files/578,480 bytes.

Run:

```sh
tools/build-android16-libbase-foundation.sh
```

Acceptance requires 19 arm64 members, force-loaded definition and unresolved
manifests, and an executable smoke covering parsing, strings, properties,
file IO, the real fmt object, logging severity state, Darwin error conversion,
and the exported `posix_strerror_r` compatibility entry point.
Apple Clang classifies libbase's two intentional ancillary-data stack arrays
in `cmsg.cpp` as `-Wvla-cxx-extension`; that diagnostic alone is suppressed
while the upstream `-Wall -Werror -Wextra -Wexit-time-destructors` policy stays
enabled.

The verified provider is a 951 KiB archive with 19 members. Its 1,092-entry
global-definition manifest has SHA-256
`9fb3377db28e9ca815f9640d3a24cfb182550736f293c410f30502fab22a32e5`.
The force-loaded object records 185 libc++/Darwin/liblog dependency symbols
with SHA-256
`0bd7a6ba5079f251b9faf173f57641fb44f428cb843a8b9b2b547a71e37cda0d`;
the real liblog archive resolves its logging dependency in executable
acceptance. The smoke output manifest SHA-256 is
`69f081a96e9ceaa1b89930039641cb80a6ebe635313a4e8e3908dbd40bd9b249`.
