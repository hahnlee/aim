//! File admission plan below NativeLoader policy. No global directory fallback
//! and no recursive re-export through namespace links.
use super::*;
use std::fs::File;
use std::io;

pub struct NamespaceSearchPlan {
    scopes: Vec<(NamespaceId, NamespaceConfig, Vec<PathBuf>)>,
}

pub struct OpenedNamespaceFile {
    pub namespace: NamespaceId,
    pub canonical_path: PathBuf,
    pub file: File,
}

impl NamespaceSearchPlan {
    pub(super) fn new(
        scopes: Vec<(NamespaceId, NamespaceConfig)>,
        soname: &str,
        runpath: &[PathBuf],
    ) -> Result<Self, NamespaceError> {
        let scopes = scopes
            .into_iter()
            .map(|(id, config)| {
                let paths = config.search_candidates(soname, runpath)?;
                Ok((id, config, paths))
            })
            .collect::<Result<_, NamespaceError>>()?;
        Ok(Self { scopes })
    }

    /// The opener must return a canonical guest path and its already-open file
    /// as one admission. It must not resolve via host-global search. NotFound
    /// tries the next candidate; other I/O failures propagate. Rejected leases
    /// drop before the next attempt. Resident-image lookup precedes this API.
    pub fn open<F>(&self, mut opener: F) -> io::Result<Option<OpenedNamespaceFile>>
    where
        F: FnMut(&Path) -> io::Result<(PathBuf, File)>,
    {
        for (id, config, candidates) in &self.scopes {
            for candidate in candidates {
                let (path, file) = match opener(candidate) {
                    Ok(opened) => opened,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                if !config.permits(&path) {
                    continue;
                }
                if !file.metadata()?.is_file() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "namespace image is not a regular file",
                    ));
                }
                return Ok(Some(OpenedNamespaceFile {
                    namespace: *id,
                    canonical_path: path,
                    file,
                }));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn linked_file_keeps_target_owner_and_rejects_symlink_escape() {
        let path = std::env::temp_dir().join(format!(
            "darwin-search-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, b"image").unwrap();
        let mut registry: NamespaceRegistry<()> = NamespaceRegistry::default();
        let app = registry
            .create(
                NamespaceConfig {
                    isolated: true,
                    search_paths: vec!["/app".into()],
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let system = registry
            .create(
                NamespaceConfig {
                    isolated: true,
                    search_paths: vec!["/system".into()],
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        registry
            .link(app, system, BTreeSet::from(["libpublic.so".into()]))
            .unwrap();
        let plan = registry.search_plan(app, "libpublic.so", &[]).unwrap();
        let mut attempts = Vec::new();
        let mut opened = plan
            .open(|candidate| {
                attempts.push(candidate.to_owned());
                let canonical = if candidate.starts_with("/app") {
                    "/private/libpublic.so"
                } else {
                    "/system/libpublic.so"
                };
                Ok((canonical.into(), File::open(&path)?))
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            attempts,
            vec![
                PathBuf::from("/app/libpublic.so"),
                "/system/libpublic.so".into()
            ]
        );
        assert_eq!(opened.namespace, system);
        std::fs::remove_file(&path).unwrap();
        let mut bytes = String::new();
        opened.file.read_to_string(&mut bytes).unwrap();
        assert_eq!(bytes, "image");
        // No permission to search a linked namespace for an unlisted SONAME.
        let private = registry.search_plan(app, "libprivate.so", &[]).unwrap();
        let mut attempts = 0;
        assert!(
            private
                .open(|candidate| {
                    attempts += 1;
                    assert!(candidate.starts_with("/app"));
                    Err(io::Error::from(io::ErrorKind::NotFound))
                })
                .unwrap()
                .is_none()
        );
        assert_eq!(attempts, 1);
    }
}
