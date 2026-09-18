#![forbid(unsafe_code)]

use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

mod dev;
mod graph;

use darwin_art_build_contract::RUNTIME_CACHE_IDENTITY;
use graph::emit::emit_graph;

const GRAPHICS_BOOTSTRAP_ARCHIVE: &str =
    "runtime-graphics-bootstrap/libart-runtime-graphics-bootstrap-darwin.a";
const RUNTIME_BOOTSTRAP_ARCHIVE: &str = "runtime-bootstrap/libart-runtime-bootstrap-darwin.a";
const HWUI_STATIC_FOUNDATION_ARCHIVE: &str = "hwui-static-foundation/libhwui-static-darwin.a";
const HWUI_APEX_FOUNDATION_ARCHIVE: &str =
    "hwui-static-foundation/libandroid-graphics-apex-common-darwin.a";
const ANDROID_GRAPHICS_JNI_ARCHIVE: &str = "android-graphics-jni/libandroid-graphics-jni-darwin.a";
const ANDROID_GRAPHICS_REGISTRAR_ARCHIVE: &str =
    "android-graphics-jni/libandroid-graphics-layoutlib-registrar-darwin.a";
const ANDROID_GRAPHICS_FORCE_LOADED_OBJECT: &str =
    "android-graphics-jni/android-graphics-jni-force-loaded.o";
const ICU_COMMON_FOUNDATION_ARCHIVE: &str = "icu-foundation/libicuuc-common-darwin.a";
const ICU_I18N_FOUNDATION_ARCHIVE: &str = "icu-foundation/libicui18n-darwin.a";
const ICU_STUBDATA_FOUNDATION_ARCHIVE: &str = "icu-foundation/libicuuc-stubdata-darwin.a";
const ICU_INIT_FOUNDATION_ARCHIVE: &str = "icu-foundation/libandroidicuinit-darwin.a";
const GRAPHICS_RUNTIME_LIBRARY: &str =
    "runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib";
const HEADLESS_RUNTIME_LIBRARY: &str = "runtime-link-probe/libdarwin_art_runtime.dylib";

const SDK_NAME: &str = "macosx";

const CXX_FLAGS: &[&str] = &[
    "-std=c++20",
    "-fPIC",
    "-Wall",
    "-Wextra",
    "-DDARWIN_ART_REAL_GRAPHICS",
    "-DDARWIN_ART_HWUI_GPU",
    "-DSK_BUILD_FOR_ANDROID_FRAMEWORK",
];

fn main() {
    if let Err(error) = run() {
        eprintln!("darwin-art-xtask: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("dev") => dev::run(args),
        Some("native-graph") => {
            let mut out = None;
            let mut with_support = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--out" => out = args.next().map(PathBuf::from),
                    "--with-support" => with_support = true,
                    value => return Err(format!("unknown native-graph option: {value}")),
                }
            }
            let out = out.ok_or_else(|| "native-graph requires --out <path>".to_owned())?;
            if with_support {
                graph::emit::emit_graph_with_support(&out, true)
            } else {
                emit_graph(&out)
            }
            .map_err(|error| error.to_string())
        }
        _ => Err("usage: cargo run -p darwin-art-xtask -- native-graph --out <path>".to_owned()),
    }
}

fn repository_root(out: &Path) -> PathBuf {
    let start = if out.is_absolute() {
        out.parent().unwrap_or(out).to_path_buf()
    } else {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };

    if let Some(root) = start
        .ancestors()
        .find(|candidate| candidate.join("Cargo.toml").is_file())
    {
        return root.to_path_buf();
    }
    // An absolute output path outside the checkout is useful for CI scratch
    // graphs. Fall back to the invocation directory before accepting an
    // unrelated path as the repository root.
    env::current_dir()
        .ok()
        .and_then(|current| {
            current
                .ancestors()
                .find(|candidate| candidate.join("Cargo.toml").is_file())
                .map(Path::to_path_buf)
        })
        .unwrap_or(start)
}

