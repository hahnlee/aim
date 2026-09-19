use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const PROVIDER_RECIPE: &str = "tools/build-bionic-runtime-provider-closure.sh";
// These are the paths named directly by the top-level production recipe. Keep
// this list explicit: a new recipe path must be reviewed here instead of
// being swept in by a broad tools/bionic-* directory walk.
const RECIPE_PATHS: &[&str] = &[
    "tools/build-android16-ftw.sh",
    "tools/build-android16-property-client.sh",
    "tools/bionic-strftime-facade/src/upstream_shim.h",
    "tools/bionic-strerror-facade/src/strerror.c",
    "tools/bionic-wide-stdio-facade/src/provider.cc",
    "tools/bionic-wide-stdio-facade/src/shims.c",
    "tools/bionic-wide-integer-facade/src/provider.c",
    "tools/bionic-wide-float-facade/src/provider.cc",
    "tools/bionic-swprintf-facade/src/provider.cc",
    "tools/bionic-swprintf-facade/src/aapcs64_entry.S",
    "tools/bionic-syslog-facade/src/syslog.cc",
    "tools/bionic-syslog-facade/src/aapcs64_entry.S",
    "tools/bionic-syscall-facade/src/syscall.cc",
    "tools/bionic-syscall-facade/src/aapcs64_entry.S",
    "tools/bionic-sendfile-facade/src/sendfile.cc",
    "tools/bionic-strftime-facade/src/provider.c",
    "tools/bionic-format-facade/src/format.cc",
    "tools/bionic-format-facade/src/aapcs64_entry.S",
    "tools/bionic-formatted-stdio-facade/src/provider.cc",
    "tools/bionic-formatted-stdio-facade/src/aapcs64_entry.S",
    "tools/bionic-ioctl-facade/src/ioctl.cc",
    "tools/bionic-ioctl-facade/src/aapcs64_entry.S",
    "tools/bionic-scanf-facade/src/scanf.cc",
    "tools/bionic-scanf-facade/src/aapcs64_entry.S",
    "tools/bionic-socket-broker-adapter/src/adapter.cc",
    "tools/bionic-socket-broker-adapter/src/android_scm_exports.cc",
    "tools/bionic-socket-broker-adapter/src/fd_inheritance.cc",
    "tools/bionic-socket-broker-adapter/src/retained_scm_export.cc",
    "tools/bionic-socket-broker-adapter/src/scm_endpoint_provider.cc",
    "tools/bionic-socket-broker-adapter/src/ancillary_intake.cc",
    "tools/bionic-socket-broker-adapter/src/scm_channel.cc",
    "tools/bionic-socket-broker-adapter/src/scm_guest_group.cc",
    "tools/bionic-socket-broker-adapter/src/scm_android_receive.cc",
    "tools/bionic-socket-broker-adapter/src/eventfd_owner.cc",
    "tools/bionic-socket-broker-adapter/src/sync_fence_merge.cc",
    "tools/bionic-socket-broker-adapter/src/sync_fence_broker.cc",
    "tools/bionic-socket-broker-adapter/src/fdsan.cc",
    "tools/bionic-socket-broker-adapter/src/fdsan_property.cc",
    "tools/bionic-socket-broker-adapter/src/fdsan_symbols.cc",
    "tools/bionic-dns-facade/src/dns.cc",
    "tools/bionic-locale-facade/src/provider.cc",
    "tools/bionic-numeric-facade/src/provider.c",
    "tools/bionic-math-facade/src/math.cc",
    "tools/bionic-abort-facade/src/provider.c",
    "tools/android-bionic-pthread-provider/src/provider.cc",
    "tools/android-dl-iterate-phdr-provider/src/provider.cc",
    "tools/bionic-central-fd-broker/src/fd_broker.cc",
    "tools/bionic-libc-leaf-facade/src/leaf.c",
    "tools/bionic-libc-allocator-facade/src/allocator.c",
    "tools/bionic-libc-allocator-facade/src/allocator_options.c",
    "tools/bionic-time-facade/src/shims.c",
    "tools/android-liblog-exec-provider/liblog_provider.cc",
    "tools/android-liblog-exec-provider/assert_abi.cc",
    "tools/android-liblog-exec-provider/aapcs64_log.S",
    "tools/bionic-provider-namespace/src/namespace.cc",
    "tools/bionic-provider-namespace/src/builtin_adapters.cc",
    "tools/bionic-strerror-facade/generated",
    "tools/bionic-provider-namespace/generated",
    "tools/bionic-float-conversion-facade/Cargo.toml",
    "tools/bionic-binary128-conversion-facade/Cargo.toml",
];

