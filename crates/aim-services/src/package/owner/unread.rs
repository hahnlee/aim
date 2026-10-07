//! First writes of restriction files deliberately not restored by readLPw.
use super::{OwnedFile, Store, WriteError, sibling, write_with_observed};
use crate::package::Restrictions;
use aim_android_xml::{Element, abx};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
};

pub(super) struct Claim<const N: usize = 3> {
    pub(super) paths: [PathBuf; N],
    directory: File,
    files: [Option<OwnedFile>; N],
}

impl<const N: usize> Claim<N> {
    pub(super) fn inspect(paths: [PathBuf; N]) -> Result<Self, String> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(paths[0].parent().unwrap())
            .map_err(|e| e.to_string())?;
        let mut files = std::array::from_fn(|_| None);
        for (index, path) in paths.iter().enumerate() {
            let mut file = match OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("{}: {error}", path.display())),
            };
            if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                return Err("claimed input is not a regular file".into());
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            files[index] = Some(OwnedFile {
                file,
                payload: bytes.into(),
            });
        }
        Ok(Self {
            paths,
            files,
            directory,
        })
    }

    pub(super) fn check(&self) -> Result<(), String> {
        let owned = self.directory.metadata().map_err(|e| e.to_string())?;
        let current =
            fs::symlink_metadata(self.paths[0].parent().unwrap()).map_err(|e| e.to_string())?;
        if !current.is_dir() || owned.dev() != current.dev() || owned.ino() != current.ino() {
            return Err("claimed directory identity changed outside owner".into());
        }
        for (path, expected) in self.paths.iter().zip(&self.files) {
            match (fs::symlink_metadata(path), expected) {
                (Err(error), None) if error.kind() == io::ErrorKind::NotFound => {}
                (Ok(meta), Some(file)) if meta.is_file() && file.same_file(&meta) => {
                    if fs::read(path).map_err(|e| e.to_string())?.as_slice()
                        != file.payload.as_ref()
                    {
                        return Err("claimed files changed outside native owner".into());
                    }
                }
                _ => return Err("claimed file identity changed outside native owner".into()),
            }
        }
        Ok(())
    }

    pub(super) fn retain_outputs(&mut self, opened: Vec<OwnedFile>) -> Result<(), String> {
        let mut known: Vec<_> = self.files.iter_mut().filter_map(Option::take).collect();
        known.extend(opened);
        for (index, path) in self.paths.iter().enumerate() {
            match fs::symlink_metadata(path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Ok(meta) if meta.is_file() => {
                    let file = known
                        .iter()
                        .find(|file| file.same_file(&meta))
                        .ok_or("claimed output identity changed outside owner")?;
                    self.files[index] = Some(OwnedFile {
                        file: file.file.try_clone().map_err(|e| e.to_string())?,
                        payload: fs::read(path).map_err(|e| e.to_string())?.into(),
                    });
                }
                _ => return Err("claimed output path changed outside owner".into()),
            }
        }
        Ok(())
    }
}

impl Store {
    /// Claim bytes without interpreting a skipped package-restriction document.
    /// Call only once the actual first-boot user/scan owner is ready to write.
    pub fn claim_unread_restrictions(&mut self, user: u32) -> Result<(), WriteError> {
        if !self.unread_restrictions.contains(&user) || self.unread_claims.contains_key(&user) {
            return Err(WriteError::before(
                "restriction user is not awaiting an initial claim",
            ));
        }
        let dir = self.data.join("system/users").join(user.to_string());
        fs::create_dir_all(&dir).map_err(WriteError::before)?;
        let path = dir.join("package-restrictions.xml");
        let claim = Claim::inspect([
            path.clone(),
            dir.join("package-restrictions-backup.xml"),
            sibling(&path, ".reservecopy"),
        ])
        .map_err(WriteError::before)?;
        self.unread_claims.insert(user, claim);
        Ok(())
    }

    /// Commit a complete user document supplied by the initialized native owner.
    pub fn commit_initial_restrictions(
        &mut self,
        user: u32,
        root: Element,
    ) -> Result<(), WriteError> {
        self.commit_initial_restrictions_using(user, root, |file, bytes| file.write_all(bytes))
    }

    pub(super) fn commit_initial_restrictions_using(
        &mut self,
        user: u32,
        root: Element,
        write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    ) -> Result<(), WriteError> {
        if root.name != "package-restrictions" {
            return Err(WriteError::before("invalid initial restriction root"));
        }
        let parsed = Restrictions::parse(&root).map_err(WriteError::before)?;
        if !self.state.users.iter().any(|(id, _)| *id == user) {
            return Err(WriteError::before("unknown initial restriction user"));
        }
        let names: std::collections::BTreeSet<_> =
            parsed.packages.iter().map(|(name, _)| name).collect();
        if names.len() != parsed.packages.len() {
            return Err(WriteError::before("duplicate initial restriction package"));
        }
        if parsed.packages.len() != self.state.settings.packages.len()
            || parsed
                .packages
                .iter()
                .any(|(name, _)| !self.state.settings.packages.iter().any(|p| p.name == *name))
        {
            return Err(WriteError::before(
                "initial restriction inventory differs from settings",
            ));
        }
        let bytes = abx::write(&root).map_err(WriteError::before)?;
        let claim = self
            .unread_claims
            .get_mut(&user)
            .ok_or_else(|| WriteError::before("initial restriction files are not claimed"))?;
        claim.check().map_err(WriteError::before)?;
        let mut opened = Vec::new();
        let result = write_with_observed(
            &claim.paths[0],
            &claim.paths[1],
            |file| write(file, &bytes),
            |file, _| {
                opened.push(OwnedFile {
                    file: file.try_clone()?,
                    payload: std::sync::Arc::from([]),
                });
                Ok(())
            },
        );
        if result.is_ok() || result.as_ref().is_err_and(|e| e.committed) {
            self.state
                .users
                .iter_mut()
                .find(|(id, _)| *id == user)
                .ok_or_else(|| WriteError::before("unknown initial restriction user"))?
                .1
                .restrictions = parsed;
            if crate::package::preferred::has_preferred_resolver(&root) {
                self.preferred_users.insert(user);
            }
            self.restrictions.insert(user, root);
            self.unread_restrictions.remove(&user);
            self.unread_claims.remove(&user);
        } else {
            claim.retain_outputs(opened).map_err(WriteError::before)?;
        }
        result
    }
}
