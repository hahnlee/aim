use std::env;
use std::path::{Path, PathBuf};

/// Output paths owned by one native bootstrap action.
///
/// The default remains the historical `_build` tree. A graph action can set
/// `AIM_NATIVE_OUTPUT_ROOT` to put its outputs (including dependency
/// fingerprints) in a content-addressed directory without changing the
/// source or runtime paths used as inputs.
#[derive(Debug, Clone)]
pub(crate) struct BuildPaths {
    native_output_root: PathBuf,
}

impl BuildPaths {
    pub(crate) fn from_root(root: &Path) -> Self {
        let native_output_root = env::var_os("AIM_NATIVE_OUTPUT_ROOT")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                }
            })
            .unwrap_or_else(|| root.join("_build"));
        Self { native_output_root }
    }

    pub(crate) fn native_output(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.native_output_root.join(relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_paths_are_rooted_under_the_selected_directory() {
        let paths = BuildPaths {
            native_output_root: PathBuf::from("/repo/_build"),
        };
        assert_eq!(
            paths.native_output("runtime-graphics-bootstrap/objects"),
            PathBuf::from("/repo/_build/runtime-graphics-bootstrap/objects")
        );
    }
}