// The top-level recipe invokes these scripts in build mode. These are kept
// separate from RECIPE_PATHS because their paths are quoted by the nested
// script, not by the caller; validating the union catches a stale handoff.
const NESTED_RECIPE_PATHS: &[(&str, &[&str])] = &[
    (
        "tools/build-android16-ftw.sh",
        &[
            "tools/android16-ftw/sources.tsv",
            "tools/android16-ftw/ndk_declarations.h",
            "tools/android16-ftw/bindings.c",
            "tools/android16-ftw/resolver.c",
        ],
    ),
    (
        "tools/build-android16-property-client.sh",
        &[
            "tools/android16-property-client/sources.tsv",
            "tools/android16-property-client/bindings.c",
            "tools/android16-property-client/logging.c",
            "tools/build-android16-bionic-linker-config.sh",
            "tools/native-loader-policy/bionic_config_types.h",
        ],
    ),
    (
        "tools/build-android16-bionic-linker-config.sh",
        &[
            "upstream/android16-bionic-linker-config.sources",
            "patches/native-loader-policy/bionic-config-filesystem.patch",
            "patches/native-loader-policy/async-safe-darwin.patch",
            "tools/native-loader-policy/bionic_config_types.h",
            "compat/loader/warning_basename.h",
        ],
    ),
];

// Cargo build scripts compile these sibling sources by relative path. They
// are not Cargo path dependencies, so the manifest must name them explicitly.
const CARGO_BUILD_PATHS: &[&str] = &[
    "tools/bionic-errno-tls/src/errno_tls.c",
    "tools/bionic-syscall-facade/src/syscall.cc",
    "tools/bionic-syscall-facade/src/aapcs64_entry.S",
];

// The linker-config script expands these unit names into quoted
// `$root/compat/filesystem/$unit.cc` paths. They are explicit production
// inputs even though the shell variable prevents literal-path validation.
const NESTED_EXTRA_INPUTS: &[&str] = &[
    "compat/filesystem/guest_config.cc",
    "compat/filesystem/linker_config_fs.cc",
];

// The provider recipe compiles these non-Cargo translation units directly.
// Their local headers are real compiler inputs even though the shell recipe
// names only the .cc operands and include directories.
const PRODUCTION_OWNER_HEADERS: &[&str] = &[
    "tools/bionic-socket-broker-adapter/src/android_scm_exports.h",
    "tools/bionic-socket-broker-adapter/src/fd_inheritance.h",
    "tools/bionic-socket-broker-adapter/src/retained_scm_export.h",
    "tools/bionic-socket-broker-adapter/src/scm_endpoint_provider.h",
    "tools/bionic-socket-broker-adapter/src/scm_endpoint_lease.h",
    "tools/bionic-socket-broker-adapter/src/socket_endpoint_exports.h",
    "tools/bionic-socket-broker-adapter/src/ancillary_intake.h",
    "tools/bionic-socket-broker-adapter/src/scm_channel.h",
    "tools/bionic-socket-broker-adapter/src/scm_guest_group.h",
    "tools/bionic-socket-broker-adapter/src/scm_android_receive.h",
    "tools/bionic-socket-broker-adapter/src/eventfd_owner.h",
    "tools/bionic-socket-broker-adapter/src/fdsan.h",
    "tools/bionic-socket-broker-adapter/src/fdsan_symbols.h",
    "tools/bionic-socket-broker-adapter/src/sync_fence_broker.h",
    "tools/bionic-socket-broker-adapter/src/sync_fence_merge.h",
    "tools/bionic-socket-broker-adapter/src/unix_endpoints.h",
    "tools/bionic-socket-broker-adapter/src/unix_address.h",
    "compat/filesystem/guest_config.h",
    "compat/filesystem/guest_file.h",
    "compat/filesystem/linker_config_fs.h",
];

