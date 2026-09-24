# Licensing

Original work contributed to Darwin ART is licensed under the
[Apache License, Version 2.0](LICENSE), unless a file or the exceptions below
specifies otherwise. Copyright remains with the respective contributors.
This grant covers our original contributions, not rights in third-party work.

## Third-party exceptions

The root license does not replace existing upstream licenses, copyright
notices, exceptions, or attribution requirements. This applies even when an
upstream file is modified, translated, embedded in a script, or renamed.

| Material | Applicable terms |
| --- | --- |
| OpenJDK-derived files and patches listed in [licensing/OPENJDK.md](licensing/OPENJDK.md) | GPL-2.0-only WITH Classpath-exception-2.0; preserve the exception for covered modifications |
| Other files in `darwin-art/patches/` | The terms of each patched upstream file; our changes to those files are provided under those terms |
| `darwin-art/upstream/freebsd-linuxulator/source/` | Original per-file FreeBSD licenses and the included `COPYRIGHT` |
| `darwin-art/upstream/freebsd-linuxulator/manifests/` | Generated reference data; preserve its FreeBSD provenance and the source notices |
| `darwin-art/tools/bionic-process-state-facade/upstream/get_device_api_level_inlines.h` | BSD-2-Clause, as stated in its retained header |
| Downloaded sources, tools, images, libraries, fonts, and generated derivatives | Their upstream terms; their presence in a build directory does not make them Apache-2.0 |
| License and notice documents | Reproduced under their original terms, not relicensed by the root license |

See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for attribution and
[licensing/upstream-sources.json](licensing/upstream-sources.json) for the
origins and hashes of the copied notice documents.

For an independent new source file, use:

```text
SPDX-License-Identifier: Apache-2.0
Copyright 2026 Darwin ART contributors
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

An app or runtime binary release must include the notices of the components
actually shipped, including statically linked code and archive contents.
OpenJDK binary releases must also satisfy the corresponding-source conditions
described in [licensing/OPENJDK.md](licensing/OPENJDK.md).

GMS/Google Play applications are not authorized for redistribution by this
project's license. Android files extracted from SDK system images retain their
individual terms. Their current locks establish byte identity, not licensing
clearance or correspondence to a complete source build. Customer app packaging
and replacement of these image-derived inputs are outside this source-only
licensing change.
