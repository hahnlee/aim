//! Owning graph operations versus a read-only selected-image source.
use super::*;

pub(super) fn lock_graph(
    handle: &DarwinArtElfGraphHandle,
) -> Result<MutexGuard<'_, LoadedElfGraph>, FfiFailure> {
    match &handle.owner {
        GraphHandleOwner::Graph(graph) => graph.lock().map_err(|_| FfiFailure::Poisoned),
        GraphHandleOwner::Selected(_) => Err(FfiFailure::Invalid(
            "selected source is not an owning graph",
        )),
    }
}

pub(super) fn select_graph_image(
    handle: &DarwinArtElfGraphHandle,
    soname: &str,
) -> Result<crate::GlobalElfImage, FfiFailure> {
    match &handle.owner {
        GraphHandleOwner::Selected(image) if image.soname() == soname => Ok(image.clone()),
        GraphHandleOwner::Selected(_) => {
            Err(FfiFailure::Invalid("image is outside selected source"))
        }
        GraphHandleOwner::Graph(_) => lock_graph(handle)?
            .global_image(soname)
            .ok_or(FfiFailure::Invalid("missing selected image")),
    }
}
