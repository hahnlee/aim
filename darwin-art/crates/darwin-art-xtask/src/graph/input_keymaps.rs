use std::path::{Path, PathBuf};

use super::super::{ninja_path, shell_quote};
use super::inputs::collect_files;

pub(crate) fn inputs(root: &Path) -> Vec<PathBuf> {
    let mut paths = [
        "upstream/android16-key-character-map.lock",
        "tools/materialize-android16-key-character-map.sh",
        "tools/build-android16-input-keymaps.sh",
        "tools/lib/key-character-map-compile-context.sh",
        "tools/lib/key-character-map-jni-compile-context.sh",
        "patches/frameworks-base/0009-darwin-key-character-map-factory-handoff.patch",
        "tools/lib/surfaceflinger-compile-flags.sh",
        "runtime/framework/input/system_keyboard_maps_jni.cc",
        "runtime/framework/input/system_keyboard_maps_jni.h",
        "compat/filesystem/guest_config.h",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect::<Vec<_>>();
    collect_files(
        &root.join("_aosp/android16-key-character-map"),
        root,
        &mut paths,
    );
    paths.sort();
    paths.dedup();
    paths
}

pub(crate) fn emit(graph: &mut String, root: &Path) -> String {
    let archive =
        ninja_path(&root.join("_build/android16-input-keymaps/libandroid-input-keymaps.a"));
    graph.push_str(
        "rule input_keymaps_archive\n  pool = runtime_bootstrap_fallback\n  command = cd ",
    );
    graph.push_str(&shell_quote(&root.to_string_lossy()));
    graph.push_str(" && bash tools/build-android16-input-keymaps.sh\n");
    graph.push_str("  description = AOSP input keymaps and JNI archive\n  restat = 1\n\n");
    graph.push_str("build ");
    graph.push_str(&archive);
    graph.push_str(": input_keymaps_archive ");
    for input in inputs(root) {
        graph.push_str(&ninja_path(&root.join(input)));
        graph.push(' ');
    }
    graph.push('\n');
    archive
}
