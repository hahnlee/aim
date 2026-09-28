# ICU version-coherence diagnostic

This standalone diagnostic compares the ICU base-data version embedded in the
actual `core-icu4j` class or DEX JAR with the linked ICU4C library and the
registered ICU common-data package. It also opens the minimum resource classes
used by the Java/native bridge: the `icuver` and root bundles, UTF-8 converter
data, root collation data, and root break-iterator data.

The gate is diagnostic-only. It does not rewrite JAR constants, substitute a
different `.dat`, or downgrade a mismatch to success. Exit status `0` means all
versions and resources agree, `2` means a coherent inspection found a version
mismatch, and `3` means inspection or a required resource failed.

From the repository root:

```sh
tools/icu-version-coherence/check.sh
```

Optional positional arguments select a different JAR, ICU foundation build
directory, data file, and ICU source root respectively. A DEX JAR requires
Android SDK `dexdump`; a class JAR requires `javap`. The native probe is compiled
locally and linked only against the selected Darwin ICU archives.

