# Native build graph

[build-system.md](build-system.md) is the developer entry point. Cargo owns
Rust; `darwin-art-xtask` emits persistent Ninja edges for native objects,
archives and final links. `art-bootstrap` supplies locked cold-start builders.

## Commands and gate

From `darwin-art/`:

```sh
cargo run -p darwin-art-xtask -- native-graph --out _build/native-graph/build.ninja
_aosp/external/skia/third_party/ninja/ninja -f _build/native-graph/build.ninja -n graphics-bootstrap
./tools/audit-native-graph.sh
```

The structural audit checks runtime/GraphicsJNI/ICU TU promotion, depfiles,
separate narrow fixture edges, Rust final-link ownership and exact warm no-op.
Short wall time alone is not cache evidence.

## Cache and rebuild contract

- Missing/incomplete fingerprints or command stamps use canonical bootstrap
  edges. Complete objects promote to per-TU compile and deterministic archive
  edges. Legacy missing depfiles get a preprocessing-only scan before promotion.
- Stable `_build/runtime-*` outputs are shared by bootstrap and Ninja. Every
  object/archive is declared; deleted products rebuild. Depfiles capture
  transitive headers. Keys cover content, commands, compiler/SDK and includes.
- Patched ART/HWUI/libartbase shadow trees publish only byte changes. Strict
  upstream locks and tracked patches remain authoritative; ignored `_aosp`
  edits cannot become production inputs.
- Flavor-independent ART/compat objects share `_build/runtime-common/`.
  `darwin-art-build-contract` defines the common identity; changed identity
  disables promotion until canonical preparation rebuilds the matching cache.
  Graphics adapters remain flavor-local.
- Phase content stamps change only with source/header bytes, including when
  checkout preserves mtimes. Graph regeneration observes content identity.
- `target/release/libdarwin_art_runtime.a` is an explicit Cargo final-link
  dependency, not a prerequisite of C++ compilation. Rust ownership changes
  rebuild Rust and relink without recompiling unchanged ART/HWUI objects.
- `graphics-foundation` (`foundation` alias) owns HWUI and GraphicsJNI archive
  families; `icu-foundation` owns the four ICU archives. Historical inventory:
  458 ICU TUs (201 common, 254 i18n, stubdata, two init), GraphicsJNI objects plus
  registrar/force-load, and HWUI objects. Exact counts come from current graph.
- Keep production process/input/shutdown owners and test fixtures as distinct
  input sets. NativeBridge test staging uses disposable action roots while
  canonical compiled outputs remain shared and protected.

## Verification and remaining work

After changes, regenerate the graph, verify only owning objects/archives/link
edges invalidate, build, and require an exact repeat to report `no work to do`.
Check missing outputs, changed headers/patches, interrupted stamps and both
flavors; final symbol checks and app acceptance are separate gates.

Provider manifests cover reviewed recipes, not a generic proof of dynamic
shell/build.rs/Cargo/symlink input closure. Preserve fail-closed admission and
review actual reads when recipes change. Production closure and current app
failures are tracked in [architecture-migration.md](architecture-migration.md).
Historical compile timings and per-file extraction logs live in Git history;
measurement rules are in [build-performance-baseline.md](build-performance-baseline.md).
