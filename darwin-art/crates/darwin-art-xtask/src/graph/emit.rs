use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use darwin_art_build_contract::RUNTIME_CACHE_IDENTITY;
use darwin_art_build_contract::{GRAPHICS_ADAPTER_SOURCES, HEADLESS_ADAPTER_SOURCES};

use super::super::*;
use super::GRAPH_VERSION;
use super::atomic;
use super::cache::{
    cached_native_objects_from_dirs, emit_cached_native_graph,
    emit_cached_native_graph_with_inputs, emit_cached_native_object_edges_with_order_only,
};
use super::foundation::{
    FoundationFamily, cached_foundation_objects, foundation_input_list, foundation_inputs,
};
use super::inputs::collect_files;
use super::manifest::prepare as prepare_manifest;
use super::probe_manifest;
use super::representative::emit_representative_edges;

pub(crate) fn interpreter_core_inputs(root: &Path) -> Vec<PathBuf> {
    [
        "crates/art-bootstrap/src/runtime_art/interpreter.rs",
        "patches/art/0037-darwin-shadow-frame-single-initialization.patch",
        "patches/art/0074-darwin-interpreter-reference-copy.patch",
        "_aosp/art/runtime/interpreter/interpreter.cc",
        "_aosp/art/runtime/interpreter/interpreter_cache.cc",
        "_aosp/art/runtime/interpreter/interpreter_common.cc",
        "_aosp/art/runtime/interpreter/interpreter_switch_impl0.cc",
        "_aosp/art/runtime/interpreter/lock_count_data.cc",
        "_aosp/art/runtime/interpreter/shadow_frame.cc",
        "_aosp/art/runtime/interpreter/shadow_frame.h",
        "_aosp/art/runtime/interpreter/unstarted_runtime.cc",
    ]
    .into_iter()
    .map(|path| root.join(path))
    .collect()
}

#[path = "../../../art-bootstrap/src/runtime_bootstrap/manifest.rs"]
mod bootstrap_shadow_manifest;

use bootstrap_shadow_manifest::PATCHED_RUNTIME_PATCHES as RUNTIME_COMMON_SHADOW_PATCHES;
use bootstrap_shadow_manifest::PATCHED_RUNTIME_SOURCES as RUNTIME_COMMON_SHADOW_SOURCES;
use bootstrap_shadow_manifest::RUNTIME_SHADOW_IDENTITY_VERSION;

#[cfg(test)]
mod tests {
    use super::{shadow_identity, shadow_identity_matches};
    use std::fs;

    #[test]
    fn both_runtime_flavors_track_provider_archive_changes() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = root.join(format!(
            "_build/provider-link-boundary-{}.ninja",
            std::process::id()
        ));
        super::emit_graph(&output).expect("emit native graph");
        let graph = fs::read_to_string(&output).expect("read native graph");
        for rule in [": headless_runtime_audit ", ": graphics_audit "] {
            let edge = graph
                .lines()
                .find(|line| line.contains(rule))
                .expect("runtime link edge");
            for archive in [
                "libdarwin-art-bionic-rust-providers.a",
                "libdarwin-art-bionic-native-providers.a",
                "libdarwin-art-bionic-float-conversion.a",
                "libdarwin-art-bionic-binary128-conversion.a",
            ] {
                assert!(edge.contains(archive), "{rule} omits {archive}");
            }
        }
        for rule in [
            ": bionic_provider_closure ",
            ": bionic_provider_acceptance ",
        ] {
            let edge = graph
                .lines()
                .find(|line| line.contains(rule))
                .expect("provider producer/audit edge");
            for archive in [
                "libandroidicuinit-darwin.a",
                "libicuuc-common-darwin.a",
                "libicuuc-stubdata-darwin.a",
                "liblog-darwin.a",
            ] {
                assert!(edge.contains(archive), "{rule} omits real read {archive}");
            }
        }
        fs::remove_file(output).expect("remove test graph");
    }

    #[test]
    fn graph_shadow_identity_matches_bootstrap_manifest() {
        assert_eq!(
            super::RUNTIME_COMMON_SHADOW_SOURCES,
            super::bootstrap_shadow_manifest::PATCHED_RUNTIME_SOURCES
        );
        assert_eq!(
            super::RUNTIME_COMMON_SHADOW_PATCHES,
            super::bootstrap_shadow_manifest::PATCHED_RUNTIME_PATCHES
        );
    }

    #[test]
    fn changed_shadow_patch_blocks_cached_promotion() {
        let root = std::env::temp_dir().join(format!(
            "darwin-art-shadow-patch-test-{}",
            std::process::id()
        ));
        let staged = root.join("patched-source");
        let source = staged.join("runtime/jit/jit.cc");
        let patch_file = root.join("patches/test.patch");
        fs::create_dir_all(source.parent().expect("source parent")).expect("source directory");
        fs::create_dir_all(patch_file.parent().expect("patch parent")).expect("patch directory");
        fs::write(&source, "old\n").expect("original staged source");
        fs::write(&patch_file, "patch-v1\n").expect("patch file");
        let sources = vec![source];
        let patches = vec![patch_file];
        let marker = staged.join(".darwin-art-shadow-identity");
        fs::write(
            &marker,
            format!(
                "{}\n",
                shadow_identity(&sources, &patches).expect("identity")
            ),
        )
        .expect("shadow marker");
        assert!(shadow_identity_matches(&marker, &sources, &patches));
        fs::write(&patches[0], "patch-v2\n").expect("changed patch file");
        assert!(!shadow_identity_matches(&marker, &sources, &patches));
        fs::remove_dir_all(root).expect("test cleanup");
    }
}

// The native graph consumes art-bootstrap's canonical runtime-shadow manifest
// directly. This keeps cache invalidation and source staging on one contract;
// a newly added runtime patch cannot be omitted from cached-object promotion.

