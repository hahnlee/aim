# Contributing

## Licensing

Independent original contributions are submitted under Apache-2.0. Contributions
to upstream-derived files and patches must preserve the upstream license,
copyrights, notices, and applicable exceptions. In particular, retain the
Classpath exception for modifications to covered OpenJDK code; do not relabel
that code as Apache-2.0. See [LICENSING.md](LICENSING.md) and the
[OpenJDK modification inventory](licensing/OPENJDK.md).

Record the upstream project, exact revision, applicable license, and local
changes when importing or adapting code. Include required notices in the same
change. Do not remove headers to fit a file under the repository default.

## Commits

Use Conventional Commits for every commit:

```text
<type>(<optional-scope>): <imperative summary>
```

Common types are `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`,
`ci`, and `chore`. Keep each commit focused and buildable when practical.

Examples:

```text
feat(art): execute app dex through PathClassLoader
fix(darwin): preserve compressed references above PAGEZERO
perf(gpu): avoid framebuffer copies during presentation
docs(architecture): document the host graphics boundary
```

Generated outputs, downloaded AOSP trees, local Android system inputs, and
tool caches must stay out of Git. Revision locks and reproducible source/build
orchestration belong in Git.
