# ADR 0010: Device identity

Status: accepted (#10)

## Context

Apps and SDKs identify the device through `android.os.Build`, which reads
`ro.product.*`, `ro.build.*`, `ro.hardware` and `ro.soc.*`. Only
`ro.build.version.*` and the CPU ABI list were published, so `Build.MODEL`,
`Build.MANUFACTURER`, `Build.FINGERPRINT` and friends read `"unknown"`.
Analytics, anti-tamper and crash reporting (Unity's crash header, Adjust, the
Nexon SDK) read these values.

The values must identify this runtime honestly. It is not a phone, and it
must not claim to be one: impersonating a real Android device would make
SDKs apply that device's quirks and would misreport where the app runs.

## Decision

- **One pinned property file.** `runtime/framework/android-build.properties`
  is the runtime's `build.prop`. The host loads it into every process's
  property snapshot and the system image carries it as `/system/build.prop`,
  so reading the file and reading the property agree.
- **Build identity is the framework's.** The Java framework and services are
  the pinned PS16K image's, so `ro.build.id` (`BP22.250325.006`),
  `ro.build.version.incremental`, `ro.build.version.security_patch`,
  `ro.build.date(.utc)`, `ro.build.type` (`user`) and `ro.build.tags`
  (`dev-keys`) are that image's values.
- **Product identity is the Mac running the runtime.** Manufacturer and brand
  are `Apple`; device, name and product are `aim_darwin_arm64`; board and
  hardware are `darwin`. `ro.build.fingerprint` follows Build's composition:
  `Apple/aim_darwin_arm64/aim_darwin_arm64:16/BP22.250325.006/13344233:user/dev-keys`.
- **Host facts, not guesses.** `ro.product.model` is the Mac's `hw.model`
  (for example `Mac14,10`) and `ro.soc.model` its `machdep.cpu.brand_string`
  (`Apple M2 Pro`); `ro.soc.manufacturer` is `Apple`. They enter the snapshot
  beside the page size and time zone. A value the host cannot read is left
  unset rather than invented.

## Consequences

`Build.*` reads a coherent, truthful identity. Code that gates on a specific
phone model or brand treats this as an unknown device, which is correct.
Play Integrity and similar attestations still fail; this runtime has no
attested key (#34).
