use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io;
use std::path::Path;

use super::super::{CXX_FLAGS, ninja_path, shell_quote};
use super::GRAPH_VERSION;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RepresentativeEdge {
    pub(crate) name: &'static str,
    pub(crate) source: &'static str,
    pub(crate) headers: &'static [&'static str],
    pub(crate) objc: bool,
}

pub(crate) const REPRESENTATIVE_EDGES: &[RepresentativeEdge] = &[
    RepresentativeEdge {
        name: "elf-image-registry",
        source: "compat/darwin_android_elf_image_registry.cc",
        headers: &[
            "compat/darwin_android_elf_image_registry.h",
            "crates/aim-elf-loader/include/aim_elf_loader.h",
            "tools/android-dl-iterate-phdr-provider/include/aim_dl_iterate_phdr.h",
        ],
        objc: false,
    },
    RepresentativeEdge {
        name: "asynchronous-close-monitor",
        source: "compat/darwin_asynchronous_close_monitor.cc",
        headers: &["compat/AsynchronousCloseMonitor.h"],
        objc: false,
    },
    RepresentativeEdge {
        name: "android-base-logging",
        source: "compat/android_base_logging.cc",
        headers: &[
            "compat/log/log.h",
            "_aosp/system/libbase/include/android-base/logging.h",
        ],
        objc: false,
    },
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ToolchainInputs {
    pub(crate) cxx: String,
    pub(crate) sdkroot: String,
}

pub(crate) fn toolchain_inputs() -> ToolchainInputs {
    ToolchainInputs {
        cxx: env::var("AIM_CXX")
            .or_else(|_| env::var("CXX"))
            .unwrap_or_else(|_| "clang++".to_owned()),
        sdkroot: env::var("AIM_SDKROOT")
            .or_else(|_| env::var("SDKROOT"))
            .unwrap_or_default(),
    }
}

pub(crate) fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

pub(crate) fn edge_digest(
    root: &Path,
    edge: &RepresentativeEdge,
    toolchain: &ToolchainInputs,
) -> io::Result<String> {
    let mut digest = Sha256::new();
    digest.update(GRAPH_VERSION.as_bytes());
    digest.update(edge.name.as_bytes());
    digest.update(edge.source.as_bytes());
    digest.update(toolchain.cxx.as_bytes());
    digest.update(toolchain.sdkroot.as_bytes());
    for flag in CXX_FLAGS {
        digest.update(flag.as_bytes());
        digest.update([0]);
    }
    for path in std::iter::once(edge.source).chain(edge.headers.iter().copied()) {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(fs::read(root.join(path))?);
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn emit_representative_edges(
    graph: &mut String,
    root: &Path,
    cache_dir: &Path,
    toolchain: &ToolchainInputs,
) -> io::Result<()> {
    // These edges are deliberately separate from the ART bootstrap archive:
    // they are small, independently cacheable production-side TUs and make
    // the graph's object ownership observable without duplicating the 200+
    // upstream ART objects in the transitional archive builder.
    let object_dir = cache_dir.join("native-tu");
    graph.push_str("rule native_representative_cpp\n");
    graph.push_str("  command = mkdir -p ");
    graph.push_str(&shell_quote(&object_dir.to_string_lossy()));
    graph.push_str(" && ");
    graph.push_str(&shell_quote(&toolchain.cxx));
    for flag in CXX_FLAGS {
        graph.push(' ');
        graph.push_str(flag);
    }
    for include in [
        "compat",
        "include",
        "crates/aim-elf-loader/include",
        "_aosp/art/runtime",
        "_aosp/art/libartbase",
        "_build/foundation/patched-source/libartbase",
        "tools/android-dl-iterate-phdr-provider/include",
        "_aosp/system/libbase/include",
        "_aosp/libnativehelper/include_jni",
    ] {
        let include_path = root.join(include);
        if include_path.is_dir() {
            graph.push_str(" -I");
            graph.push_str(&shell_quote(&include_path.to_string_lossy()));
        }
    }
    if !toolchain.sdkroot.is_empty() {
        graph.push_str(" -isysroot ");
        graph.push_str(&shell_quote(&toolchain.sdkroot));
    }
    graph.push_str(" -MMD -MF $depfile -c $in -o $out\n");
    graph.push_str("  depfile = $out.d\n");
    graph.push_str("  deps = gcc\n");
    graph.push_str("  description = CXX $out\n\n");

    let mut outputs = Vec::new();
    for edge in REPRESENTATIVE_EDGES {
        let digest = edge_digest(root, edge, toolchain)?;
        let object = object_dir.join(format!("{}-{digest}.o", edge.name));
        let output = ninja_path(&object);
        let source = ninja_path(&root.join(edge.source));
        graph.push_str("build ");
        graph.push_str(&output);
        graph.push_str(": native_representative_cpp ");
        graph.push_str(&source);
        for header in edge.headers {
            graph.push(' ');
            graph.push_str(&ninja_path(&root.join(header)));
        }
        graph.push('\n');
        outputs.push(output);
    }
    graph.push_str("build native-tu-preflight: phony ");
    graph.push_str(&outputs.join(" "));
    graph.push('\n');
    Ok(())
}