// These are literal tool paths in shell variables (rather than direct source
// operands) and therefore are checked by the reverse-reference audit below.
const STATIC_TOOL_REFERENCES: &[&str] = &[
    "tools/bionic-runtime-provider-closure",
    "tools/bionic-stat-facade/include",
];

const SUPPORT_DIRS: &[&str] = &[
    "tools/bionic-libc-leaf-facade/include",
    "tools/bionic-libc-allocator-facade/include",
    "tools/bionic-time-facade/include",
    "tools/android-bionic-pthread-provider/include",
    "tools/android-dl-iterate-phdr-provider/include",
    "tools/bionic-central-fd-broker/include",
    "tools/bionic-socket-broker-adapter/include",
    "tools/bionic-errno-tls/include",
    "tools/bionic-dns-facade/include",
    "tools/bionic-fs-facade/include",
    "tools/bionic-ioctl-facade/include",
    "tools/bionic-locale-facade/include",
    "tools/bionic-wide-stdio-facade/include",
    "tools/bionic-numeric-facade/include",
    "tools/bionic-scanf-facade/include",
    "tools/bionic-swprintf-facade/include",
    "tools/bionic-format-facade/include",
    "tools/bionic-formatted-stdio-facade/include",
    "tools/bionic-stdio-facade/include",
    "tools/bionic-syslog-facade/include",
    "tools/bionic-syscall-facade/include",
    "tools/bionic-strerror-facade/include",
    "tools/bionic-strerror-facade/generated",
    "tools/bionic-strftime-facade/include",
    "tools/bionic-wide-integer-facade/include",
    "tools/bionic-wide-float-facade/include",
    "tools/bionic-abort-facade/include",
    "tools/android-liblog-exec-provider/include",
    "tools/bionic-provider-namespace/include",
    "tools/bionic-provider-namespace/generated",
    "tools/bionic-math-facade/include",
    "tools/bionic-errno-tls/generated",
    "tools/bionic-float-conversion-facade/include",
    "tools/bionic-sendfile-facade/include",
];