fn digest_inputs(root: &Path, inputs: &[PathBuf]) -> io::Result<String> {
    let mut digest = Sha256::new();
    digest.update(graph::GRAPH_VERSION.as_bytes());
    digest.update([0]);
    digest.update(RUNTIME_CACHE_IDENTITY.as_bytes());
    for path in inputs {
        digest.update(path.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(fs::read(root.join(path))?);
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn ninja_path(path: &Path) -> String {
    path.to_string_lossy().replace('$', "$$").replace(' ', "$ ")
}

fn shell_quote(path: &str) -> String {
    if path
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_./-".contains(&byte))
    {
        path.to_owned()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::emit::{emit_graph, interpreter_core_inputs};
    use crate::graph::foundation::{FoundationFamily, is_foundation_family_input};
    use crate::graph::inputs::{
        graph_inputs, is_global_digest_excluded, native_owner_content_stamp, probe_content_stamp,
    };
    use crate::graph::provider_inputs;

    #[test]
    fn shell_quote_is_stable_for_normal_repository_paths() {
        assert_eq!(shell_quote("/tmp/darwin-art"), "/tmp/darwin-art");
        assert_eq!(shell_quote("/tmp/with space"), "'/tmp/with space'");
    }

    #[test]
    fn ninja_path_escapes_ninja_metacharacters() {
        assert_eq!(ninja_path(Path::new("a b/$c")), "a$ b/$$c");
    }

    #[test]
    fn graphics_bootstrap_archive_is_declared_at_stable_output_path() {
        let output_root = Path::new("_build");
        assert_eq!(
            output_root.join(GRAPHICS_BOOTSTRAP_ARCHIVE),
            PathBuf::from(
                "_build/runtime-graphics-bootstrap/\
                 libart-runtime-graphics-bootstrap-darwin.a"
            )
        );
    }

    #[test]
    fn native_graph_digest_excludes_rust_orchestration() {
        let paths = graph_inputs(Path::new("."));
        for excluded in [
            "Cargo.toml",
            "Cargo.lock",
            "crates/art-bootstrap/Cargo.toml",
            "crates/art-bootstrap/src/main.rs",
            "crates/art-bootstrap/src/build_context.rs",
            "crates/art-bootstrap/src/help.rs",
            "crates/darwin-art-elf-loader/Cargo.toml",
            "crates/darwin-art-xtask/Cargo.toml",
            "crates/darwin-art-xtask/src/main.rs",
        ] {
            assert!(
                !paths.iter().any(|path| path == Path::new(excluded)),
                "Rust orchestration path leaked into native graph digest: {excluded}"
            );
        }
    }

    #[test]
    fn libcore_linux_archive_tracks_each_native_phase_input() {
        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let paths = graph_inputs(&repository_root);
        for input in [
            "tools/build-android16-libcore-darwin-linux.sh",
            "upstream/android16-libcore-darwin-linux.lock",
            "compat/libcore_darwin_linux.cc",
            "compat/libcore_darwin_linux_system_natives.cc",
            "compat/libcore_darwin_linux_syscalls.cc",
            "compat/libcore_darwin_linux.h",
        ] {
            assert!(
                paths.iter().any(|path| path == Path::new(input)),
                "libcore archive input is missing from native graph: {input}"
            );
        }
    }

    #[test]
    fn unix_filesystem_archive_and_managed_audit_have_separate_inputs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = root.join(format!(
            "_build/unixfs-boundary-{}.ninja",
            std::process::id()
        ));
        emit_graph(&output).expect("emit native graph");
        let graph = fs::read_to_string(&output).unwrap();
        let archive = graph
            .lines()
            .find(|line| line.contains(": unix_filesystem_archive "))
            .unwrap();
        assert!(!archive.contains("/probes/"));
        assert!(!archive.contains("test-android16-unix-filesystem-darwin.sh"));
        assert!(graph.contains("build-android16-unix-filesystem-darwin.sh --archive-only"));
        let audit = graph
            .lines()
            .find(|line| line.contains(": unix_filesystem_managed_test "))
            .unwrap();
        for input in [
            "libopenjdk-unix-filesystem-darwin.a",
            "test-android16-unix-filesystem-darwin.sh",
            "probes/android16_unix_filesystem_jni.c",
            "probes/unix-filesystem/UnixFileSystemDarwinSmoke.java",
        ] {
            assert!(audit.contains(input), "managed audit omitted {input}");
        }
        fs::remove_file(output).unwrap();
    }

    #[test]
    fn async_close_audit_is_not_a_libcore_archive_dependency() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = root.join(format!(
            "_build/async-boundary-{}.ninja",
            std::process::id()
        ));
        emit_graph(&output).expect("emit native graph");
        let graph = fs::read_to_string(&output).unwrap();
        let archive = graph
            .lines()
            .find(|line| line.contains(": libcore_linux_archive "))
            .unwrap();
        assert!(!archive.contains("android16_asynchronous_close_monitor_smoke.cc"));
        assert!(!archive.contains("android16_asynchronous_close_monitor_jni.cc"));
        assert!(!archive.contains("android16_libcore_darwin_linux_smoke.cc"));
        for output in [
            "darwin_linux_method_table.inc",
            "libcore-darwin-linux-methods.tsv",
            "libandroid-system-os-constants-darwin.a",
        ] {
            assert!(
                archive.contains(output),
                "libcore producer omitted {output}"
            );
        }
        assert!(graph.contains("build-android16-libcore-darwin-linux.sh --archive-only"));
        assert!(!archive.contains("android16_os_constants_jni.cc"));
        assert!(!archive.contains("probes/"));
        let os_audit = graph
            .lines()
            .find(|line| line.contains(": os_constants_test "))
            .unwrap();
        for input in [
            "test-android16-os-constants-darwin.sh",
            "android16_os_constants_jni.cc",
            "libandroid-system-os-constants-darwin.a",
            "android16_os_constants_values.inc",
            "derived-values.tsv",
        ] {
            assert!(
                os_audit.contains(input),
                "OsConstants audit omitted {input}"
            );
        }
        let libcore_audit = graph
            .lines()
            .find(|line| line.contains(": libcore_linux_test "))
            .unwrap();
        for input in [
            "libcore-darwin-linux.a",
            "darwin_linux_method_table.inc",
            "libcore-darwin-linux-methods.tsv",
            "test-android16-libcore-darwin-linux.sh",
            "android16_libcore_darwin_linux_smoke.cc",
        ] {
            assert!(
                libcore_audit.contains(input),
                "libcore audit omitted {input}"
            );
        }
        let audit = graph
            .lines()
            .find(|line| line.contains(": async_close_test "))
            .unwrap();
        for input in [
            "libandroidio-darwin.a",
            "libcore-io-asynchronous-close-monitor-registrar-darwin.a",
            "test-android16-asynchronous-close-monitor.sh",
            "android16_asynchronous_close_monitor_smoke.cc",
            "android16_asynchronous_close_monitor_jni.cc",
        ] {
            assert!(audit.contains(input), "async audit omitted {input}");
        }
        fs::remove_file(output).unwrap();
    }

    #[test]
    fn surfaceflinger_edge_tracks_commit_wait_boundary() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = root.join(format!(
            "_build/commit-wait-graph-{}.ninja",
            std::process::id()
        ));
        emit_graph(&output).expect("emit native graph");
        let graph = fs::read_to_string(&output).unwrap();
        let edge = graph
            .lines()
            .find(|line| line.contains(": surfaceflinger_core "))
            .unwrap();
        for input in [
            "tools/lib/surfaceflinger-compile-flags.sh",
            "patches/frameworks-native/0002-darwin-surface-commit-wait.patch",
            "patches/frameworks-native/0003-buffer-release-message-length.patch",
            "patches/frameworks-native/0004-darwin-release-record-transport.patch",
            "patches/frameworks-native/0005-darwin-binder-trigger-poll.patch",
            "patches/frameworks-native/0006-darwin-binder-rpc-peer-identity.patch",
            "patches/frameworks-native/0007-rpc-binder-death-log-handle.patch",
            "patches/frameworks-native/0008-darwin-binder-platform-syscalls.patch",
            "patches/frameworks-native/0009-darwin-binder-fd-namespace.patch",
            "tools/sync-android16-hostgraphics.sh",
            "compat/surfaceflinger/release_record_transport.cc",
            "compat/surfaceflinger/release_record_transport.h",
            "compat/surfaceflinger/commit_signal.h",
        ] {
            assert!(edge.contains(input), "missing SF producer input: {input}");
        }
        assert!(!edge.contains("/probes/"));
        assert!(!edge.contains("/tools/tests/"));
        assert!(!edge.contains("surfaceflinger-transaction-runtime"));
        let audit = graph
            .lines()
            .find(|line| line.contains(": surfaceflinger_core_test "))
            .unwrap();
        for input in [
            "tools/test-android16-surfaceflinger-core.sh",
            "tools/test-release-record-transport.sh",
            "tools/tests/release-record-transport-test.cc",
            "tools/tests/parcel-native-handle-test.cc",
            "probes/surfaceflinger_transaction_handler_compile.cc",
            "probes/surfaceflinger_transaction_handler_runtime.cc",
            "surfaceflinger-transaction-runtime",
            "libsurfaceflinger-frontend-darwin.a",
            "libbinder-darwin.a",
            "libgui-transaction-darwin.a",
            "libui-fence-darwin.a",
        ] {
            assert!(
                audit.contains(input),
                "missing SF audit input/output: {input}"
            );
        }
        fs::remove_file(output).unwrap();
    }

    #[test]
    fn interpreter_archive_edge_tracks_patch_or_orchestration_changes() {
        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = repository_root.join(format!(
            "_build/darwin-art-interpreter-graph-{}.ninja",
            std::process::id()
        ));
        emit_graph(&output).expect("emit native graph");
        let graph = fs::read_to_string(&output).expect("read emitted graph");
        let archive = repository_root.join("_build/interpreter-core/libart-interpreter-darwin.a");
        let archive_edge = graph
            .lines()
            .find(|line| {
                line.starts_with(&format!("build {}: interpreter_core", archive.display()))
            })
            .expect("interpreter archive producer edge");
        let shadow_marker = repository_root
            .join("_build/runtime-common/patched-source/.darwin-art-shadow-identity")
            .to_string_lossy()
            .into_owned();
        let shadow_edge = graph
            .lines()
            .find(|line| line.starts_with(&format!("build {shadow_marker}: runtime_common_shadow")))
            .expect("shared ART shadow producer edge");
        assert!(
            shadow_edge.contains("patches/art/0027-darwin-string-abi-overlay.patch"),
            "shadow producer must own the ABI header patch set"
        );
        assert!(
            archive_edge.contains(&shadow_marker),
            "interpreter must wait for complete shadow publication"
        );
        assert!(
            archive_edge.contains("patches/art/0074-darwin-interpreter-reference-copy.patch"),
            "interpreter reference-copy patch must invalidate its archive"
        );
        for input in interpreter_core_inputs(&repository_root) {
            assert!(
                archive_edge.contains(&input.to_string_lossy().to_string()),
                "interpreter edge is missing input {}",
                input.display()
            );
        }
        let graphics_link = graph
            .lines()
            .find(|line| line.starts_with(&format!("build {}: graphics_audit", repository_root.join("_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib").display())))
            .expect("graphics audit edge");
        assert!(
            graphics_link.contains(&archive.to_string_lossy().to_string()),
            "graphics link edge does not depend on interpreter archive"
        );
        let jit_edge = graph
            .lines()
            .find(|line| line.contains(": jit_compiler "))
            .expect("JIT compiler archive producer edge");
        assert!(
            jit_edge.contains(&shadow_marker),
            "JIT must wait for the narrow shadow producer"
        );
        assert!(
            !jit_edge.contains(
                repository_root
                    .join(GRAPHICS_BOOTSTRAP_ARCHIVE)
                    .to_string_lossy()
                    .as_ref()
            ),
            "JIT must not serialize behind the full graphics archive"
        );
        let compiler =
            fs::read_to_string(repository_root.join("crates/art-bootstrap/src/runtime_art/jit.rs"))
                .expect("read compiler patch registration");
        let patches: Vec<_> = compiler
            .split('"')
            .filter(|part| part.starts_with("patches/art/") && part.ends_with(".patch"))
            .collect();
        assert!(!patches.is_empty());
        for patch in patches {
            assert!(
                jit_edge.contains(patch),
                "JIT archive edge is missing {patch}"
            );
        }
        fs::remove_file(output).expect("remove emitted graph");
    }

    #[test]
    fn jit_layout_audit_has_a_real_success_stamp_output() {
        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = repository_root.join(format!(
            "_build/darwin-art-jit-layout-graph-{}.ninja",
            std::process::id()
        ));
        emit_graph(&output).expect("emit native graph");
        let graph = fs::read_to_string(&output).expect("read emitted graph");
        let alias = graph
            .lines()
            .find(|line| line.starts_with("build jit-layout-audit: phony "))
            .expect("JIT layout phony alias");
        assert!(
            alias.contains("jit-layout-audit.stamp"),
            "JIT layout alias must depend on its materialized stamp"
        );
        let stamp_edge = graph
            .lines()
            .find(|line| line.contains(": jit_layout_audit "))
            .expect("JIT layout stamp producer edge");
        assert!(
            stamp_edge.contains("jit-layout-audit.stamp"),
            "JIT layout rule must declare the stamp as its output"
        );
        assert!(
            stamp_edge.contains("tools/audit-jit-layout.sh")
                && stamp_edge.contains("tools/jit-layout-smoke.cc"),
            "JIT layout stamp must track the audit inputs"
        );
        let rule_start = graph
            .find("rule jit_layout_audit\n")
            .expect("JIT layout rule");
        let rule = graph[rule_start..]
            .split_once("\n\n")
            .expect("JIT layout rule body")
            .0;
        assert!(
            rule.contains("audit-jit-layout.sh && touch")
                && rule.contains("jit-layout-audit.stamp"),
            "JIT layout stamp must only be published after a successful audit"
        );
        fs::remove_file(output).expect("remove emitted graph");
    }

    #[test]
    fn every_bootstrap_cli_recipe_has_a_direct_binary_prerequisite() {
        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = repository_root.join(format!(
            "_build/darwin-art-cli-prerequisite-graph-{}.ninja",
            std::process::id()
        ));
        emit_graph(&output).expect("emit native graph");
        let graph = fs::read_to_string(&output).expect("read emitted graph");
        let bootstrap_cli_path = repository_root.join("target/debug/art-bootstrap");
        let bootstrap_cli = ninja_path(&bootstrap_cli_path);
        let bootstrap_cli_raw = bootstrap_cli_path.to_string_lossy();

        // Keep this scanner deliberately small: Ninja permits multiple outputs
        // before the colon, so identify the rule from the post-colon fields
        // rather than assuming a single output token.
        let mut cli_rules = Vec::new();
        let mut current_rule = None;
        for line in graph.lines() {
            if let Some(rule) = line.strip_prefix("rule ") {
                current_rule = Some(rule.trim());
            } else if line.starts_with("  command = ") && line.contains(bootstrap_cli_raw.as_ref())
            {
                if let Some(rule) = current_rule {
                    cli_rules.push(rule.to_owned());
                }
            }
        }
        assert!(
            !cli_rules.is_empty(),
            "emitted graph has no bootstrap CLI rules"
        );

        for rule in cli_rules {
            let edges: Vec<_> = graph
                .lines()
                .filter_map(|line| {
                    let (_, fields) = line.strip_prefix("build ")?.split_once(": ")?;
                    let mut fields = fields.split_whitespace();
                    (fields.next() == Some(rule.as_str())).then_some(
                        fields
                            .take_while(|field| *field != "||")
                            .collect::<Vec<_>>(),
                    )
                })
                .collect();
            assert!(
                !edges.is_empty(),
                "missing producer edge for bootstrap CLI rule {rule}"
            );
            for edge in edges {
                assert!(
                    edge.iter().any(|input| *input == bootstrap_cli.as_str()),
                    "bootstrap CLI rule {rule} lacks ordinary prerequisite {bootstrap_cli}"
                );
            }
        }
        fs::remove_file(output).expect("remove emitted graph");
    }

    #[test]
    fn probe_sources_do_not_invalidate_the_bootstrap_archive() {
        assert!(is_global_digest_excluded(Path::new(
            "probes/runtime_link_probe.cc"
        )));
        assert!(is_global_digest_excluded(Path::new(
            "compat/darwin_surface_bridge.mm"
        )));
        assert!(is_global_digest_excluded(Path::new(
            "compat/window/appkit_content_view.mm"
        )));
        assert!(is_global_digest_excluded(Path::new(
            "compat/input/darwin_hardware_key_translation.mm"
        )));
        assert!(is_global_digest_excluded(Path::new(
            "compat/darwin_surface_gpu_bridge.mm"
        )));
        assert!(is_global_digest_excluded(Path::new(
            "probes/runtime_fixture_options.cc"
        )));
        assert!(!is_global_digest_excluded(Path::new(
            "runtime/embedding/process_config.cc"
        )));
        assert!(!is_global_digest_excluded(Path::new(
            "compat/darwin_runtime_adapters.cc"
        )));
    }

    #[test]
    fn foundation_fallback_inputs_are_partitioned_by_owner() {
        let hwui_script = Path::new("tools/build-android16-hwui-static-foundation.sh");
        let graphics_jni_script = Path::new("tools/build-android16-android-graphics-jni.sh");
        let icu_script = Path::new("tools/build-android16-icu-foundation.sh");
        assert!(is_foundation_family_input(
            hwui_script,
            FoundationFamily::Hwui
        ));
        assert!(!is_foundation_family_input(
            hwui_script,
            FoundationFamily::Icu
        ));
        let hwui_animation_patch =
            Path::new("patches/frameworks-base/0005-darwin-hwui-animation-pulse.patch");
        assert!(is_foundation_family_input(
            hwui_animation_patch,
            FoundationFamily::Hwui
        ));
        assert!(!is_foundation_family_input(
            hwui_animation_patch,
            FoundationFamily::Icu
        ));
        assert!(is_foundation_family_input(
            graphics_jni_script,
            FoundationFamily::GraphicsJni
        ));
        assert!(!is_foundation_family_input(
            graphics_jni_script,
            FoundationFamily::Hwui
        ));
        assert!(is_foundation_family_input(
            icu_script,
            FoundationFamily::Icu
        ));
        assert!(!is_foundation_family_input(
            icu_script,
            FoundationFamily::GraphicsJni
        ));
        let icu_source = Path::new("_aosp/external/icu-graphics/icu4c/source/common/foo.cpp");
        let hwui_source = Path::new("_aosp/frameworks/base/libs/hwui/RenderNode.cpp");
        assert!(is_foundation_family_input(
            icu_source,
            FoundationFamily::Icu
        ));
        assert!(!is_foundation_family_input(
            icu_source,
            FoundationFamily::Hwui
        ));
        assert!(is_foundation_family_input(
            hwui_source,
            FoundationFamily::Hwui
        ));
        assert!(is_foundation_family_input(
            hwui_source,
            FoundationFamily::GraphicsJni
        ));
    }

    #[test]
    fn probe_content_stamp_changes_only_for_content_changes() {
        let root = std::env::temp_dir().join(format!(
            "darwin-art-probe-stamp-test-{}",
            std::process::id()
        ));
        let source = root.join("probes/state.cc");
        fs::create_dir_all(source.parent().expect("probe parent")).expect("probe directory");
        fs::write(&source, "state-v1\n").expect("initial probe source");
        let first =
            probe_content_stamp(&root, "state", &["probes/state.cc"]).expect("first content stamp");
        let first_content = fs::read_to_string(&first).expect("first stamp content");
        let second = probe_content_stamp(&root, "state", &["probes/state.cc"])
            .expect("stable content stamp");
        assert_eq!(first, second);
        assert_eq!(
            first_content,
            fs::read_to_string(&second).expect("stable stamp content")
        );
        fs::write(&source, "state-v2\n").expect("changed probe source");
        let third = probe_content_stamp(&root, "state", &["probes/state.cc"])
            .expect("changed content stamp");
        assert_ne!(
            first_content,
            fs::read_to_string(third).expect("changed stamp content")
        );
        fs::remove_dir_all(root).expect("probe stamp test cleanup");
    }

    #[test]
    fn production_owner_stamp_rejects_fixture_dependencies() {
        let root = Path::new("/nonexistent");
        for fixture in [
            "probes/runtime_graphics_vsync_diagnostic.cc",
            "probes/headless_graphics_fixture_natives.h",
            "probes/runtime_entry_probe.cc",
            "tools/bionic-float-conversion-facade/probes/host_state.cc",
            "tools/bionic-socket-broker-adapter/probes/fdsan.cc",
            "tools/tests/native-window-transaction-admission-test.cc",
        ] {
            assert_eq!(
                native_owner_content_stamp(root, "product", &[fixture])
                    .expect_err("fixture must be rejected before reading files")
                    .kind(),
                std::io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn provider_manifest_keeps_numeric_audit_inputs_off_production_edge() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest = provider_inputs::collect(&root).expect("provider input manifest");
        let production = &manifest.production;
        let audit = &manifest.audit;
        for input in [
            "tools/bionic-float-conversion-facade/build.rs",
            "tools/bionic-float-conversion-facade/src/lib.rs",
            "tools/bionic-binary128-conversion-facade/build.rs",
            "tools/bionic-binary128-conversion-facade/src/lib.rs",
            "tools/bionic-abort-facade/src/provider.c",
            "tools/bionic-errno-tls/src/errno_tls.c",
        ] {
            assert!(
                production.iter().any(|path| path == Path::new(input)),
                "production provider input missing: {input}"
            );
        }
        for input in [
            "tools/bionic-float-conversion-facade/probes/host_state.cc",
            "tools/bionic-binary128-conversion-facade/probes/host_state.cc",
        ] {
            assert!(
                audit.iter().any(|path| path == Path::new(input)),
                "numeric audit input missing: {input}"
            );
        }
        for path in production {
            let text = path.to_string_lossy();
            assert!(!text.starts_with("probes/") && !text.contains("/probes/"));
            assert!(!text.ends_with("/audit.sh"));
            assert!(!text.ends_with("/src/main.rs"));
        }
    }
}
