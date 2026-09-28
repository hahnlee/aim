//! Stable graph identity and cache metadata.
//!
//! Keeping this work out of `emit_graph` is intentional: changing the Ninja
//! edge layout must not accidentally change how the input closure or cache
//! namespace is computed.  The returned manifest is immutable for one graph
//! emission and is consumed by the edge emitter below.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::super::{digest_inputs, repository_root};
use super::GRAPH_VERSION;
use super::atomic;
use super::inputs::{graph_inputs, is_global_digest_excluded};
use super::representative::{
    REPRESENTATIVE_EDGES, ToolchainInputs, edge_digest, json_escape, toolchain_inputs,
};

pub(crate) struct GraphManifest {
    pub(crate) root: PathBuf,
    pub(crate) inputs: Vec<PathBuf>,
    pub(crate) bootstrap_inputs: Vec<PathBuf>,
    pub(crate) digest: String,
    pub(crate) toolchain: ToolchainInputs,
    pub(crate) cache_dir: PathBuf,
    pub(crate) digest_path: PathBuf,
}

pub(crate) fn prepare(out: &Path) -> io::Result<GraphManifest> {
    let root = repository_root(out);
    let inputs = graph_inputs(&root);
    // Fixtures never enter product archives. Product leaf sources also have
    // narrower compile/link edges than the shared bootstrap archives.
    let bootstrap_inputs = inputs
        .iter()
        .filter(|path| !is_global_digest_excluded(path))
        .cloned()
        .collect::<Vec<_>>();
    let digest = digest_inputs(&root, &bootstrap_inputs)?;
    let toolchain = toolchain_inputs();
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_dir = root.join("_build/native-cache").join(&digest);
    fs::create_dir_all(&cache_dir)?;
    let digest_path = cache_dir.join("inputs.sha256");
    atomic::write(
        &digest_path,
        format!("{digest}  {GRAPH_VERSION}\n").as_bytes(),
    )?;
    let edges = REPRESENTATIVE_EDGES
        .iter()
        .map(|edge| {
            format!(
                "{{\"name\":\"{}\",\"digest\":\"{}\",\"source\":\"{}\",\"language\":\"{}\"}}",
                edge.name,
                edge_digest(&root, edge, &toolchain).unwrap_or_else(|_| "missing".into()),
                edge.source,
                if edge.objc { "objc++" } else { "c++" }
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    atomic::write(
        &cache_dir.join("manifest.json"),
        format!(
            "{{\"graph_version\":\"{GRAPH_VERSION}\",\"digest\":\"{digest}\",\"input_count\":{},\"compiler\":\"{}\",\"sdk\":\"{}\",\"edges\":[{edges}]}}\n",
            bootstrap_inputs.len(),
            json_escape(&toolchain.cxx),
            json_escape(&toolchain.sdkroot),
        )
        .as_bytes(),
    )?;

    Ok(GraphManifest {
        root,
        inputs,
        bootstrap_inputs,
        digest,
        toolchain,
        cache_dir,
        digest_path,
    })
}