const ACCEPTANCE_INPUTS: &[&str] = &[
    "tools/tests/bionic-runtime-provider-closure-acceptance.sh",
    "tools/tests/property-client-logging-test.sh",
    "tools/tests/numeric-provider-production-boundary.sh",
    "tools/bionic-runtime-provider-closure/full_link_smoke.cc",
    "tools/android16-ftw/traversal_smoke.cc",
    "tools/bionic-runtime-provider-closure/property_iteration_smoke.cc",
    "tools/bionic-runtime-provider-closure/signed_numeric_smoke.cc",
    "tools/bionic-runtime-provider-closure/mkdirat_smoke.cc",
    "tools/bionic-runtime-provider-closure/credentials_snapshot_smoke.cc",
    "tools/bionic-socket-broker-adapter/probes/unix_connect.cc",
    "tools/bionic-socket-broker-adapter/probes/fdsan.cc",
    "tools/android16-property-client/client_smoke.cc",
    "tools/android16-property-client/logging_test.c",
    "tools/bionic-process-state-facade/probes/configured_snapshot.cc",
    "tools/bionic-stdio-facade/probes/fortify_stream.cc",
    "tools/android-liblog-exec-provider/buf_print_call_test.S",
    "tools/android-liblog-exec-provider/assert_call_test.S",
    "tools/android-liblog-exec-provider/assert_call_test.cc",
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProviderInputManifest {
    pub(crate) production: Vec<PathBuf>,
    pub(crate) audit: Vec<PathBuf>,
}

pub(crate) fn collect(root: &Path) -> io::Result<ProviderInputManifest> {
    let root = root.canonicalize()?;
    let root = root.as_path();
    let recipe = read_recipe(root, PROVIDER_RECIPE)?;
    validate_recipe_text(&recipe, RECIPE_PATHS)?;
    let (exact_tool_paths, production_tool_dirs) = allowed_tool_paths();
    validate_quoted_tool_paths(&recipe, &exact_tool_paths, &production_tool_dirs)?;
    for (nested, required) in NESTED_RECIPE_PATHS {
        let text = read_recipe(root, nested)?;
        validate_recipe_text(&text, required)?;
        validate_quoted_tool_paths(&text, &exact_tool_paths, &production_tool_dirs)?;
    }

    let mut production = BTreeSet::new();
    add_existing(root, PROVIDER_RECIPE, &mut production);
    add_existing(
        root,
        "upstream/android35-libcxx-provider-coverage.lock",
        &mut production,
    );
    for path in RECIPE_PATHS {
        add_existing(root, path, &mut production);
    }
    for (_, paths) in NESTED_RECIPE_PATHS {
        for path in *paths {
            add_existing(root, path, &mut production);
        }
    }
    for path in NESTED_EXTRA_INPUTS {
        add_existing(root, path, &mut production);
    }
    for path in PRODUCTION_OWNER_HEADERS {
        add_existing(root, path, &mut production);
    }
    for directory in SUPPORT_DIRS {
        collect_support_dir(root, Path::new(directory), &mut production)?;
    }
    collect_existing_tree(root, Path::new("_aosp/android16-ftw"), &mut production)?;
    collect_existing_tree(
        root,
        Path::new("_aosp/android16-property-client"),
        &mut production,
    )?;
    collect_existing_tree(
        root,
        Path::new("_aosp/bionic-linker-config"),
        &mut production,
    )?;
    collect_existing_tree(
        root,
        Path::new("_aosp/android16-native-loader-policy"),
        &mut production,
    )?;
    collect_existing_tree(
        root,
        Path::new("_aosp/system/logging/liblog/include"),
        &mut production,
    )?;
    collect_existing_tree(
        root,
        Path::new("_aosp/system/libbase/include"),
        &mut production,
    )?;
    collect_existing_tree(
        root,
        Path::new("_aosp/bionic-float-conversion-facade/libc/upstream-openbsd/android/include"),
        &mut production,
    )?;
    collect_existing_tree(
        root,
        Path::new("_aosp/bionic-float-conversion-facade/libc/upstream-openbsd/lib/libc/gdtoa"),
        &mut production,
    )?;
    collect_existing_tree(
        root,
        Path::new("_aosp/bionic-binary128-conversion-facade/libc/upstream-openbsd/lib/libc/gdtoa"),
        &mut production,
    )?;
    add_existing(
        root,
        "_aosp/bionic-strftime-facade/platform/bionic/libc/tzcode/strftime.c",
        &mut production,
    );
    add_existing(
        root,
        "_aosp/bionic-strftime-facade/platform/bionic/libc/tzcode/private.h",
        &mut production,
    );
    add_existing(
        root,
        "_aosp/bionic-swprintf-facade/libc/upstream-openbsd/lib/libc/gdtoa/gdtoa.c",
        &mut production,
    );
    collect_existing_tree(
        root,
        Path::new("_aosp/external/icu-graphics/android_icu4c/include"),
        &mut production,
    )?;
    collect_existing_tree(
        root,
        Path::new("_aosp/external/icu-graphics/icu4c/source/common"),
        &mut production,
    )?;
    collect_existing_tree(
        root,
        Path::new("_aosp/external/icu-graphics/libandroidicuinit/include"),
        &mut production,
    )?;
    collect_cargo_reachable(root, &mut production)?;
    for path in CARGO_BUILD_PATHS {
        add_existing(root, path, &mut production);
    }

    let mut audit = BTreeSet::new();
    for path in ACCEPTANCE_INPUTS {
        add_existing(root, path, &mut audit);
    }
    // These are emitted by the two Cargo build scripts only for the explicit
    // audit feature. Keep them visible on the acceptance edge, never on the
    // production archive edge.
    for path in [
        "tools/bionic-float-conversion-facade/probes/host_state.cc",
        "tools/bionic-binary128-conversion-facade/probes/host_state.cc",
        "tools/bionic-libc-allocator-facade/src/allocator.c",
        "tools/bionic-libc-allocator-facade/src/allocator_options.c",
    ] {
        add_existing(root, path, &mut audit);
    }
    Ok(ProviderInputManifest {
        production: production.into_iter().collect(),
        audit: audit.into_iter().collect(),
    })
}

fn read_recipe(root: &Path, relative: &str) -> io::Result<String> {
    fs::read_to_string(root.join(relative))
}

fn add_existing(root: &Path, relative: &str, output: &mut BTreeSet<PathBuf>) {
    let path = root.join(relative);
    if path.is_file() {
        output.insert(PathBuf::from(relative));
    }
}

fn collect_support_dir(
    root: &Path,
    directory: &Path,
    output: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    if !root.join(directory).exists() {
        return Ok(());
    }
    collect_tree(root, directory, output, false)
}

fn collect_existing_tree(
    root: &Path,
    directory: &Path,
    output: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    if !root.join(directory).exists() {
        return Ok(());
    }
    collect_tree(root, directory, output, true)
}

fn collect_tree(
    root: &Path,
    directory: &Path,
    output: &mut BTreeSet<PathBuf>,
    reject_fixture: bool,
) -> io::Result<()> {
    for entry in fs::read_dir(root.join(directory))? {
        let path = entry?.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if path.is_dir() {
            if reject_fixture && has_fixture_component(relative) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "unexpected fixture directory in production owner: {}",
                        relative.display()
                    ),
                ));
            }
            collect_tree(root, relative, output, reject_fixture)?;
        } else if path.is_file() {
            if reject_fixture && has_fixture_component(relative) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "unexpected fixture input in production owner: {}",
                        relative.display()
                    ),
                ));
            }
            output.insert(relative.to_path_buf());
        }
    }
    Ok(())
}

