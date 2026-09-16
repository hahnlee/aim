use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::{ninja_path, shell_quote};

#[derive(Clone)]
pub(crate) struct CachedNativeObject {
    pub(crate) object: PathBuf,
    pub(crate) source: PathBuf,
    pub(crate) command: String,
    pub(crate) shell_quoted: bool,
}

fn dependency_scan_command(object: &CachedNativeObject, depfile: &Path) -> String {
    let command_tokens = object
        .command
        .split_whitespace()
        // `-E` is a driver-only preprocessing action; the original `-c`
        // must be removed or Clang's `-Werror` rejects the scan for an
        // unused compilation argument. The source path remains positional
        // and is still covered by the depfile.
        .filter(|token| *token != "-c")
        .collect::<Vec<_>>();
    // Foundation command files already contain shell escaping (`\(…\)` and
    // `\"…\"`). Re-quoting those tokens would turn valid compiler arguments
    // into literal backslashes during the one-time dependency scan. Runtime
    // fingerprints contain raw argv diagnostics and still need per-token
    // quoting.
    let quoted_command = if object.shell_quoted {
        command_tokens.join(" ")
    } else {
        command_tokens
            .into_iter()
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" ")
    };
    // The depfile must be newer than the source after the one-time scan.
    // Copying the object's timestamp is incorrect when a checkout restored
    // the source with a newer mtime, so a plain touch records the successful
    // scan at the current time and leaves the object untouched.
    format!(
        "{quoted_command} -E -Wno-unused-command-line-argument -MMD -MF {} -o /dev/null && touch {}",
        shell_quote(&depfile.to_string_lossy()),
        shell_quote(&depfile.to_string_lossy()),
    )
}

