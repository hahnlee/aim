# Build-performance baseline

Measurement rules for existing build commands. Current developer commands and
inner-loop timings are in [build-system.md](build-system.md); native cache
contracts are in [native-build-graph.md](native-build-graph.md).

## Measure

From `darwin-art/`:

```sh
tools/measure-build-performance.sh --all
```

Default is `--cargo-check`; optional selectors are `--graphics-bootstrap` and
`--audit-graphics`. They invoke workspace check, runtime graphics bootstrap and
full graphics link audit respectively, with output-only verbosity. Use
`--keep-output` to retain raw temporary logs.

Record command, source/toolchain identity, warm/cold state, exit, `/usr/bin/time -p`
wall time and reported compile/cache signals. Signals are heuristic; fast exit
alone is not a cache hit. A no-op needs successful consecutive warm runs with
unchanged products and exact Ninja no-work where applicable. No wall-time
budget is established. Measurement does not delete build trees or change flags.

## Historical reference samples

These workloads and workspace shapes differ; do not compare them as equivalent
or treat them as fresh current-source acceptance.

| Date | State / invocation | Wall (s) |
| --- | --- | ---: |
| 2026-08-21 | Warm Cargo workspace check, original / consolidated workspace | 0.08 / 0.16 |
| 2026-08-21 | Warm Cargo graphics bootstrap | 4.06 |
| 2026-08-21 | Warm full / fast Cargo graphics link audit | 58.88 / 4.62 |
| 2026-08-21 | Warm native graph structural audit | 1.99 |
| 2026-08-21 | Warm direct CLI runtime / fast graphics link audit | 14.60 / 3.41 |
| 2026-08-21 | Warm incremental direct CLI graphics audit | 24.81 |
| 2026-08-29 | Warm Cargo build-foundation; source/archive mtimes stable, next graph no-work | 1.47 |

All rows exited 0. Link cost and serial full audits dominated these samples.
Use the fast/incremental inner loop and owning-provider audits appropriately;
validate phase-local invalidation before removing broad bootstrap stamps.
Past cache narratives and per-run logs live in Git history.