fn has_fixture_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some("probes" | "tests" | "examples" | "fixtures")
        )
    })
}

fn collect_cargo_reachable(root: &Path, output: &mut BTreeSet<PathBuf>) -> io::Result<()> {
    let mut queue = VecDeque::from([
        PathBuf::from("tools/bionic-runtime-provider-closure/Cargo.toml"),
        PathBuf::from("tools/bionic-float-conversion-facade/Cargo.toml"),
        PathBuf::from("tools/bionic-binary128-conversion-facade/Cargo.toml"),
    ]);
    let mut visited = BTreeSet::new();
    add_existing(root, "Cargo.toml", output);
    add_existing(root, "Cargo.lock", output);
    while let Some(manifest) = queue.pop_front() {
        if !visited.insert(manifest.clone()) || !root.join(&manifest).is_file() {
            continue;
        }
        output.insert(manifest.clone());
        let package = manifest.parent().unwrap_or(Path::new("."));
        for sibling in [
            "Cargo.lock",
            "build.rs",
            "sources.lock",
            "upstream-sources.tsv",
        ] {
            let path = root.join(package).join(sibling);
            if path.is_file() {
                if let Ok(relative) = path.strip_prefix(root) {
                    output.insert(relative.to_path_buf());
                }
            }
        }
        collect_rust_sources(root, package, output)?;
        // A reachable build.rs may compile C/C++ sources and include headers
        // from these package-owned directories. Keep the owner boundary
        // explicit, while still rejecting fixture trees from production.
        for directory in ["include", "generated", "upstream"] {
            let directory = package.join(directory);
            if root.join(&directory).is_dir() {
                collect_tree(root, &directory, output, true)?;
            }
        }
        let text = fs::read_to_string(root.join(&manifest))?;
        for dependency in path_dependencies(&text) {
            let candidate = package.join(dependency);
            let dependency_manifest = if candidate
                .file_name()
                .is_some_and(|name| name == "Cargo.toml")
            {
                candidate
            } else {
                candidate.join("Cargo.toml")
            };
            if let Ok(canonical) = root.join(&dependency_manifest).canonicalize() {
                let relative = canonical.strip_prefix(root).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Cargo path dependency escaped repository",
                    )
                })?;
                queue.push_back(relative.to_path_buf());
            }
        }
    }
    Ok(())
}