fn depfile_dependencies(depfile: &Path) -> io::Result<Vec<PathBuf>> {
    let contents = fs::read_to_string(depfile)?;
    let flattened = contents.replace("\\\n", " ");
    let Some((_, prerequisites)) = flattened.split_once(':') else {
        return Ok(Vec::new());
    };
    let mut dependencies = Vec::new();
    let mut token = String::new();
    let mut escaped = false;
    for character in prerequisites.chars() {
        if escaped {
            token.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character.is_whitespace() {
            if !token.is_empty() {
                dependencies.push(PathBuf::from(std::mem::take(&mut token)));
            }
        } else {
            token.push(character);
        }
    }
    if escaped {
        token.push('\\');
    }
    if !token.is_empty() {
        dependencies.push(PathBuf::from(token));
    }
    Ok(dependencies)
}

/// Recover the compiler command and source identity persisted by
/// `compile_with_dependency_cache`. This lets a later graph generation turn
/// an already materialized bootstrap into real per-object Ninja edges without
/// duplicating ART's long include/define command construction in xtask.
/// Recover one archive from a flavor-local adapter directory plus the shared
/// ART core object directory.  The archive member list remains authoritative,
/// so stale objects that are no longer members cannot leak into the Ninja
/// graph.
fn matches_required_source(source: &Path, required: &str) -> bool {
    // Adapter paths are relative to compat; runtime modules start with ../.
    // Compare the subsystem path, not the basename: app/process_entry.cc must
    // never satisfy a missing system/process_entry.cc producer.
    source.ends_with(required.strip_prefix("../").unwrap_or(required))
}

pub(crate) fn cached_native_objects_from_dirs(
    object_dirs: &[&Path],
    archive: &Path,
    required_sources: &[&str],
) -> io::Result<Option<Vec<CachedNativeObject>>> {
    let mut by_name = BTreeMap::new();
    for object_dir in object_dirs {
        let Ok(entries) = fs::read_dir(object_dir) else {
            return Ok(None);
        };
        for entry in entries.flatten() {
            let fingerprint = entry.path();
            if fingerprint
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("fingerprint")
            {
                continue;
            }
            let object = fingerprint.with_extension("");
            // A Ninja-produced depfile may not exist yet (Ninja can be starting
            // from a persisted Rust fingerprint), but the object and its command
            // fingerprint are still sufficient to seed the cached graph.  The
            // first cached edge will regenerate the depfile; requiring it here
            // would incorrectly fall back to the monolithic Rust builder.
            if !object.is_file() {
                continue;
            }
            let contents = fs::read_to_string(&fingerprint)?;
            let Some(command_line) = contents
                .lines()
                .find_map(|line| line.strip_prefix("command="))
            else {
                continue;
            };
            let tokens = command_line.split_whitespace().collect::<Vec<_>>();
            let Some(source_index) = tokens.iter().position(|token| *token == "-c") else {
                continue;
            };
            let Some(source) = tokens.get(source_index + 1).map(PathBuf::from) else {
                continue;
            };
            if !source.is_file() {
                continue;
            }
            let Some(name) = object.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            // The shared ART core directory is visited first.  Preserve its
            // object when the flavor directory still contains a pre-split
            // copy with the same member name.
            by_name
                .entry(name.to_owned())
                .or_insert(CachedNativeObject {
                    object,
                    source,
                    command: command_line.to_owned(),
                    shell_quoted: false,
                });
        }
    }
    if required_sources.iter().any(|required| {
        !by_name
            .values()
            .any(|object| matches_required_source(&object.source, required))
    }) {
        // A source-list change must not be hidden by an older archive cache.
        // Let the canonical builder materialize the missing TU and its
        // fingerprint before promoting the archive back to Ninja edges.
        return Ok(None);
    }
    // The bootstrap archives are deliberately large (200+ objects).  A
    // partially interrupted Rust builder can leave a handful of fingerprints
    // and a seemingly valid archive; do not promote that into a native graph.
    // Falling back to the canonical builder below is safer than silently
    // linking an incomplete ART runtime.
    if by_name.len() < 128 || !archive.is_file() {
        return Ok(None);
    }
    let archive_members = Command::new("ar").arg("-t").arg(archive).output()?;
    if !archive_members.status.success() {
        return Ok(None);
    }
    let mut ordered = Vec::new();
    for member in String::from_utf8_lossy(&archive_members.stdout).lines() {
        if member == "__.SYMDEF" || member == "__.SYMDEF SORTED" {
            continue;
        }
        if let Some(object) = by_name.remove(member.trim()) {
            ordered.push(object);
        }
    }
    // An interrupted archive command can leave a valid, but incomplete,
    // archive behind.  Never let that partial member order become the cache
    // manifest: append every remaining fingerprint object in stable name
    // order so the next graph rebuilds a complete archive.
    ordered.extend(by_name.into_values());
    if ordered.len() < 128 {
        return Ok(None);
    }
    Ok(Some(ordered))
}

pub(crate) fn emit_cached_native_graph(
    graph: &mut String,
    objects: &[CachedNativeObject],
    archive: &str,
    rules_emitted: &mut bool,
) {
    emit_cached_native_graph_with_inputs(graph, objects, archive, rules_emitted, &[], &[]);
}

pub(crate) fn emit_cached_native_graph_with_inputs(
    graph: &mut String,
    objects: &[CachedNativeObject],
    archive: &str,
    rules_emitted: &mut bool,
    extra_archive_inputs: &[PathBuf],
    object_order_only_inputs: &[PathBuf],
) {
    let mut object_paths = emit_cached_native_object_edges_with_order_only(
        graph,
        objects,
        rules_emitted,
        object_order_only_inputs,
    );
    object_paths.extend(extra_archive_inputs.iter().map(|path| ninja_path(path)));
    graph.push_str("build ");
    graph.push_str(archive);
    graph.push_str(": native_cached_archive ");
    graph.push_str(&object_paths.join(" "));
    graph.push('\n');
}

/// Emit source-owning edges without creating an archive. Shared runtime-core
/// objects are members of both the headless and graphics archives, but each
/// object must have exactly one Ninja producer so edits invalidate both
/// consumers without duplicate output declarations.
pub(crate) fn emit_cached_native_object_edges_with_order_only(
    graph: &mut String,
    objects: &[CachedNativeObject],
    rules_emitted: &mut bool,
    order_only_inputs: &[PathBuf],
) -> Vec<String> {
    if !*rules_emitted {
        graph.push_str("rule native_cached_cpp\n");
        graph.push_str("  command = $compile_command -MMD -MF $out.d\n");
        graph.push_str("  depfile = $out.d\n");
        graph.push_str("  deps = gcc\n");
        graph.push_str("  description = CXX $out\n");
        graph.push_str("  restat = 1\n\n");
        // Older canonical builders persisted command/fingerprint metadata but
        // did not emit depfiles.  Treat those objects as valid cache entries
        // instead of making Ninja rebuild the entire archive merely because
        // metadata is missing.  Once such a TU is rebuilt, the normal rule
        // above writes a depfile and the next graph generation promotes it to
        // dependency-aware scheduling.
        graph.push_str("rule native_cached_cpp_legacy\n");
        graph.push_str("  command = $compile_command -MMD -MF $out.d\n");
        graph.push_str("  description = CXX $out (legacy cache)\n");
        graph.push_str("  restat = 1\n\n");
        // A dependency scan performed outside Ninja cannot populate
        // `.ninja_deps`.  Promote such an object with the scanned headers as
        // explicit inputs so graph regeneration does not rebuild the whole
        // ART archive merely to teach Ninja dependencies it already has.
        graph.push_str("rule native_cached_cpp_promoted\n");
        graph.push_str("  command = $compile_command -MMD -MF $out.d\n");
        graph.push_str("  description = CXX $out\n");
        graph.push_str("  restat = 1\n\n");
        graph.push_str("rule native_dependency_scan\n");
        graph.push_str("  command = $dependency_scan_command\n");
        graph.push_str("  description = DEP $out\n");
        graph.push_str("  restat = 1\n\n");
        graph.push_str("rule native_cached_archive\n");
        graph.push_str("  command = rm -f $out && ar rcs $out $in\n");
        graph.push_str("  description = AR $out\n");
        graph.push_str("  restat = 1\n\n");
        *rules_emitted = true;
    }
    let mut object_paths = Vec::with_capacity(objects.len());
    for object in objects {
        let output = ninja_path(&object.object);
        let source = ninja_path(&object.source);
        // The canonical builder names depfiles `<source>.o.d`; using
        // `with_extension("d")` silently looked for `<source>.d` and kept
        // every promoted object on the legacy rule forever.
        let depfile = object.object.with_extension("o.d");
        let needs_dependency_scan = !depfile.is_file();
        if needs_dependency_scan {
            // Older cache entries contain the exact compile command and a
            // fingerprint, but not the compiler-generated dependency file.
            // Seed that missing metadata with a preprocessing-only edge.  It
            // is order-only for the object, so the one-time scan does not
            // force a needless recompilation; the next Ninja invocation then
            // consumes the depfile through the normal `deps = gcc` rule.
            let dependency_scan_command = dependency_scan_command(object, &depfile);
            graph.push_str("build ");
            graph.push_str(&ninja_path(&depfile));
            graph.push_str(": native_dependency_scan ");
            graph.push_str(&source);
            graph.push('\n');
            graph.push_str("  dependency_scan_command = ");
            graph.push_str(&dependency_scan_command.replace('$', "$$"));
            graph.push('\n');
        }
        graph.push_str("build ");
        graph.push_str(&output);
        let promoted_dependencies = if depfile.is_file() {
            depfile_dependencies(&depfile).unwrap_or_default()
        } else {
            Vec::new()
        };
        graph.push_str(if !promoted_dependencies.is_empty() {
            ": native_cached_cpp_promoted "
        } else {
            ": native_cached_cpp_legacy "
        });
        graph.push_str(&source);
        if !promoted_dependencies.is_empty() {
            graph.push_str(" | ");
            graph.push_str(
                &promoted_dependencies
                    .iter()
                    .filter(|dependency| **dependency != object.source)
                    .map(|dependency| ninja_path(dependency))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        let mut order_only = order_only_inputs
            .iter()
            .map(|input| ninja_path(input))
            .collect::<Vec<_>>();
        if needs_dependency_scan {
            order_only.push(ninja_path(&depfile));
        }
        if !order_only.is_empty() {
            graph.push_str(" || ");
            graph.push_str(&order_only.join(" "));
        }
        graph.push('\n');
        graph.push_str("  compile_command = ");
        // Fingerprints store a diagnostic, whitespace-separated command.  It
        // is sufficient to recover the persisted argv here because the
        // bootstrap paths contain no spaces; every token still needs shell
        // quoting (notably ART defines containing parentheses or quotes).
        let quoted_command = if object.shell_quoted {
            object.command.clone()
        } else {
            object
                .command
                .split_whitespace()
                .map(shell_quote)
                .collect::<Vec<_>>()
                .join(" ")
        };
        graph.push_str(&quoted_command.replace('$', "$$"));
        graph.push('\n');
        object_paths.push(output);
    }
    object_paths
}

#[cfg(test)]
mod tests {
    use super::{
        CachedNativeObject, dependency_scan_command, depfile_dependencies,
        emit_cached_native_object_edges_with_order_only,
    };
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn required_source_retains_subsystem_identity() {
        let required = "../runtime/framework/system/process_entry.cc";
        assert!(!super::matches_required_source(
            PathBuf::from("/repo/compat/../runtime/framework/app/process_entry.cc").as_path(),
            required,
        ));
        assert!(super::matches_required_source(
            PathBuf::from("/repo/compat/../runtime/framework/system/process_entry.cc").as_path(),
            required,
        ));
        assert!(super::matches_required_source(
            PathBuf::from("/repo/compat/binder/service_endpoint.cc").as_path(),
            "binder/service_endpoint.cc",
        ));
    }

    fn object(command: &str, shell_quoted: bool) -> CachedNativeObject {
        CachedNativeObject {
            object: PathBuf::from("/tmp/object.o"),
            source: PathBuf::from("/tmp/source.cc"),
            command: command.to_owned(),
            shell_quoted,
        }
    }

    #[test]
    fn parses_continued_and_escaped_depfile_paths() {
        let path = std::env::temp_dir().join(format!(
            "darwin-art-depfile-{}-{}.d",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(
            &path,
            "object.o: source.cc \\\n+ include/header.h path\\ with\\ spaces/header.h\n"
                .replace("\n+ ", "\n "),
        )
        .unwrap();
        let dependencies = depfile_dependencies(&path).unwrap();
        assert_eq!(
            dependencies,
            vec![
                PathBuf::from("source.cc"),
                PathBuf::from("include/header.h"),
                PathBuf::from("path with spaces/header.h")
            ]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn dependency_scan_preserves_preescaped_foundation_arguments() {
        let command = r#"clang++ -D__INTRODUCED_IN\(n\)= -DSK_USER_CONFIG_HEADER=\"include/config/SkUserConfigManual.h\" -c /tmp/source.cc -o /tmp/object.o"#;
        let scan = dependency_scan_command(
            &object(command, true),
            PathBuf::from("/tmp/object.o.d").as_path(),
        );
        assert!(scan.contains(r#"-D__INTRODUCED_IN\(n\)="#));
        assert!(
            scan.contains(r#"-DSK_USER_CONFIG_HEADER=\"include/config/SkUserConfigManual.h\""#)
        );
        assert!(!scan.contains(r#"'-D__INTRODUCED_IN"#));
        assert!(!scan.contains(" -c "));
    }

    #[test]
    fn dependency_scan_quotes_raw_runtime_arguments() {
        let command =
            "clang++ -DART_BASE_ADDRESS_MIN_DELTA=(-0x1000000) -c /tmp/source.cc -o /tmp/object.o";
        let scan = dependency_scan_command(
            &object(command, false),
            PathBuf::from("/tmp/object.o.d").as_path(),
        );
        assert!(scan.contains("'-DART_BASE_ADDRESS_MIN_DELTA=(-0x1000000)'"));
        assert!(!scan.contains(" -c "));
    }

    #[test]
    fn cached_object_waits_for_shared_shadow_publication() {
        let id = format!("{}", std::process::id());
        let cached = CachedNativeObject {
            object: PathBuf::from(format!("/tmp/darwin-art-{id}-object.o")),
            source: PathBuf::from(format!("/tmp/darwin-art-{id}-source.cc")),
            command: format!(
                "clang++ -c /tmp/darwin-art-{id}-source.cc -o /tmp/darwin-art-{id}-object.o"
            ),
            shell_quoted: false,
        };
        let marker = PathBuf::from(format!("/tmp/darwin-art-{id}-shadow-ready"));
        let mut graph = String::new();
        let mut rules = false;
        emit_cached_native_object_edges_with_order_only(
            &mut graph,
            &[cached],
            &mut rules,
            std::slice::from_ref(&marker),
        );
        assert!(
            graph.lines().any(|line| {
                line.starts_with("build /tmp/darwin-art-")
                    && line.contains("-object.o: native_cached_cpp")
                    && line.contains(&format!("|| {}", marker.display()))
            }),
            "cached object edge lacks the shared-shadow order-only barrier: {graph}"
        );
    }
}