fn shadow_identity(source_paths: &[PathBuf], patch_paths: &[PathBuf]) -> Option<String> {
    let mut digest = Sha256::new();
    digest.update(RUNTIME_SHADOW_IDENTITY_VERSION.as_bytes());
    digest.update([0]);
    for path in source_paths.iter().chain(patch_paths) {
        let bytes = fs::read(path).ok()?;
        digest.update(path.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(bytes);
        digest.update([0]);
    }
    Some(format!("{:x}", digest.finalize()))
}

fn shadow_identity_matches(
    marker: &Path,
    source_paths: &[PathBuf],
    patch_paths: &[PathBuf],
) -> bool {
    shadow_identity(source_paths, patch_paths).is_some_and(|expected| {
        fs::read_to_string(marker).is_ok_and(|actual| actual.trim() == expected)
    })
}

// Every recipe that invokes the bootstrap CLI must depend on the exact binary
// path it executes.  A phony alias containing both targets does not order
// sibling prerequisites in Ninja, so omitting this direct edge can run an old
// CLI while Cargo is rebuilding the same path.
fn append_bootstrap_cli_prerequisite(graph: &mut String, target: &str) {
    graph.push_str(target);
    graph.push(' ');
}

fn runtime_common_shadow_is_current(root: &Path) -> bool {
    let runtime = root.join("_aosp/art/runtime");
    let source_files = RUNTIME_COMMON_SHADOW_SOURCES
        .iter()
        .map(|source| runtime.join(source))
        .collect::<Vec<_>>();
    let patch_files = RUNTIME_COMMON_SHADOW_PATCHES
        .iter()
        .map(|patch| root.join(patch))
        .collect::<Vec<_>>();
    shadow_identity_matches(
        &root.join("_build/runtime-common/patched-source/.darwin-art-shadow-identity"),
        &source_files,
        &patch_files,
    )
}

fn runtime_common_shadow_inputs(root: &Path) -> Vec<PathBuf> {
    let runtime = root.join("_aosp/art/runtime");
    let mut inputs = RUNTIME_COMMON_SHADOW_SOURCES
        .iter()
        .map(|source| runtime.join(source))
        .chain(
            RUNTIME_COMMON_SHADOW_PATCHES
                .iter()
                .map(|patch| root.join(patch)),
        )
        .collect::<Vec<_>>();
    inputs.push(root.join("crates/art-bootstrap/src/runtime_bootstrap/manifest.rs"));
    inputs.push(root.join("crates/art-bootstrap/src/runtime_bootstrap/staging.rs"));
    inputs.sort();
    inputs.dedup();
    inputs
}

pub(crate) fn emit_graph(out: &Path) -> io::Result<()> {
    emit_graph_with_support(out, false)
}

pub(crate) fn emit_graph_with_support(out: &Path, with_support: bool) -> io::Result<()> {
    let manifest = prepare_manifest(out)?;
    let root = manifest.root;
    let inputs = manifest.inputs;
    let bootstrap_inputs = manifest.bootstrap_inputs;
    let digest = manifest.digest;
    let toolchain = manifest.toolchain;
    let cache_dir = manifest.cache_dir;
    let digest_path = manifest.digest_path;

    let root_for_shell = root.to_string_lossy().into_owned();
    // Ninja edges run the already-built bootstrap CLI directly.  Cargo is
    // still the public entry point that emits this graph, but invoking Cargo
    // once per native TU edge defeats the persistent object cache and adds a
    // process/workspace resolution cost to every warm build.
    let bootstrap_cli = shell_quote(&root.join("target/debug/art-bootstrap").to_string_lossy());
    let bootstrap_cli_path = root.join("target/debug/art-bootstrap");
    let bootstrap_cli_target = ninja_path(&bootstrap_cli_path);
    let mut bootstrap_cli_inputs = vec![root.join("Cargo.toml"), root.join("Cargo.lock")];
    collect_files(
        &root.join("crates/art-bootstrap"),
        &root,
        &mut bootstrap_cli_inputs,
    );
    collect_files(
        &root.join("crates/darwin-art-build-contract"),
        &root,
        &mut bootstrap_cli_inputs,
    );
    bootstrap_cli_inputs.sort();
    bootstrap_cli_inputs.dedup();
    // Keep compiler outputs at a stable path.  The graph digest is a
    // manifest/invalidation identity, not an object-cache namespace: moving
    // objects into a new digest directory would turn every header edit into
    // a cold 200+ TU rebuild and defeat dependency-fingerprint caching.
    let native_output_root = root.join("_build");
    let runtime_owner_archive_path = root.join("target/release/libdarwin_art_runtime.a");
    let interpreter_archive_path = root.join("_build/interpreter-core/libart-interpreter-darwin.a");
    let runtime_shadow_marker_path =
        root.join("_build/runtime-common/patched-source/.darwin-art-shadow-identity");
    let archive_path = native_output_root.join(GRAPHICS_BOOTSTRAP_ARCHIVE);
    let runtime_archive_path = native_output_root.join(RUNTIME_BOOTSTRAP_ARCHIVE);
    let hwui_foundation_archive_path = native_output_root.join(HWUI_STATIC_FOUNDATION_ARCHIVE);
    let hwui_apex_foundation_archive_path = native_output_root.join(HWUI_APEX_FOUNDATION_ARCHIVE);
    let graphics_jni_archive_path = native_output_root.join(ANDROID_GRAPHICS_JNI_ARCHIVE);
    let graphics_registrar_archive_path =
        native_output_root.join(ANDROID_GRAPHICS_REGISTRAR_ARCHIVE);
    let graphics_force_loaded_object_path =
        native_output_root.join(ANDROID_GRAPHICS_FORCE_LOADED_OBJECT);
    let icu_common_archive_path = native_output_root.join(ICU_COMMON_FOUNDATION_ARCHIVE);
    let icu_i18n_archive_path = native_output_root.join(ICU_I18N_FOUNDATION_ARCHIVE);
    let icu_stubdata_archive_path = native_output_root.join(ICU_STUBDATA_FOUNDATION_ARCHIVE);
    let icu_init_archive_path = native_output_root.join(ICU_INIT_FOUNDATION_ARCHIVE);
    // The shell builder has a fixed, published output contract under the
    // repository root. Keep this edge on that path even if other graph
    // artifacts are moved behind a custom native output root.
    let libcore_linux_archive_path =
        root.join("_build/libcore-darwin-linux/libcore-darwin-linux.a");
    let unix_filesystem_archive_path =
        root.join("_build/unix-filesystem-darwin/libopenjdk-unix-filesystem-darwin.a");
    let system_natives_archive_path =
        root.join("_build/system-natives-darwin/libopenjdk-system-natives-darwin.a");
    let boringssl_archive_path =
        root.join("_build/system-natives-darwin/libcrypto-boringssl-darwin.a");
    let runtime_library_path = native_output_root.join(GRAPHICS_RUNTIME_LIBRARY);
    let headless_runtime_library_path = native_output_root.join(HEADLESS_RUNTIME_LIBRARY);
    let surfaceflinger_frontend_archive_path =
        native_output_root.join("surfaceflinger-core/libsurfaceflinger-frontend-darwin.a");
    let surfaceflinger_binder_archive_path =
        native_output_root.join("surfaceflinger-core/libbinder-darwin.a");
    let surfaceflinger_gui_archive_path =
        native_output_root.join("surfaceflinger-core/libgui-transaction-darwin.a");
    let surfaceflinger_fence_archive_path =
        native_output_root.join("surfaceflinger-core/libui-fence-darwin.a");
    let surfaceflinger_runtime_probe_path =
        native_output_root.join("surfaceflinger-core/surfaceflinger-transaction-runtime");
    let graphics_runtime_closure_path = native_output_root
        .join("graphics-runtime-closure-audit/android16-graphics-runtime-closure.o");
    let bionic_provider_root = native_output_root.join("bionic-runtime-provider-closure");
    let bionic_rust_provider_archive_path =
        bionic_provider_root.join("libdarwin-art-bionic-rust-providers.a");
    let bionic_native_provider_archive_path =
        bionic_provider_root.join("libdarwin-art-bionic-native-providers.a");
    let bionic_float_provider_archive_path =
        bionic_provider_root.join("libdarwin-art-bionic-float-conversion.a");
    let bionic_binary128_provider_archive_path =
        bionic_provider_root.join("libdarwin-art-bionic-binary128-conversion.a");
    let network_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_network_probe.cc.o");
    let hwui_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_hwui_probe.cc.o");
    let graphics_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_graphics_probe.cc.o");
    let graphics_gpu_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_graphics_gpu.cc.o");
    let graphics_phase_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_graphics_phase.cc.o");
    let graphics_input_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_graphics_input.cc.o");
    let graphics_state_object_path =
        native_output_root.join("runtime-embedding/darwin_art_graphics_state.cc.o");
    let graphics_session_object_path =
        native_output_root.join("runtime-embedding/darwin_art_graphics_session.cc.o");
    let jni_acceptance_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_jni_acceptance_probe.cc.o");
    let app_bootstrap_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_app_bootstrap.cc.o");
    let app_resources_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_app_resources.cc.o");
    let app_activity_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_app_activity.cc.o");
    let app_presentation_object_path =
        native_output_root.join("runtime-probes/darwin_art_runtime_app_presentation.cc.o");
    let stamp_path = cache_dir.join("graphics-bootstrap.stamp");
    let graphics_ready_path = cache_dir.join("graphics-bootstrap.ready");
    let runtime_stamp_path = cache_dir.join("runtime-bootstrap.stamp");
    let runtime_ready_path = cache_dir.join("runtime-bootstrap.ready");
    let jit_layout_stamp_path = cache_dir.join("jit-layout-audit.stamp");
    let stamp = ninja_path(&stamp_path);
    let runtime_stamp = ninja_path(&runtime_stamp_path);
    let jit_layout_stamp = ninja_path(&jit_layout_stamp_path);
    let archive = ninja_path(&archive_path);
    let runtime_archive = ninja_path(&runtime_archive_path);
    let hwui_foundation_archive = ninja_path(&hwui_foundation_archive_path);
    let hwui_apex_foundation_archive = ninja_path(&hwui_apex_foundation_archive_path);
    let graphics_jni_archive = ninja_path(&graphics_jni_archive_path);
    let graphics_registrar_archive = ninja_path(&graphics_registrar_archive_path);
    let graphics_force_loaded_object = ninja_path(&graphics_force_loaded_object_path);
    let icu_common_archive = ninja_path(&icu_common_archive_path);
    let icu_i18n_archive = ninja_path(&icu_i18n_archive_path);
    let icu_stubdata_archive = ninja_path(&icu_stubdata_archive_path);
    let icu_init_archive = ninja_path(&icu_init_archive_path);
    let libcore_linux_archive = ninja_path(&libcore_linux_archive_path);
    let unix_filesystem_archive = ninja_path(&unix_filesystem_archive_path);
    let system_natives_archive = ninja_path(&system_natives_archive_path);
    let boringssl_archive = ninja_path(&boringssl_archive_path);
    let runtime_library = ninja_path(&runtime_library_path);
    let headless_runtime_library = ninja_path(&headless_runtime_library_path);
    let surfaceflinger_frontend_archive = ninja_path(&surfaceflinger_frontend_archive_path);
    let surfaceflinger_binder_archive = ninja_path(&surfaceflinger_binder_archive_path);
    let surfaceflinger_gui_archive = ninja_path(&surfaceflinger_gui_archive_path);
    let surfaceflinger_fence_archive = ninja_path(&surfaceflinger_fence_archive_path);
    let surfaceflinger_runtime_probe = ninja_path(&surfaceflinger_runtime_probe_path);
    let graphics_runtime_closure = ninja_path(&graphics_runtime_closure_path);
    let bionic_rust_provider_archive = ninja_path(&bionic_rust_provider_archive_path);
    let bionic_native_provider_archive = ninja_path(&bionic_native_provider_archive_path);
    let bionic_float_provider_archive = ninja_path(&bionic_float_provider_archive_path);
    let bionic_binary128_provider_archive = ninja_path(&bionic_binary128_provider_archive_path);
    let runtime_owner_archive = ninja_path(&runtime_owner_archive_path);
    let interpreter_archive = ninja_path(&interpreter_archive_path);
    let runtime_shadow_marker = ninja_path(&runtime_shadow_marker_path);
    let network_object = ninja_path(&network_object_path);
    let hwui_object = ninja_path(&hwui_object_path);
    let graphics_object = ninja_path(&graphics_object_path);
    let graphics_gpu_object = ninja_path(&graphics_gpu_object_path);
    let graphics_phase_object = ninja_path(&graphics_phase_object_path);
    let graphics_input_object = ninja_path(&graphics_input_object_path);
    let graphics_state_object = ninja_path(&graphics_state_object_path);
    let jni_acceptance_object = ninja_path(&jni_acceptance_object_path);
    let app_bootstrap_object = ninja_path(&app_bootstrap_object_path);
    let app_resources_object = ninja_path(&app_resources_object_path);
    let app_activity_object = ninja_path(&app_activity_object_path);
    let app_presentation_object = ninja_path(&app_presentation_object_path);
    let stamp_for_shell = stamp_path.to_string_lossy().into_owned();
    let native_output_for_shell = native_output_root.to_string_lossy().into_owned();
    let runtime_common_objects = native_output_root.join("runtime-common/objects");
    let runtime_objects = native_output_root.join("runtime-bootstrap/objects");
    let graphics_objects = native_output_root.join("runtime-graphics-bootstrap/objects");
    let shared_runtime_cache_ready =
        fs::read_to_string(native_output_root.join("runtime-common/cache-identity"))
            .is_ok_and(|identity| identity.trim() == RUNTIME_CACHE_IDENTITY);
    // Cached objects refer to the shared staged shadow. If a patch changed
    // after that shadow was prepared, force the canonical bootstrap edge to
    // run so it restages the original sources before recompiling. Otherwise a
    // regenerated graph would faithfully reuse an obsolete staged pathname.
    let runtime_common_shadow_current = runtime_common_shadow_is_current(&root);
    let cached_runtime_objects = if shared_runtime_cache_ready && runtime_common_shadow_current {
        cached_native_objects_from_dirs(
            &[&runtime_common_objects, &runtime_objects],
            &runtime_archive_path,
            HEADLESS_ADAPTER_SOURCES,
        )?
    } else {
        None
    };
    let cached_graphics_objects = if shared_runtime_cache_ready && runtime_common_shadow_current {
        cached_native_objects_from_dirs(
            &[&runtime_common_objects, &graphics_objects],
            &archive_path,
            GRAPHICS_ADAPTER_SOURCES,
        )?
    } else {
        None
    };
    let network_object_for_shell = network_object_path.to_string_lossy().into_owned();
    let fallback_adapter_sources = super::inputs::bootstrap_fallback_adapter_sources();
    let bootstrap_input_list = bootstrap_inputs
        .iter()
        .chain(
            inputs
                .iter()
                .filter(|path| super::inputs::is_bootstrap_fallback_header(path)),
        )
        // A new adapter has no persisted object command yet. Its source may
        // deliberately be excluded from the broad ART digest, but the
        // canonical fallback still reads it and must track that dependency.
        // Otherwise a second edit before graph regeneration can reuse an
        // archive compiled from the previous provider implementation.
        .chain(fallback_adapter_sources.iter())
        .map(|path| ninja_path(&root.join(path)))
        .collect::<Vec<_>>()
        .join(" ");
    let foundation_inputs = foundation_inputs(&root);
    let hwui_foundation_input_list =
        foundation_input_list(&root, &foundation_inputs, FoundationFamily::Hwui);
    let graphics_jni_foundation_input_list =
        foundation_input_list(&root, &foundation_inputs, FoundationFamily::GraphicsJni);
    let icu_foundation_input_list =
        foundation_input_list(&root, &foundation_inputs, FoundationFamily::Icu);
    let graphics_runtime_closure_inputs = [
        "_build/android-graphics-jni/libandroid-graphics-layoutlib-registrar-darwin.a",
        "_build/android-graphics-jni/libandroid-graphics-jni-darwin.a",
        "_build/hwui-static-foundation/libhwui-static-darwin.a",
        "_build/hwui-static-foundation/libandroid-graphics-apex-common-darwin.a",
        "_build/skia-metal-gpu/libskia.a",
        "_build/skia-metal-gpu/libskcms.a",
        "_build/androidfw-foundation/libandroidfw-darwin.a",
        "_build/hostgraphics/libhostgraphics-darwin.a",
        "_build/codec-foundation/libimage_io-darwin.a",
        "_build/codec-foundation/libmodpb64-darwin.a",
        "_build/codec-foundation/libultrahdr-darwin.a",
        "_build/codec-foundation/libjpegencoder-darwin.a",
        "_build/codec-foundation/libjpegdecoder-darwin.a",
        "_build/codec-foundation/libjpeg-darwin.a",
        "_build/minikin-foundation/libminikin.a",
        "_build/harfbuzz-foundation/libharfbuzz_ng-darwin.a",
        "_build/graphics-codecs/libft2-darwin.a",
        "_build/ui-types-foundation/libui-types.a",
        "_build/nativehelper-device-foundation/libnativehelper-device-darwin.a",
        "_build/nativehelper-foundation/libnativehelper_any_vm.a",
        "_build/graphics-foundations/libutils-darwin.a",
        "_build/graphics-foundations/libutils-binder-darwin.a",
        "_build/graphics-foundations/libcutils-darwin.a",
        "_build/graphics-foundations/liblog-darwin.a",
        "_build/libbase-foundation/libandroid-base-darwin.a",
        "_build/ziparchive-incfs/libziparchive-for-incfs-darwin.a",
        "_build/graphics-codecs/libpng-darwin.a",
        "_build/graphics-codecs/libz-darwin.a",
        "_build/icu-foundation/libicui18n-darwin.a",
        "_build/icu-foundation/libicuuc-common-darwin.a",
        "_build/icu-foundation/libandroidicuinit-darwin.a",
        "_build/icu-foundation/libicuuc-stubdata-darwin.a",
        "_build/angle-source/out/DarwinArtRelease/libEGL.dylib",
        "_build/angle-source/out/DarwinArtRelease/libGLESv2.dylib",
        "tools/audit-android16-graphics-closure.sh",
        "upstream/android16-graphics-closure-audit.lock",
    ]
    .iter()
    .map(|path| ninja_path(&root.join(path)))
    .collect::<Vec<_>>()
    .join(" ");
    // The provider closure is linked directly into the final runtime dylib,
    // rather than copied into the large ART/HWUI bootstrap archive. Keep it
    // on its own narrow Ninja edge so a Rust facade edit rebuilds four small
    // provider archives and relinks, without recompiling the C++ runtime.
    let provider_manifest = super::provider_inputs::collect(&root)?;
    let bionic_provider_input_list = provider_manifest
        .production
        .iter()
        .filter(|path| root.join(path).is_file())
        .map(|path| ninja_path(&root.join(path)))
        .collect::<Vec<_>>()
        .join(" ");
    let interpreter_inputs = interpreter_core_inputs(&root);
    let runtime_shadow_inputs = runtime_common_shadow_inputs(&root);
    // Probe objects are separate graph products.  Do not attach the complete
    // bootstrap input closure to each one: that turns an edit to an unrelated
    // probe/provider into a rebuild of every probe.  The compiler writes the
    // real transitive dependency list to `$out.d`; the explicit inputs below
    // seed Ninja's first build and keep the ownership boundary readable.
    let probe_manifest::ProbeGraphInputs {
        network_probe_inputs,
        hwui_probe_inputs,
        graphics_probe_inputs,
        graphics_gpu_probe_inputs,
        graphics_phase_inputs,
        graphics_input_inputs,
        graphics_state_inputs,
        graphics_session_inputs,
        jni_acceptance_inputs,
        app_bootstrap_inputs,
        app_resources_inputs,
        app_activity_inputs,
        app_presentation_inputs,
        network_probe_stamp,
        hwui_probe_stamp,
        graphics_probe_stamp,
        graphics_gpu_probe_stamp,
        graphics_phase_stamp,
        graphics_input_stamp,
        graphics_state_stamp,
        graphics_session_stamp,
        jni_acceptance_stamp,
        app_bootstrap_stamp,
        app_resources_stamp,
        app_activity_stamp,
        app_presentation_stamp,
        runtime_entry_stamp,
    } = probe_manifest::collect(&root)?;

    // Rust runtime ownership is a first-class link input.  Keeping this as a
    // separate Ninja edge means a RuntimeSession/provider change rebuilds the
    // Rust archive and relinks the final dylib without invalidating any C++
    // translation unit in the ART/HWUI graph.
    let mut runtime_owner_inputs = vec![root.join("Cargo.toml"), root.join("Cargo.lock")];
    for directory in [
        "crates/darwin-art-runtime",
        "crates/darwin-art-engine-sys",
        "crates/darwin-art-abi",
    ] {
        collect_files(&root.join(directory), &root, &mut runtime_owner_inputs);
    }
    runtime_owner_inputs.sort();
    runtime_owner_inputs.dedup();

    let mut graph = String::new();
    graph.push_str("# Generated by darwin-art-xtask. Do not edit.\n");
    graph.push_str(&format!("# graph-version: {GRAPH_VERSION}\n"));
    graph.push_str(&format!("# input-digest: {digest}\n"));
    graph.push_str(&format!("# compiler: {} sdk: {SDK_NAME}\n", toolchain.cxx));
    graph.push_str("ninja_required_version = 1.10\n\n");
    graph.push_str("pool runtime_bootstrap_fallback\n  depth = 1\n\n");
    graph.push_str("rule art_bootstrap_cli\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && cargo build -q -p art-bootstrap\n");
    graph.push_str("  description = Rust art-bootstrap CLI\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&bootstrap_cli_target);
    graph.push_str(": art_bootstrap_cli ");
    for input in &bootstrap_cli_inputs {
        graph.push_str(&ninja_path(input));
        graph.push(' ');
    }
    graph.push('\n');
    graph.push_str("rule runtime_common_shadow\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && ");
    graph.push_str(&bootstrap_cli);
    graph.push_str(" prepare-runtime-common-shadow\n");
    graph.push_str("  description = STAGE shared ART runtime sources\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&runtime_shadow_marker);
    graph.push_str(": runtime_common_shadow ");
    graph.push_str(&bootstrap_cli_target);
    for input in &runtime_shadow_inputs {
        graph.push(' ');
        graph.push_str(&ninja_path(input));
    }
    graph.push_str("\nbuild runtime-common-shadow: phony ");
    graph.push_str(&runtime_shadow_marker);
    graph.push_str("\n\n");
    let mut cached_rules_emitted = false;
    // The runtime-common directory is shared by both archive flavors. It used
    // to be passed only as an archive input, which made Ninja treat stale
    // objects as source files and never rebuild them after their C++ source
    // changed. Emit exactly one producer edge per common object, taking the
    // first complete flavor cache as the authoritative persisted command set.
    let shared_objects = cached_graphics_objects
        .as_deref()
        .or(cached_runtime_objects.as_deref())
        .map(|objects| {
            objects
                .iter()
                .filter(|object| object.object.starts_with(&runtime_common_objects))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let shared_inputs = emit_cached_native_object_edges_with_order_only(
        &mut graph,
        &shared_objects,
        &mut cached_rules_emitted,
        std::slice::from_ref(&runtime_shadow_marker_path),
    )
    .into_iter()
    .map(PathBuf::from)
    .collect::<Vec<_>>();
    if let Some(cached_objects) = cached_graphics_objects.as_deref() {
        let (_, flavor_objects): (Vec<_>, Vec<_>) = cached_objects
            .iter()
            .cloned()
            .partition(|object| object.object.starts_with(&runtime_common_objects));
        emit_cached_native_graph_with_inputs(
            &mut graph,
            &flavor_objects,
            &archive,
            &mut cached_rules_emitted,
            &shared_inputs,
            std::slice::from_ref(&runtime_shadow_marker_path),
        );
        graph.push_str("build ");
        graph.push_str(&stamp);
        graph.push_str(": phony ");
        graph.push_str(&archive);
        graph.push('\n');
        graph.push_str("# graphics-bootstrap uses persisted per-object commands\n\n");
    } else {
        // A prior fallback may have produced an archive before a newly added
        // adapter source was promoted into the cache. The readiness marker is
        // intentionally a separate output: an existing archive alone must
        // not make Ninja accept an incomplete canonical fallback.
        graph.push_str("rule graphics_bootstrap\n");
        graph.push_str("  pool = runtime_bootstrap_fallback\n");
        graph.push_str("  command = cd ");
        graph.push_str(&shell_quote(&root_for_shell));
        graph.push_str(" && DARWIN_ART_NATIVE_OUTPUT_ROOT=");
        graph.push_str(&shell_quote(&native_output_for_shell));
        graph.push(' ');
        graph.push_str(&bootstrap_cli);
        graph.push_str(" build-runtime-graphics-bootstrap-internal && touch ");
        graph.push_str(&shell_quote(&stamp_for_shell));
        graph.push(' ');
        graph.push_str(&shell_quote(&graphics_ready_path.to_string_lossy()));
        graph.push('\n');
        graph.push_str("  description = GRAPHICS bootstrap\n");
        graph.push_str("  restat = 1\n\n");
        graph.push_str("build ");
        graph.push_str(&stamp);
        graph.push(' ');
        graph.push_str(&archive);
        graph.push(' ');
        graph.push_str(&ninja_path(&graphics_ready_path));
        graph.push_str(": graphics_bootstrap ");
        // Depend at the producer edge, not only the phony alias: otherwise
        // Ninja can execute the old CLI while rebuilding that same binary.
        graph.push_str(&bootstrap_cli_target);
        graph.push(' ');
        graph.push_str(&runtime_shadow_marker);
        graph.push(' ');
        graph.push_str(&bootstrap_input_list);
        graph.push('\n');
    }
    graph.push_str("build graphics-bootstrap: phony ");
    graph.push_str(&bootstrap_cli_target);
    graph.push(' ');
    graph.push_str(&archive);
    graph.push('\n');
    if let Some(cached_objects) = cached_runtime_objects.as_deref() {
        let (_, flavor_objects): (Vec<_>, Vec<_>) = cached_objects
            .iter()
            .cloned()
            .partition(|object| object.object.starts_with(&runtime_common_objects));
        emit_cached_native_graph_with_inputs(
            &mut graph,
            &flavor_objects,
            &runtime_archive,
            &mut cached_rules_emitted,
            &shared_inputs,
            std::slice::from_ref(&runtime_shadow_marker_path),
        );
        graph.push_str("build ");
        graph.push_str(&runtime_stamp);
        graph.push_str(": phony ");
        graph.push_str(&runtime_archive);
        graph.push('\n');
        graph.push_str("# runtime-bootstrap uses persisted per-object commands\n\n");
    } else {
        // Keep the same completeness contract for the headless archive. This
        // marker is absent after an interrupted or source-incomplete fallback
        // and therefore forces the canonical builder to run again.
        graph.push_str("rule runtime_bootstrap\n");
        graph.push_str("  pool = runtime_bootstrap_fallback\n");
        graph.push_str("  command = cd ");
        graph.push_str(&shell_quote(&root_for_shell));
        graph.push_str(" && DARWIN_ART_NATIVE_OUTPUT_ROOT=");
        graph.push_str(&shell_quote(&native_output_for_shell));
        graph.push(' ');
        graph.push_str(&bootstrap_cli);
        graph.push_str(" build-runtime-bootstrap-internal && touch ");
        graph.push_str(&shell_quote(&runtime_stamp_path.to_string_lossy()));
        graph.push(' ');
        graph.push_str(&shell_quote(&runtime_ready_path.to_string_lossy()));
        graph.push('\n');
        graph.push_str("  description = ART bootstrap\n");
        graph.push_str("  restat = 1\n\n");
        graph.push_str("build ");
        graph.push_str(&runtime_stamp);
        graph.push(' ');
        graph.push_str(&runtime_archive);
        graph.push(' ');
        graph.push_str(&ninja_path(&runtime_ready_path));
        graph.push_str(": runtime_bootstrap ");
        graph.push_str(&bootstrap_cli_target);
        graph.push(' ');
        graph.push_str(&runtime_shadow_marker);
        graph.push(' ');
        graph.push_str(&bootstrap_input_list);
        graph.push('\n');
    }
    graph.push_str("build runtime-bootstrap: phony ");
    graph.push_str(&bootstrap_cli_target);
    graph.push(' ');
    graph.push_str(&runtime_archive);
    graph.push('\n');

    let input_keymaps_archive = super::input_keymaps::emit(&mut graph, &root);
    // The headless C ABI dylib is a first-class graph artifact as well.  Keep
    // its producer on the canonical audit path so a stale bootstrap archive
    // can never be force-loaded behind Ninja's back, and so reproducibility
    // checks exercise the same edge users build in CI.
    graph.push_str("rule headless_runtime_audit\n");
    graph.push_str("  pool = runtime_bootstrap_fallback\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && ");
    graph.push_str(&bootstrap_cli);
    graph.push_str(" audit-runtime-link\n");
    graph.push_str("  description = HEADLESS runtime link/audit\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&headless_runtime_library);
    graph.push_str(": headless_runtime_audit ");
    graph.push_str(&runtime_archive);
    graph.push(' ');
    graph.push_str(&bootstrap_cli_target);
    // The headless linker consumes the same provider archives as graphics.
    // Track those products directly: a provider-only edit must relink both
    // runtime flavors even when the ART bootstrap archive is unchanged.
    for archive in [
        &bionic_rust_provider_archive,
        &bionic_native_provider_archive,
        &bionic_float_provider_archive,
        &bionic_binary128_provider_archive,
        &input_keymaps_archive,
    ] {
        graph.push(' ');
        graph.push_str(archive);
    }
    for input in &bootstrap_inputs {
        graph.push(' ');
        graph.push_str(&ninja_path(&root.join(input)));
    }
    graph.push('\n');
    graph.push_str("build headless-runtime-audit: phony ");
    graph.push_str(&bootstrap_cli_target);
    graph.push(' ');
    graph.push_str(&headless_runtime_library);
    graph.push_str("\n\n");

    // The standalone interpreter archive is consumed by both runtime-link
    // audits. Keep its Rust orchestration file and dedicated shadow-frame
    // patch on this narrow edge so either change rebuilds the archive and its
    // final dylib dependents without rotating the broad native graph cache.
    graph.push_str("rule interpreter_core\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && ");
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-interpreter-core\n");
    graph.push_str("  description = ART interpreter core\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&interpreter_archive);
    graph.push_str(": interpreter_core ");
    graph.push_str(&bootstrap_cli_target);
    graph.push(' ');
    graph.push_str(&runtime_shadow_marker);
    graph.push(' ');
    for input in &interpreter_inputs {
        graph.push_str(&ninja_path(input));
        graph.push(' ');
    }
    graph.push('\n');
    graph.push_str("build interpreter-core: phony ");
    graph.push_str(&bootstrap_cli_target);
    graph.push(' ');
    graph.push_str(&interpreter_archive);
    graph.push('\n');

    // The JIT compiler consumes the shared staged ABI headers. Depend on the
    // narrow staging producer rather than the complete graphics archive so
    // JIT and runtime compilation can proceed in parallel after publication.
    let jit_archive = ninja_path(&root.join("_build/jit-compiler/libart-compiler-darwin.a"));
    let jit_support_archive =
        ninja_path(&root.join("_build/jit-compiler/libart-libelffile-darwin.a"));
    let mut jit_inputs = Vec::new();
    for directory in [
        "_aosp/art/compiler",
        "_aosp/art/disassembler",
        "_aosp/external/vixl",
        "_aosp/external/lzma",
        "_aosp/art/libelffile",
        "_aosp/art/runtime",
        "_aosp/art/libartbase",
        "compat",
    ] {
        collect_files(&root.join(directory), &root, &mut jit_inputs);
    }
    for file in [
        "sources.lock",
        "crates/art-bootstrap/src/runtime_art/jit.rs",
        "crates/art-bootstrap/src/runtime_art/jit_support.rs",
        "patches/art/0040-darwin-jit-compiler-gate.patch",
        "patches/art/0043-darwin-jit-reference-return.patch",
        "patches/art/0044-darwin-arm64-compressed32-field-get.patch",
        "patches/art/0045-darwin-jit-forwarding-call-abi.patch",
        "patches/art/0050-darwin-jit-native-root-slot-literals.patch",
        "patches/art/0052-darwin-jit-boot-object-literals.patch",
        "patches/art/0053-darwin-arm64-compressed-field-store-card-address.patch",
        "patches/art/0054-darwin-arm64-compressed-array-addresses.patch",
        "patches/art/0056-darwin-arm64-allocation-boundary.patch",
        "patches/art/0059-darwin-arm64-virtual-dispatch-addresses.patch",
        "patches/art/0060-darwin-jit-inline-capability.patch",
        "patches/art/0061-darwin-arm64-throw-boundary.patch",
        "patches/art/0062-darwin-arm64-monitor-boundary.patch",
        "patches/art/0063-darwin-arm64-type-check-boundary.patch",
        "patches/art/0064-darwin-arm64-interface-check-boundary.patch",
        "patches/art/0065-darwin-arm64-array-type-boundary.patch",
        "patches/art/0068-darwin-string-resolution-boundary.patch",
        "patches/art/0069-darwin-interface-dispatch-boundary.patch",
        "patches/art/0071-darwin-unresolved-static-field-boundary.patch",
        "patches/art/0073-darwin-polymorphic-runtime-dispatch.patch",
        "patches/art/0075-darwin-varhandle-addresses.patch",
        "patches/art/0077-darwin-varhandle-fp-acquire-scratch.patch",
        "patches/art/0079-darwin-baker-reference-window.patch",
        "patches/art/0080-darwin-baker-array-reference-window.patch",
        "patches/art/0082-darwin-fast-jit-native-root-slot-literals.patch",
        "patches/art/0083-darwin-reference-array-intermediate-address.patch",
        "patches/art/0084-darwin-baker-unresolved-fields.patch",
        "patches/art/0085-darwin-baker-gc-root-thunk-address.patch",
        "patches/art/0087-darwin-baker-unresolved-invokes.patch",
        "patches/art/0088-darwin-invoke-custom-graph.patch",
        "patches/art/0089-darwin-aosp-arm64-intrinsics.patch",
        "patches/art/0090-darwin-unsafe-get-addresses.patch",
        "patches/art/0091-darwin-unsafe-write-atomic-addresses.patch",
        "patches/art/0093-darwin-aosp-jit-admission.patch",
        "patches/art/0095-darwin-arm64-jni-handle-return.patch",
        "patches/art/0096-darwin-arm64-string-intrinsic-addresses.patch",
        "patches/art/0097-darwin-arm64-crc32-array-address.patch",
        "patches/art/0098-darwin-arm64-reference-addresses.patch",
        "patches/art/0099-darwin-arm64-vector-memory-addresses.patch",
        "patches/art/0121-darwin-arm64-jni-monitor-boundary.patch",
        "patches/art/0122-darwin-arm64-image-method-addresses.patch",
        "patches/art/0124-darwin-arm64-implicit-null-address.patch",
        "patches/art/0135-darwin-remove-optimizing-allowlist.patch",
        "patches/art/0136-darwin-remove-optimizing-allowlist-tail.patch",
        "patches/art/0127-darwin-arm64-boxing-allocation-boundary.patch",
        "patches/art/0129-darwin-arm64-baker-intermediate-array-address.patch",
        "patches/art/0131-darwin-arm64-boxing-cache-address.patch",
        "patches/art/0133-darwin-arm64-implicit-invoke-receiver.patch",
        "patches/art/0134-darwin-arm64-implicit-field-receiver.patch",
        "patches/art/0135-darwin-arm64-jni-stack-abi.patch",
        "patches/art/0136-darwin-arm64-char-arraycopy-addresses.patch",
        "patches/art/0137-darwin-arm64-frame-clinit-address.patch",
        "patches/art/0138-darwin-sharpening-boot-image-address.patch",
        "patches/art/0139-darwin-arm64-boot-literal-reference.patch",
        "patches/art/0140-darwin-arm64-reference-intrinsic-class.patch",
        "patches/art/0145-darwin-arm64-jni-method-pointer.patch",
        "patches/art/0146-darwin-arm64-managed-method-pointer.patch",
        "patches/art/0147-darwin-enable-implicit-null-checks.patch",
        "patches/art/0179-darwin-arm64-clinit-reference-boundary.patch",
        "patches/art/0180-darwin-arm64-fast-invoke-receiver-boundary.patch",
        "patches/art/0181-darwin-fast-reference-codegen-include.patch",
        "patches/art/0182-darwin-arm64-fast-field-reference-boundaries.patch",
        "patches/art/0183-darwin-arm64-fast-checkcast-reference-boundary.patch",
        "patches/art/0184-darwin-arm64-reference-referent-boundary.patch",
    ] {
        jit_inputs.push(PathBuf::from(file));
    }
    jit_inputs.sort();
    jit_inputs.dedup();
    graph.push_str("rule jit_compiler\n  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && ");
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-jit-compiler\n  description = ART ARM64 JIT compiler\n  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&jit_archive);
    graph.push(' ');
    graph.push_str(&jit_support_archive);
    graph.push_str(": jit_compiler ");
    graph.push_str(&bootstrap_cli_target);
    graph.push(' ');
    graph.push_str(&runtime_shadow_marker);
    for input in &jit_inputs {
        graph.push(' ');
        graph.push_str(&ninja_path(&root.join(input)));
    }
    graph.push_str("\nbuild jit-compiler: phony ");
    graph.push_str(&jit_archive);
    graph.push('\n');

    graph.push_str("rule runtime_owner_archive\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(
        " && cargo build -q --release --manifest-path crates/darwin-art-runtime/Cargo.toml\n",
    );
    graph.push_str("  description = Rust runtime owner archive\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&runtime_owner_archive);
    graph.push_str(": runtime_owner_archive ");
    for input in &runtime_owner_inputs {
        graph.push_str(&ninja_path(input));
        graph.push(' ');
    }
    graph.push('\n');
    graph.push_str("build runtime-owner: phony ");
    graph.push_str(&runtime_owner_archive);
    graph.push('\n');

    let hwui_cached =
        cached_foundation_objects(&native_output_root.join("hwui-static-foundation/objects"))?;
    let hwui_main = hwui_cached
        .iter()
        .filter(|object| {
            object
                .object
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.starts_with("apex_"))
        })
        .cloned()
        .collect::<Vec<_>>();
    let hwui_apex = hwui_cached
        .iter()
        .filter(|object| {
            object
                .object
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("apex_"))
        })
        .cloned()
        .collect::<Vec<_>>();
    let graphics_cached =
        cached_foundation_objects(&native_output_root.join("android-graphics-jni/objects"))?;
    let graphics_registrar = graphics_cached
        .iter()
        .filter(|object| {
            object.object.file_name().and_then(|name| name.to_str()) == Some("LayoutlibLoader.o")
        })
        .cloned()
        .collect::<Vec<_>>();
    let graphics_main = graphics_cached
        .iter()
        .filter(|object| {
            object.object.file_name().and_then(|name| name.to_str()) != Some("LayoutlibLoader.o")
        })
        .cloned()
        .collect::<Vec<_>>();
    let hwui_ready = hwui_main.len() == 81 && hwui_apex.len() == 5;
    let graphics_ready = graphics_main.len() == 66 && graphics_registrar.len() == 1;
    let icu_cached = cached_foundation_objects(&native_output_root.join("icu-foundation/objects"))?;
    let icu_common = icu_cached
        .iter()
        .filter(|object| object.object.to_string_lossy().contains("/common/"))
        .cloned()
        .collect::<Vec<_>>();
    let icu_i18n = icu_cached
        .iter()
        .filter(|object| object.object.to_string_lossy().contains("/i18n/"))
        .cloned()
        .collect::<Vec<_>>();
    let icu_stubdata = icu_cached
        .iter()
        .filter(|object| object.object.to_string_lossy().contains("/stubdata/"))
        .cloned()
        .collect::<Vec<_>>();
    let icu_init = icu_cached
        .iter()
        .filter(|object| object.object.to_string_lossy().contains("/androidicuinit/"))
        .cloned()
        .collect::<Vec<_>>();
    let icu_ready = icu_common.len() == 201
        && icu_i18n.len() == 254
        && icu_stubdata.len() == 1
        && icu_init.len() == 2;
    if hwui_ready {
        emit_cached_native_graph(
            &mut graph,
            &hwui_main,
            &hwui_foundation_archive,
            &mut cached_rules_emitted,
        );
        emit_cached_native_graph(
            &mut graph,
            &hwui_apex,
            &hwui_apex_foundation_archive,
            &mut cached_rules_emitted,
        );
    } else {
        graph.push_str("rule hwui_foundation_bootstrap\n");
        graph.push_str("  command = cd ");
        graph.push_str(&shell_quote(&root_for_shell));
        graph.push_str(" && tools/build-android16-hwui-static-foundation.sh\n");
        graph.push_str("  description = HWUI foundation archives\n");
        graph.push_str("  restat = 1\n\n");
        graph.push_str("build ");
        graph.push_str(&hwui_foundation_archive);
        graph.push(' ');
        graph.push_str(&hwui_apex_foundation_archive);
        graph.push_str(": hwui_foundation_bootstrap ");
        graph.push_str(&hwui_foundation_input_list);
        graph.push('\n');
    }

    if icu_ready {
        emit_cached_native_graph(
            &mut graph,
            &icu_common,
            &icu_common_archive,
            &mut cached_rules_emitted,
        );
        emit_cached_native_graph(
            &mut graph,
            &icu_i18n,
            &icu_i18n_archive,
            &mut cached_rules_emitted,
        );
        emit_cached_native_graph(
            &mut graph,
            &icu_stubdata,
            &icu_stubdata_archive,
            &mut cached_rules_emitted,
        );
        emit_cached_native_graph(
            &mut graph,
            &icu_init,
            &icu_init_archive,
            &mut cached_rules_emitted,
        );
    } else {
        graph.push_str("rule icu_foundation_bootstrap\n");
        graph.push_str("  command = cd ");
        graph.push_str(&shell_quote(&root_for_shell));
        graph.push_str(" && tools/build-android16-icu-foundation.sh\n");
        graph.push_str("  description = ICU foundation archives\n");
        graph.push_str("  restat = 1\n\n");
        graph.push_str("build ");
        graph.push_str(&icu_common_archive);
        graph.push(' ');
        graph.push_str(&icu_i18n_archive);
        graph.push(' ');
        graph.push_str(&icu_stubdata_archive);
        graph.push(' ');
        graph.push_str(&icu_init_archive);
        graph.push_str(": icu_foundation_bootstrap ");
        graph.push_str(&icu_foundation_input_list);
        graph.push('\n');
    }

    if graphics_ready {
        emit_cached_native_graph(
            &mut graph,
            &graphics_main,
            &graphics_jni_archive,
            &mut cached_rules_emitted,
        );
        emit_cached_native_graph(
            &mut graph,
            &graphics_registrar,
            &graphics_registrar_archive,
            &mut cached_rules_emitted,
        );
        graph.push_str("rule graphics_jni_force_load\n");
        graph.push_str("  command = ");
        graph.push_str(&shell_quote(&toolchain.cxx));
        graph.push_str(" -r -arch arm64 -Wl,-force_load,");
        graph.push_str(&shell_quote(&graphics_registrar_archive));
        graph.push_str(" -Wl,-force_load,");
        graph.push_str(&shell_quote(&graphics_jni_archive));
        graph.push_str(" -o $out\n");
        graph.push_str("  description = LINK GraphicsJNI force-load\n\n");
        graph.push_str("build ");
        graph.push_str(&graphics_force_loaded_object);
        graph.push_str(": graphics_jni_force_load ");
        graph.push_str(&graphics_jni_archive);
        graph.push(' ');
        graph.push_str(&graphics_registrar_archive);
        graph.push('\n');
    } else {
        graph.push_str("rule graphics_jni_foundation_bootstrap\n");
        graph.push_str("  command = cd ");
        graph.push_str(&shell_quote(&root_for_shell));
        graph.push_str(" && tools/build-android16-android-graphics-jni.sh --object-audit\n");
        graph.push_str("  description = GraphicsJNI foundation archives\n");
        graph.push_str("  restat = 1\n\n");
        graph.push_str("build ");
        graph.push_str(&graphics_jni_archive);
        graph.push(' ');
        graph.push_str(&graphics_registrar_archive);
        graph.push(' ');
        graph.push_str(&graphics_force_loaded_object);
        graph.push_str(": graphics_jni_foundation_bootstrap ");
        graph.push_str(&graphics_jni_foundation_input_list);
        graph.push('\n');
    }
    graph.push_str("build graphics-foundation: phony ");
    graph.push_str(&hwui_foundation_archive);
    graph.push(' ');
    graph.push_str(&hwui_apex_foundation_archive);
    graph.push(' ');
    graph.push_str(&graphics_jni_archive);
    graph.push(' ');
    graph.push_str(&graphics_registrar_archive);
    graph.push(' ');
    graph.push_str(&graphics_force_loaded_object);
    graph.push(' ');
    graph.push_str(&icu_common_archive);
    graph.push(' ');
    graph.push_str(&icu_i18n_archive);
    graph.push(' ');
    graph.push_str(&icu_stubdata_archive);
    graph.push(' ');
    graph.push_str(&icu_init_archive);
    graph.push('\n');
    graph.push_str("build icu-foundation: phony ");
    graph.push_str(&icu_common_archive);
    graph.push(' ');
    graph.push_str(&icu_i18n_archive);
    graph.push(' ');
    graph.push_str(&icu_stubdata_archive);
    graph.push(' ');
    graph.push_str(&icu_init_archive);
    graph.push('\n');
    graph.push_str("build foundation: phony graphics-foundation icu-foundation\n\n");

    // The final dylib consumes the force-linked graphics closure object, not
    // the individual archive members directly. Give that object a real
    // producer edge so a rebuilt HWUI, Skia, GraphicsJNI, or text archive can
    // never leave a stale closure in the developer inner loop.
    graph.push_str("rule graphics_runtime_closure\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && tools/audit-android16-graphics-closure.sh --art-runtime\n");
    graph.push_str("  description = LINK Android graphics runtime closure\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&graphics_runtime_closure);
    graph.push_str(": graphics_runtime_closure ");
    graph.push_str(&graphics_runtime_closure_inputs);
    graph.push('\n');

    graph.push_str("rule bionic_provider_closure\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && bash tools/build-bionic-runtime-provider-closure.sh --build-only\n");
    graph.push_str("  description = RUST/C Bionic provider closure\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&bionic_rust_provider_archive);
    graph.push(' ');
    graph.push_str(&bionic_native_provider_archive);
    graph.push(' ');
    graph.push_str(&bionic_float_provider_archive);
    graph.push(' ');
    graph.push_str(&bionic_binary128_provider_archive);
    graph.push_str(": bionic_provider_closure ");
    graph.push_str(&bionic_provider_input_list);
    let provider_foundation_inputs = [
        icu_init_archive.clone(),
        icu_common_archive.clone(),
        icu_stubdata_archive.clone(),
        ninja_path(&root.join("_build/graphics-foundations/liblog-darwin.a")),
    ];
    // These archives are read by the production symbol ownership gate, not
    // just by acceptance. ICU has producer edges above; liblog is an existing
    // graphics-foundations artifact, as on the graphics closure edge.
    for archive in &provider_foundation_inputs {
        graph.push(' ');
        graph.push_str(archive);
    }
    graph.push('\n');

    // Acceptance consumes produced archives; archives never depend backward
    // on this fixture executable. Invoke this explicitly for release testing.
    let provider_acceptance =
        ninja_path(&root.join("_build/bionic-runtime-provider-closure/acceptance.stamp"));
    graph.push_str("rule bionic_provider_acceptance\n  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(
        " && bash tools/tests/bionic-runtime-provider-closure-acceptance.sh && touch $out\n",
    );
    graph.push_str("  description = TEST produced Bionic provider archives\n\n");
    graph.push_str("build ");
    graph.push_str(&provider_acceptance);
    graph.push_str(": bionic_provider_acceptance ");
    for archive in &provider_foundation_inputs {
        graph.push_str(archive);
        graph.push(' ');
    }
    for archive in [
        &bionic_rust_provider_archive,
        &bionic_native_provider_archive,
        &bionic_float_provider_archive,
        &bionic_binary128_provider_archive,
    ] {
        graph.push_str(archive);
        graph.push(' ');
    }
    for input in &provider_manifest.audit {
        graph.push_str(&ninja_path(&root.join(input)));
        graph.push(' ');
    }
    graph.push('\n');
    graph.push_str("build bionic-provider-acceptance: phony ");
    graph.push_str(&provider_acceptance);
    graph.push('\n');

    graph.push_str("rule runtime_network_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_OUTPUT=");
    graph.push_str(&shell_quote(&network_object_for_shell));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-network-probe\n");
    graph.push_str("  description = CXX runtime_network_probe\n");
    // Dependency fingerprints are maintained by art-bootstrap.
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&network_object);
    graph.push_str(": runtime_network_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&network_probe_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&network_probe_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_hwui_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_OUTPUT=");
    graph.push_str(&shell_quote(&hwui_object_path.to_string_lossy()));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-hwui-probe\n");
    graph.push_str("  description = CXX runtime_hwui_probe\n");
    // Dependency fingerprints are maintained by art-bootstrap.
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&hwui_object);
    graph.push_str(": runtime_hwui_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&hwui_probe_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&hwui_probe_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_graphics_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_GRAPHICS_OBJECT=");
    graph.push_str(&shell_quote(&graphics_object_path.to_string_lossy()));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-graphics-probe\n");
    graph.push_str("  description = CXX runtime_graphics_probe\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&graphics_object);
    graph.push_str(": runtime_graphics_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&graphics_probe_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&graphics_probe_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_graphics_gpu_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_GRAPHICS_GPU_OBJECT=");
    graph.push_str(&shell_quote(&graphics_gpu_object_path.to_string_lossy()));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-graphics-gpu-probe\n");
    graph.push_str("  description = CXX runtime_graphics_gpu\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&graphics_gpu_object);
    graph.push_str(": runtime_graphics_gpu_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&graphics_gpu_probe_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&graphics_gpu_probe_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_graphics_phase_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_GRAPHICS_PHASE_OBJECT=");
    graph.push_str(&shell_quote(&graphics_phase_object_path.to_string_lossy()));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-graphics-phase-probe\n");
    graph.push_str("  description = CXX runtime_graphics_phase\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&graphics_phase_object);
    graph.push_str(": runtime_graphics_phase_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&graphics_phase_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&graphics_phase_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_graphics_input_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_GRAPHICS_INPUT_OBJECT=");
    graph.push_str(&shell_quote(&graphics_input_object_path.to_string_lossy()));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-graphics-input-probe\n");
    graph.push_str("  description = CXX runtime_graphics_input\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&graphics_input_object);
    graph.push_str(": runtime_graphics_input_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&graphics_input_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&graphics_input_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_graphics_state_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_GRAPHICS_STATE_OBJECT=");
    graph.push_str(&shell_quote(&graphics_state_object_path.to_string_lossy()));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-graphics-state\n");
    graph.push_str("  description = CXX runtime_graphics_state\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&graphics_state_object);
    graph.push_str(": runtime_graphics_state_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&graphics_state_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&graphics_state_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_graphics_session\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_GRAPHICS_SESSION_OBJECT=");
    graph.push_str(&shell_quote(
        &graphics_session_object_path.to_string_lossy(),
    ));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-graphics-session\n");
    graph.push_str("  description = CXX runtime_graphics_session\n");
    graph.push_str("  restat = 1\n\n");
    let graphics_session_object = ninja_path(&graphics_session_object_path);
    graph.push_str("build ");
    graph.push_str(&graphics_session_object);
    graph.push_str(": runtime_graphics_session ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&graphics_session_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&graphics_session_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_jni_acceptance_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_JNI_ACCEPTANCE_OBJECT=");
    graph.push_str(&shell_quote(&jni_acceptance_object_path.to_string_lossy()));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-jni-acceptance-probe\n");
    graph.push_str("  description = CXX runtime_jni_acceptance\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&jni_acceptance_object);
    graph.push_str(": runtime_jni_acceptance_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&jni_acceptance_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&jni_acceptance_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_app_bootstrap_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_APP_BOOTSTRAP_OBJECT=");
    graph.push_str(&shell_quote(&app_bootstrap_object_path.to_string_lossy()));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-app-bootstrap-probe\n");
    graph.push_str("  description = CXX runtime_app_bootstrap\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&app_bootstrap_object);
    graph.push_str(": runtime_app_bootstrap_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&app_bootstrap_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&app_bootstrap_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_app_resources_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_APP_RESOURCES_OBJECT=");
    graph.push_str(&shell_quote(&app_resources_object_path.to_string_lossy()));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-app-resources-probe\n");
    graph.push_str("  description = CXX runtime_app_resources\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&app_resources_object);
    graph.push_str(": runtime_app_resources_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&app_resources_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&app_resources_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_app_activity_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_APP_ACTIVITY_OBJECT=");
    graph.push_str(&shell_quote(&app_activity_object_path.to_string_lossy()));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-app-activity-probe\n");
    graph.push_str("  description = CXX runtime_app_activity\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&app_activity_object);
    graph.push_str(": runtime_app_activity_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push_str(&app_activity_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&app_activity_stamp));
    graph.push('\n');
    graph.push_str("rule runtime_app_presentation_probe\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_APP_PRESENTATION_OBJECT=");
    graph.push_str(&shell_quote(
        &app_presentation_object_path.to_string_lossy(),
    ));
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" build-runtime-app-presentation-probe\n");
    graph.push_str("  description = CXX runtime_app_presentation\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&app_presentation_object);
    graph.push_str(": runtime_app_presentation_probe ");
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    graph.push(' ');
    graph.push_str(&app_presentation_inputs);
    graph.push(' ');
    graph.push_str(&ninja_path(&app_presentation_stamp));
    graph.push('\n');
    // Keep the libcore Linux archive on the same incremental path as its
    // source and lockfile. The graphics audit consumes this archive directly,
    // so treating it as an ambient prebuilt lets a changed compatibility TU
    // silently leave the final dylib linked against stale JNI code.
    graph.push_str("rule libcore_linux_archive\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && tools/build-android16-libcore-darwin-linux.sh --archive-only\n");
    graph.push_str("  description = libcore Linux compatibility archive\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&libcore_linux_archive);
    graph.push_str(" | ");
    for output in [
        "_build/libcore-darwin-linux/darwin_linux_method_table.inc",
        "_build/libcore-darwin-linux/libcore-darwin-linux-methods.tsv",
        "_build/libcore-darwin-linux/libcore-darwin-linux-unsupported-abi.tsv",
        "_build/libcore-darwin-linux/libcore-darwin-linux-counts.txt",
        "_build/libcore-darwin-linux/libcore-darwin-linux-definitions.txt",
        "_build/os-constants/libandroid-system-os-constants-darwin.a",
        "_build/os-constants/android16-os-constants-values.tsv",
        "_build/os-constants/generated/android16_os_constants_values.inc",
        "_build/os-constants/generated/android16_os_constants_errno.inc",
        "_build/os-constants/generated/android16_os_constants_sysconf.inc",
        "_build/os-constants/generated/names.txt",
        "_build/os-constants/generated/expressions.tsv",
        "_build/os-constants/generated/derived-values.tsv",
        "_build/asynchronous-close-monitor/libandroidio-darwin.a",
        "_build/asynchronous-close-monitor/libcore-io-asynchronous-close-monitor-registrar-darwin.a",
    ] {
        graph.push_str(&ninja_path(&root.join(output)));
        graph.push(' ');
    }
    graph.push_str(": libcore_linux_archive ");
    for input in [
        "tools/build-android16-libcore-darwin-linux.sh",
        "upstream/android16-libcore-darwin-linux.lock",
        "compat/libcore_darwin_linux.cc",
        "compat/libcore_darwin_linux_system_natives.cc",
        "compat/libcore_darwin_linux_syscalls.cc",
        "compat/libcore_darwin_linux.h",
        "compat/darwin_dns_hints.h",
        "compat/darwin_os_constants.h",
        "compat/AsynchronousCloseMonitor.h",
        "compat/darwin_asynchronous_close_monitor.cc",
        "tools/bionic-socket-broker-adapter/include/darwin_art_bionic_socket_broker.h",
        "tools/build-android16-asynchronous-close-monitor.sh",
        "upstream/android16-asynchronous-close-monitor.lock",
        "tools/build-android16-os-constants-darwin.sh",
        "compat/darwin_os_constants.cc",
        "upstream/android16-os-constants.lock",
        "upstream/android16-os-constants-values.tsv",
    ] {
        graph.push_str(&ninja_path(&root.join(input)));
        graph.push(' ');
    }
    graph.push('\n');
    let libcore_test_stamp = ninja_path(&root.join("_build/libcore-darwin-linux/acceptance.stamp"));
    graph.push_str("rule libcore_linux_test\n  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(
        " && bash tools/test-android16-libcore-darwin-linux.sh --prepared && touch $out\n",
    );
    graph.push_str("  description = Libcore Linux native/managed ABI acceptance\n\n");
    graph.push_str("build ");
    graph.push_str(&libcore_test_stamp);
    graph.push_str(": libcore_linux_test ");
    graph.push_str(&libcore_linux_archive);
    graph.push(' ');
    for input in [
        "tools/test-android16-libcore-darwin-linux.sh",
        "upstream/android16-libcore-darwin-linux.lock",
        "probes/android16_libcore_darwin_linux_smoke.cc",
        "_build/libcore-darwin-linux/darwin_linux_method_table.inc",
        "_build/libcore-darwin-linux/libcore-darwin-linux-methods.tsv",
        "_build/libcore-darwin-linux/libcore-darwin-linux-unsupported-abi.tsv",
        "_build/libcore-darwin-linux/libcore-darwin-linux-counts.txt",
        "compat/libcore_darwin_linux.cc",
        "compat/libcore_darwin_linux_system_natives.cc",
        "compat/libcore_darwin_linux_syscalls.cc",
        "compat/libcore_darwin_linux.h",
        "compat/darwin_os_constants.h",
        "_build/asynchronous-close-monitor/libandroidio-darwin.a",
        "_build/nativehelper-foundation/libnativehelper_jvm.a",
        "_build/graphics-foundations/liblog-darwin.a",
        "_build/os-constants/libandroid-system-os-constants-darwin.a",
    ] {
        graph.push_str(&ninja_path(&root.join(input)));
        graph.push(' ');
    }
    graph.push_str("\nbuild libcore-linux-test: phony ");
    graph.push_str(&libcore_test_stamp);
    graph.push('\n');
    let os_test_stamp = ninja_path(&root.join("_build/os-constants/acceptance.stamp"));
    graph.push_str("rule os_constants_test\n  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(
        " && bash tools/test-android16-os-constants-darwin.sh --prepared && touch $out\n",
    );
    graph.push_str("  description = OsConstants managed Android ABI acceptance\n\n");
    graph.push_str("build ");
    graph.push_str(&os_test_stamp);
    graph.push_str(": os_constants_test ");
    for input in [
        "tools/test-android16-os-constants-darwin.sh",
        "upstream/android16-os-constants.lock",
        "probes/android16_os_constants_jni.cc",
        "compat/darwin_os_constants.h",
        "_build/os-constants/libandroid-system-os-constants-darwin.a",
        "_build/os-constants/android16-os-constants-values.tsv",
        "_build/os-constants/generated/android16_os_constants_values.inc",
        "_build/os-constants/generated/android16_os_constants_errno.inc",
        "_build/os-constants/generated/android16_os_constants_sysconf.inc",
        "_build/os-constants/generated/names.txt",
        "_build/os-constants/generated/expressions.tsv",
        "_build/os-constants/generated/derived-values.tsv",
        "_build/nativehelper-foundation/libnativehelper_jvm.a",
        "_build/graphics-foundations/liblog-darwin.a",
    ] {
        graph.push_str(&ninja_path(&root.join(input)));
        graph.push(' ');
    }
    graph.push_str("\nbuild os-constants-test: phony ");
    graph.push_str(&os_test_stamp);
    graph.push('\n');
    let async_test_stamp =
        ninja_path(&root.join("_build/asynchronous-close-monitor/acceptance.stamp"));
    graph.push_str("rule async_close_test\n  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(
        " && bash tools/test-android16-asynchronous-close-monitor.sh --prepared && touch $out\n",
    );
    graph.push_str("  description = Asynchronous close managed/native acceptance\n\n");
    graph.push_str("build ");
    graph.push_str(&async_test_stamp);
    graph.push_str(": async_close_test ");
    for input in [
        "_build/asynchronous-close-monitor/libandroidio-darwin.a",
        "_build/asynchronous-close-monitor/libcore-io-asynchronous-close-monitor-registrar-darwin.a",
        "tools/test-android16-asynchronous-close-monitor.sh",
        "upstream/android16-asynchronous-close-monitor.lock",
        "probes/android16_asynchronous_close_monitor_smoke.cc",
        "probes/android16_asynchronous_close_monitor_jni.cc",
        "_build/nativehelper-foundation/libnativehelper_jvm.a",
        "_build/graphics-foundations/liblog-darwin.a",
    ] {
        graph.push_str(&ninja_path(&root.join(input)));
        graph.push(' ');
    }
    graph.push_str("\nbuild asynchronous-close-monitor-test: phony ");
    graph.push_str(&async_test_stamp);
    graph.push('\n');
    graph.push_str("rule unix_filesystem_archive\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && tools/build-android16-unix-filesystem-darwin.sh --archive-only\n");
    graph.push_str("  description = Unix filesystem compatibility archive\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&unix_filesystem_archive);
    graph.push_str(": unix_filesystem_archive ");
    for input in [
        "tools/build-android16-unix-filesystem-darwin.sh",
        "upstream/android16-unix-filesystem-darwin.lock",
        "compat/darwin_libcore_filesystem_bridge.c",
        "compat/darwin_libcore_filesystem_bridge.h",
        "compat/darwin_openjdk_nio_copy.c",
        "compat/darwin_openjdk_nio_fs_redirect.h",
        "tools/bionic-fs-facade/include/darwin_art_bionic_fs.h",
        "tools/bionic-fs-facade/include/darwin_art_bionic_stat.h",
        "tools/bionic-ioctl-facade/include/darwin_art_bionic_ioctl.h",
        "tools/bionic-errno-tls/include/darwin_art_bionic_errno.h",
    ] {
        graph.push_str(&ninja_path(&root.join(input)));
        graph.push(' ');
    }
    graph.push('\n');
    // Managed fixtures audit the produced archive, never become prerequisites
    // of it. Keep this independently requestable gate in the same graph.
    let unix_filesystem_test_stamp =
        ninja_path(&root.join("_build/unix-filesystem-darwin/managed-acceptance.stamp"));
    graph.push_str("rule unix_filesystem_managed_test\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(
        " && bash tools/test-android16-unix-filesystem-darwin.sh --prepared && touch $out\n",
    );
    graph.push_str("  description = Unix filesystem managed fixture acceptance\n\n");
    graph.push_str("build ");
    graph.push_str(&unix_filesystem_test_stamp);
    graph.push_str(": unix_filesystem_managed_test ");
    graph.push_str(&unix_filesystem_archive);
    graph.push(' ');
    for input in [
        "tools/test-android16-unix-filesystem-darwin.sh",
        "probes/android16_unix_filesystem_jni.c",
        "probes/unix-filesystem/UnixFileSystemDarwinSmoke.java",
        "_build/nativehelper-device-foundation/libnativehelper-device-darwin.a",
        "_build/graphics-foundations/liblog-darwin.a",
    ] {
        graph.push_str(&ninja_path(&root.join(input)));
        graph.push(' ');
    }
    graph.push_str("\nbuild unix-filesystem-managed-test: phony ");
    graph.push_str(&unix_filesystem_test_stamp);
    graph.push('\n');
    graph.push_str("rule graphics_audit\n");
    graph.push_str("  pool = runtime_bootstrap_fallback\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && DARWIN_ART_NATIVE_OUTPUT_ROOT=");
    graph.push_str(&shell_quote(&native_output_for_shell));
    graph.push_str(" DARWIN_ART_NATIVE_GRAPHICS_STATE_OBJECT=");
    graph.push_str(&shell_quote(&graphics_state_object_path.to_string_lossy()));
    graph.push_str(" DARWIN_ART_NATIVE_GRAPHICS_SESSION_OBJECT=");
    graph.push_str(&shell_quote(
        &graphics_session_object_path.to_string_lossy(),
    ));
    // The full upstream closure is a separate release/CI gate.  The Ninja
    // graph is the developer inner loop and must only relink/audit against
    // already materialized foundation artifacts.
    graph.push(' ');
    graph.push_str(&bootstrap_cli);
    graph.push_str(" audit-runtime-graphics-link-fast\n");
    graph.push_str("  description = GRAPHICS link/audit\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&runtime_library);
    graph.push_str(": graphics_audit ");
    graph.push_str(&input_keymaps_archive);
    graph.push(' ');
    append_bootstrap_cli_prerequisite(&mut graph, &bootstrap_cli_target);
    // The fast audit consumes these foundation artifacts directly.  Keep them
    // as real Ninja prerequisites so a missing or rebuilt foundation cannot
    // race the final dylib link/audit edge.
    graph.push_str(&graphics_runtime_closure);
    graph.push(' ');
    graph.push_str(&surfaceflinger_frontend_archive);
    graph.push(' ');
    graph.push_str(&surfaceflinger_binder_archive);
    graph.push(' ');
    graph.push_str(&surfaceflinger_gui_archive);
    graph.push(' ');
    graph.push_str(&surfaceflinger_fence_archive);
    graph.push(' ');
    graph.push_str(&hwui_foundation_archive);
    graph.push(' ');
    graph.push_str(&hwui_apex_foundation_archive);
    graph.push(' ');
    graph.push_str(&graphics_jni_archive);
    graph.push(' ');
    graph.push_str(&graphics_registrar_archive);
    graph.push(' ');
    graph.push_str(&graphics_force_loaded_object);
    graph.push(' ');
    graph.push_str(&icu_common_archive);
    graph.push(' ');
    graph.push_str(&icu_i18n_archive);
    graph.push(' ');
    graph.push_str(&icu_stubdata_archive);
    graph.push(' ');
    graph.push_str(&icu_init_archive);
    graph.push(' ');
    graph.push_str(&archive);
    graph.push(' ');
    graph.push_str(&interpreter_archive);
    graph.push(' ');
    graph.push_str(&jit_archive);
    graph.push(' ');
    graph.push_str(&jit_support_archive);
    graph.push(' ');
    graph.push_str(&libcore_linux_archive);
    graph.push(' ');
    graph.push_str(&unix_filesystem_archive);
    graph.push(' ');
    graph.push_str(&system_natives_archive);
    graph.push(' ');
    graph.push_str(&boringssl_archive);
    graph.push(' ');
    graph.push_str(&graphics_state_object);
    graph.push(' ');
    graph.push_str(&graphics_session_object);
    graph.push(' ');
    graph.push_str(&runtime_owner_archive);
    graph.push(' ');
    graph.push_str(&bionic_rust_provider_archive);
    graph.push(' ');
    graph.push_str(&bionic_native_provider_archive);
    graph.push(' ');
    graph.push_str(&bionic_float_provider_archive);
    graph.push(' ');
    graph.push_str(&bionic_binary128_provider_archive);
    graph.push(' ');
    graph.push_str(&ninja_path(&runtime_entry_stamp));
    graph.push('\n');
    graph.push_str("build graphics-audit: phony ");
    graph.push_str(&bootstrap_cli_target);
    graph.push(' ');
    graph.push_str(&runtime_library);
    graph.push_str(" jit-layout-audit\n\n");
    graph.push_str("rule jit_layout_audit\n");
    graph.push_str("  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(" && tools/audit-jit-layout.sh && touch ");
    graph.push_str(&shell_quote(&jit_layout_stamp_path.to_string_lossy()));
    graph.push('\n');
    graph.push_str("  description = JIT metadata-first MAP_JIT layout smoke\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build jit-layout-audit: phony ");
    graph.push_str(&jit_layout_stamp);
    graph.push_str("\n");
    graph.push_str("build ");
    graph.push_str(&jit_layout_stamp);
    graph.push_str(": jit_layout_audit ");
    graph.push_str(&ninja_path(&root.join("tools/audit-jit-layout.sh")));
    graph.push(' ');
    graph.push_str(&ninja_path(&root.join("tools/jit-layout-smoke.cc")));
    graph.push(' ');
    graph.push_str(&ninja_path(
        &root.join("config/darwin-art-host.entitlements"),
    ));
    graph.push('\n');
    graph.push_str("\nrule surfaceflinger_core\n");
    graph.push_str("  command = ");
    graph.push_str(&shell_quote(&format!(
        "{root_for_shell}/tools/build-android16-surfaceflinger-core.sh"
    )));
    graph.push_str(" --archive-only\n  description = AOSP SurfaceFlinger frontend (Darwin)\n");
    graph.push_str("  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&surfaceflinger_frontend_archive);
    graph.push_str(" | ");
    for output in [
        &surfaceflinger_binder_archive,
        &surfaceflinger_gui_archive,
        &surfaceflinger_fence_archive,
    ] {
        graph.push_str(output);
        graph.push(' ');
    }
    graph.push_str(": surfaceflinger_core ");
    for input in [
        "tools/build-android16-surfaceflinger-core.sh",
        "tools/lib/surfaceflinger-compile-flags.sh",
        "tools/sync-android16-surfaceflinger-core.sh",
        "tools/sync-android16-hostgraphics.sh",
        "upstream/android16-surfaceflinger-core.lock",
        "patches/frameworks-native/0001-darwin-surfaceflinger-core.patch",
        "patches/frameworks-native/0002-darwin-surface-commit-wait.patch",
        "patches/frameworks-native/0003-buffer-release-message-length.patch",
        "patches/frameworks-native/0004-darwin-release-record-transport.patch",
        "patches/frameworks-native/0005-darwin-binder-trigger-poll.patch",
        "patches/frameworks-native/0006-darwin-binder-rpc-peer-identity.patch",
        "patches/frameworks-native/0007-rpc-binder-death-log-handle.patch",
        "patches/frameworks-native/0008-darwin-binder-platform-syscalls.patch",
        "patches/frameworks-native/0009-darwin-binder-fd-namespace.patch",
        "compat/surfaceflinger/release_record_transport.cc",
        "compat/surfaceflinger/release_record_transport.h",
        "compat/surfaceflinger/commit_signal.h",
        "compat/surfaceflinger/tracing_perfetto.h",
        "compat/surfaceflinger/transaction_bridge.cc",
        "compat/surfaceflinger/transaction_bridge.h",
        "compat/surfaceflinger/binder_os_darwin.cc",
        "compat/surfaceflinger/binder_socket_darwin.h",
        "compat/surfaceflinger/endian.h",
        "compat/surfaceflinger/fence_sync_darwin.cc",
        "compat/surfaceflinger/sync/sync.h",
    ] {
        graph.push_str(&ninja_path(&root.join(input)));
        graph.push(' ');
    }
    graph.push('\n');
    graph.push_str("build surfaceflinger-core: phony ");
    for output in [
        &surfaceflinger_frontend_archive,
        &surfaceflinger_binder_archive,
        &surfaceflinger_gui_archive,
        &surfaceflinger_fence_archive,
    ] {
        graph.push_str(output);
        graph.push(' ');
    }
    graph.push('\n');
    let surfaceflinger_test_stamp =
        ninja_path(&root.join("_build/surfaceflinger-core/acceptance.stamp"));
    graph.push_str("rule surfaceflinger_core_test\n  command = cd ");
    graph.push_str(&shell_quote(&root_for_shell));
    graph.push_str(
        " && bash tools/test-android16-surfaceflinger-core.sh --prepared && touch $out\n",
    );
    graph.push_str("  description = SurfaceFlinger transaction/Parcel fixture acceptance\n\n");
    graph.push_str("build ");
    graph.push_str(&surfaceflinger_test_stamp);
    graph.push_str(" | ");
    graph.push_str(&surfaceflinger_runtime_probe);
    graph.push_str(": surfaceflinger_core_test ");
    for archive in [
        &surfaceflinger_frontend_archive,
        &surfaceflinger_binder_archive,
        &surfaceflinger_gui_archive,
        &surfaceflinger_fence_archive,
    ] {
        graph.push_str(archive);
        graph.push(' ');
    }
    for input in [
        "tools/test-android16-surfaceflinger-core.sh",
        "tools/lib/surfaceflinger-compile-flags.sh",
        "upstream/android16-surfaceflinger-core.lock",
        "tools/test-release-record-transport.sh",
        "tools/tests/release-record-transport-test.cc",
        "tools/tests/parcel-native-handle-test.cc",
        "probes/surfaceflinger_transaction_handler_compile.cc",
        "probes/surfaceflinger_transaction_handler_runtime.cc",
        "compat/binder/rpc_identity.cc",
        "compat/binder/calling_identity.cc",
        "compat/binder/peer_credentials.cc",
        "tools/tests/surfaceflinger-binder-platform-syscalls-fixture.cc",
        "tools/tests/surfaceflinger-binder-process-registry-fixture.cc",
        "_build/ui-types-foundation/libui-types.a",
        "_build/graphics-foundations/libutils-darwin.a",
        "_build/graphics-foundations/libcutils-darwin.a",
        "_build/graphics-foundations/liblog-darwin.a",
        "_build/libbase-foundation/libandroid-base-darwin.a",
    ] {
        graph.push_str(&ninja_path(&root.join(input)));
        graph.push(' ');
    }
    graph.push_str("\nbuild surfaceflinger-core-test: phony ");
    graph.push_str(&surfaceflinger_test_stamp);
    graph.push('\n');
    graph.push_str("build graph-input-digest: phony ");
    graph.push_str(&ninja_path(&digest_path));
    graph.push('\n');

    emit_representative_edges(&mut graph, &root, &cache_dir, &toolchain)?;
    if with_support {
        super::runtime_support::emit(&mut graph, &root)?;
    }

    atomic::write(out, graph.as_bytes())?;
    println!(
        "darwin-art-xtask: wrote {} (inputs={} digest={digest})",
        out.display(),
        inputs.len()
    );
    Ok(())
}
