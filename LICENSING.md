# Licensing

Original work contributed to AIM is licensed under the
[Apache License, Version 2.0](LICENSE), unless a file or the exceptions below
specifies otherwise. Copyright remains with the respective contributors.
This grant covers our original contributions, not rights in third-party work.

## Third-party exceptions

The root license does not replace existing upstream licenses, copyright
notices, exceptions, or attribution requirements. This applies even when an
upstream file is modified, translated, embedded in a script, or renamed.

| Material | Applicable terms |
| --- | --- |
| `patches/art-android/` | The terms of each patched upstream file (AOSP ART, Apache-2.0); our changes to those files are provided under those terms |
| Code generated from the Khronos registry and ANGLE's extension registry (`hal/gles/src/thunks.rs`, `crates/aim-host-gpu/src/table.rs`) | The registries' terms (Khronos: Apache-2.0; ANGLE: BSD-3-Clause) in addition to ours |
| Downloaded sources, tools, images, libraries and generated derivatives | Their upstream terms; their presence in a build directory does not make them Apache-2.0 |
| `crates/aim-services/src/package/owner/seinfo/sort.rs` (libcore TimSort port) | GPL-2.0-only WITH Classpath-exception-2.0; upstream notices retained, see [licensing/OPENJDK.md](licensing/OPENJDK.md) |
| License and notice documents | Reproduced under their original terms, not relicensed by the root license |

See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for attribution,
[licensing/OPENJDK.md](licensing/OPENJDK.md) for the OpenJDK code compiled by ART and the native service host, and
[licensing/upstream-sources.json](licensing/upstream-sources.json) for the
origins and hashes of the copied notice documents.

For an independent new source file, use:

```text
SPDX-License-Identifier: Apache-2.0
Copyright 2026 AIM contributors
```

Use the actual copyright holder and year where known. Preserve existing
copyrights. Do not put this header on copied or derived upstream code without
first checking its license. For an upstream patch, retain the original
headers and record the modification and its provenance.

## Source repository and future binary distributions

This repository's licensing setup covers the source tree and its documented
exceptions. It is not a declaration that every locally downloaded dependency
has been cleared for redistribution. Cargo package license metadata describes
the package's own source; it does not replace native dependency licenses.

A binary release must include the notices of the components actually shipped,
including statically linked code (the ART exception links BoringSSL's
`libcrypto_static` into `dex2oat64`). A release that ships `libopenjdkjvm.so` or the native TimSort port
must also satisfy the corresponding-source conditions in
[licensing/OPENJDK.md](licensing/OPENJDK.md).

The original Android system image (ADR 0012) is not part of this repository.
Its files retain their individual terms; GMS/Google Play components in it are
not authorized for redistribution by this project's license. The image's
identity (sha256) establishes byte identity, not licensing clearance
([docs/gsi-base.md](docs/gsi-base.md) discusses a redistributable base).
