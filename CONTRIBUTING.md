# Contributing

## Licensing

Independent original contributions are submitted under Apache-2.0. Contributions
to upstream-derived files and patches must preserve the upstream license,
copyrights, notices, and applicable exceptions. In particular, OpenJDK code
keeps GPLv2 with the Classpath exception; do not relabel it as Apache-2.0.
See [LICENSING.md](LICENSING.md) and [licensing/OPENJDK.md](licensing/OPENJDK.md).

Record the upstream project, exact revision, applicable license, and local
changes when importing or adapting code. Include required notices in the same
change. Do not remove headers to fit a file under the repository default.

## Building and testing

```sh
cargo aim build                 # the build graph (docs/build.md)
cargo aim test                  # unit tests
cargo aim test --integration    # everything, then every test with a timeout
```

`cargo aim status` says which nodes are stale and why. Do not add build
scripts: a new build step is a node in `crates/aim-build`, and a new HAL or
daemon crate needs no build edit at all (docs/build.md, "Where the graph
comes from"). Tests locate their inputs with `aim_paths`; under `cargo aim
test` a test that skips for a missing input fails.

## Tracking work

Open work is tracked in [GitHub issues](https://github.com/hahnlee/aim/issues),
not in repository documents. That covers bugs, missing Android contracts,
porting gates, follow-ups and investigations.

- File an issue for every gap found while working, including ones outside the
  current change. Record the symptom, how it was found (logs, transaction
  codes, reproduction), the expected AOSP behavior, the owning subsystem and
  acceptance criteria.
- Do not add TODO lists, gate checklists or backlog sections to Markdown files.
  `docs/boot-status.md` records the verified state and refers to issues by
  number for open failures.
- ADRs under `docs/adr/` record decisions. Link the issue that
  prompted the decision.
- Reference the issue in commits that resolve it (`Fixes #123` in the body),
  and close it only after the acceptance criteria pass.

## Commits

Use Conventional Commits for every commit:

```text
<type>(<optional-scope>): <imperative summary>
```

Common types are `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`,
`ci`, and `chore`. Keep each commit focused and buildable when practical.

Examples:

```text
feat(linux-abi): implement timerfd over kqueue
fix(binder): release a dead node's references on BC_DEAD_BINDER_DONE
perf(gpu): avoid a copy when presenting a composed frame
docs(adr): record the composer as a guest HAL
```

Generated outputs, downloaded AOSP trees, local Android system inputs, and
tool caches must stay out of Git. Revision locks and reproducible source/build
orchestration belong in Git.