fn collect_rust_sources(
    root: &Path,
    package: &Path,
    output: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    let src = package.join("src");
    if !root.join(&src).is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root.join(&src))? {
        let path = entry?.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if path.is_dir() {
            if relative.components().any(|component| {
                matches!(
                    component.as_os_str().to_str(),
                    Some("bin" | "tests" | "examples")
                )
            }) {
                continue;
            }
            collect_rust_tree(root, relative, output)?;
        } else if path.is_file() && path.file_name().is_none_or(|name| name != "main.rs") {
            output.insert(relative.to_path_buf());
        }
    }
    Ok(())
}

fn collect_rust_tree(
    root: &Path,
    directory: &Path,
    output: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    for entry in fs::read_dir(root.join(directory))? {
        let path = entry?.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if path.is_dir() {
            collect_rust_tree(root, relative, output)?;
        } else if path.is_file()
            && !has_fixture_component(relative)
            && path.file_name().is_none_or(|name| name != "main.rs")
        {
            output.insert(relative.to_path_buf());
        }
    }
    Ok(())
}

fn path_dependencies(manifest: &str) -> impl Iterator<Item = PathBuf> {
    let mut dependencies = Vec::new();
    let mut dependency_section = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed.trim_matches(['[', ']']);
            dependency_section = section == "dependencies"
                || section.ends_with(".dependencies")
                || section == "build-dependencies"
                || section.ends_with(".build-dependencies");
            continue;
        }
        if !dependency_section {
            continue;
        }
        let marker = "path = \"";
        let Some(start) = line.find(marker).map(|start| start + marker.len()) else {
            continue;
        };
        let Some(end) = line[start..].find('"').map(|end| end + start) else {
            continue;
        };
        dependencies.push(PathBuf::from(&line[start..end]));
    }
    dependencies.into_iter()
}

fn validate_recipe_text(recipe: &str, paths: &[&str]) -> io::Result<()> {
    for path in paths {
        let marker = format!("$root/{path}");
        if !recipe.contains(&marker) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("provider recipe no longer quotes required path: {marker}"),
            ));
        }
    }
    for line in recipe.lines() {
        if line.contains("$root/") && (line.contains("/probes/") || line.contains("_smoke")) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected probe/smoke read in production recipe: {line}"),
            ));
        }
    }
    Ok(())
}

fn allowed_tool_paths() -> (Vec<&'static str>, Vec<&'static str>) {
    let mut exact = Vec::new();
    exact.extend_from_slice(RECIPE_PATHS);
    exact.extend(CARGO_BUILD_PATHS.iter().copied());
    exact.extend(NESTED_EXTRA_INPUTS.iter().copied());
    exact.extend(STATIC_TOOL_REFERENCES.iter().copied());
    for (_, nested_paths) in NESTED_RECIPE_PATHS {
        exact.extend(nested_paths.iter().copied());
    }
    // Only these owner directories may authorize descendants. Audit inputs
    // are intentionally excluded: they are accepted solely at exact dispatch
    // call sites below, never as production source operands.
    (exact, SUPPORT_DIRS.to_vec())
}

fn validate_quoted_tool_paths(
    recipe: &str,
    exact_paths: &[&str],
    production_dirs: &[&str],
) -> io::Result<()> {
    for line in recipe.lines() {
        let mut remaining = line;
        while let Some(start) = remaining.find("$root/tools/") {
            let candidate = &remaining[start + "$root/".len()..];
            let end = candidate
                .find(|character: char| {
                    character.is_whitespace() || matches!(character, '"' | '\'' | '\\' | ')' | ';')
                })
                .unwrap_or(candidate.len());
            let path = &candidate[..end];
            // `$provider` is selected from the explicit provider list at
            // runtime; its include directory is covered by SUPPORT_DIRS.
            if !path.contains('$')
                && !exact_paths.contains(&path)
                && !production_dirs
                    .iter()
                    .any(|known| path == *known || path.starts_with(&format!("{known}/")))
                && !is_audit_dispatch(line, path)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unexpected unowned production tool input: $root/{path}"),
                ));
            }
            let advance = start + "$root/tools/".len();
            remaining = &remaining[advance..];
        }
    }
    Ok(())
}

