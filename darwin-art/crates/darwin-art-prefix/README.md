# AIM virtual prefix

This crate routes Android-visible byte paths into an immutable mount identifier
and a normalized mount-relative byte path. It deliberately does not require
UTF-8 and does not store or return host paths.

The caller owns the host-side mount table and must open the returned relative
path beneath a pre-opened directory descriptor. The implemented read-only
filesystem broker walks every component with `openat`-style operations,
preserves trailing-slash directory intent, and rejects symlinks. The Bionic
filesystem facade composes it with an immutable guest root, a private in-memory
writable `/data` overlay, and exact synthetic random devices. String
concatenation with a macOS path remains outside this API's security contract;
the prefix router itself grants no host authority.

Absolute `..` components clamp at the virtual Android root, matching Linux
pathname lookup. Mount selection happens after lexical normalization and uses
the longest whole-component prefix. Since lexical `..` is not equivalent to
kernel namei when a preceding component is a symlink, the eventual FD broker is
the authority for actual traversal.

Run the Rust and C ABI acceptance gate with:

```sh
bash tools/build-prefix-resolver.sh
```