fn is_audit_dispatch(line: &str, path: &str) -> bool {
    [
        "tools/tests/bionic-runtime-provider-closure-acceptance.sh",
        "tools/tests/property-client-logging-test.sh",
    ]
    .iter()
    .any(|dispatch| path == *dispatch && line.contains(&format!("bash \"$root/{dispatch}\"")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_requires_quoted_production_paths() {
        let recipe =
            r#"clang \"$root/tools/example/src/provider.cc\" \"$root/tools/example/include\""#;
        assert!(validate_recipe_text(recipe, &["tools/example/src/provider.cc"]).is_ok());
        assert!(validate_recipe_text(recipe, &["tools/example/src/missing.cc"]).is_err());
    }

    #[test]
    fn recipe_rejects_unexpected_probe_reads() {
        let recipe = r#"clang \"$root/tools/example/probes/smoke.cc\""#;
        assert!(validate_recipe_text(recipe, &[]).is_err());
    }

    #[test]
    fn recipe_rejects_unowned_quoted_tool_inputs() {
        let recipe = r#"clang \"$root/tools/example/src/new.cc\""#;
        assert!(validate_quoted_tool_paths(recipe, &["tools/example/src/old.cc"], &[]).is_err());
        let dynamic = r#"clang \"$root/tools/$provider/include\""#;
        assert!(validate_quoted_tool_paths(dynamic, &[], &[]).is_ok());
    }

    #[test]
    fn audit_dispatches_are_exact_but_fixture_sources_are_rejected() {
        let (exact, dirs) = allowed_tool_paths();
        let logging_compile =
            r#"clang -c \"$root/tools/android16-property-client/logging_test.c\""#;
        assert!(validate_quoted_tool_paths(logging_compile, &exact, &dirs).is_err());
        let provider_fixture =
            r#"clang -c \"$root/tools/bionic-runtime-provider-closure/fixture.cc\""#;
        assert!(validate_quoted_tool_paths(provider_fixture, &exact, &dirs).is_err());
        let dispatch =
            r#"--test-only) exec bash "$root/tools/tests/property-client-logging-test.sh" ;;"#;
        assert!(validate_quoted_tool_paths(dispatch, &exact, &dirs).is_ok());
    }

    #[test]
    fn cargo_path_dependencies_are_reachable() {
        let manifest = r#"
[lib]
path = "src/lib.rs"

[dependencies]
foo = { path = "../foo" }
bar = { path = "../../bar" }
"#;
        let paths: Vec<_> = path_dependencies(manifest).collect();
        assert_eq!(
            paths,
            vec![PathBuf::from("../foo"), PathBuf::from("../../bar")]
        );
    }

    #[test]
    fn actual_provider_recipe_has_a_conservative_manifest() {
        let root = option_env!("CARGO_MANIFEST_DIR")
            .map(|manifest| PathBuf::from(manifest).join("../.."))
            .unwrap_or_else(|| PathBuf::from("."));
        let manifest = collect(&root).expect("provider recipe input manifest");
        assert!(manifest.production.iter().any(|path| path
            == Path::new("_aosp/bionic-strftime-facade/platform/bionic/libc/tzcode/private.h")));
        assert!(
            manifest
                .production
                .iter()
                .any(|path| path == Path::new(PROVIDER_RECIPE))
        );
        assert!(
            manifest
                .audit
                .iter()
                .any(|path| path.ends_with("traversal_smoke.cc"))
        );
        for path in PRODUCTION_OWNER_HEADERS {
            assert!(
                manifest
                    .production
                    .iter()
                    .any(|candidate| candidate == Path::new(path)),
                "missing production owner header: {path}"
            );
        }
        assert!(
            !manifest
                .production
                .iter()
                .any(|path| has_fixture_component(path))
        );
    }

    #[test]
    fn acceptance_keeps_ftw_and_property_fixture_inputs_separate() {
        assert!(ACCEPTANCE_INPUTS.contains(&"tools/android16-ftw/traversal_smoke.cc"));
        assert!(ACCEPTANCE_INPUTS.contains(&"tools/android16-property-client/client_smoke.cc"));
        assert!(!RECIPE_PATHS.iter().any(|path| path.contains("/probes/")));
    }
}
