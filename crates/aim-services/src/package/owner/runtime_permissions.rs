//! RuntimePermissionsPersistenceImpl / AtomicFile, Android 16 r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{OwnedFile, Store, WriteError, remove, unread::Claim};
use crate::package::permissions::RuntimePermissions;
use aim_storage::guest_inode::{self, GuestInode};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};

impl Store {
    /// Pin main, legacy backup, temporary and reserve inputs without substituting
    /// saved grants for the caller's current permission producer.
    pub fn claim_runtime_permissions(&mut self,user:u32)->Result<(),WriteError>{
        if !self.state.users.iter().any(|(id,_)|*id==user)||self.runtime_claims.contains_key(&user){
            return Err(WriteError::before("runtime permission user is unknown or already claimed"));
        }
        let claim=self.inspect_runtime_permission_claim(user)?;
        self.runtime_claims.insert(user,claim);Ok(())
    }
    /// Constructor handoff, before metadata installation or any timer starts.
    /// The caller retains the exclusive stopped-Settings-writer lease. The
    /// original PermissionService keeps access.abx; these are the legacy
    /// Settings runtime-permissions.xml files and their AtomicFile companions.
    pub fn claim_runtime_permission_inventory(&mut self,users:&[u32])->Result<(),WriteError>{
        let expected:std::collections::BTreeSet<_>=self.state.users.iter().map(|(id,_)|*id).collect();
        let requested:std::collections::BTreeSet<_>=users.iter().copied().collect();
        if requested!=expected||requested.len()!=users.len()||!self.runtime_claims.is_empty(){
            return Err(WriteError::before("runtime permission handoff inventory differs or is already claimed"));
        }
        let mut claims=std::collections::BTreeMap::new();
        for user in users{claims.insert(*user,self.inspect_runtime_permission_claim(*user)?);}
        self.runtime_claims=claims;Ok(())
    }
    fn inspect_runtime_permission_claim(&self,user:u32)->Result<Claim<4>,WriteError>{
        let dir=self.data.join("misc_de").join(user.to_string()).join(crate::package::PERMISSION_DIR);
        fs::create_dir_all(&dir).map_err(WriteError::before)?;
        let main=dir.join("runtime-permissions.xml");
        Claim::inspect([main.clone(),crate::package::sibling(&main,".bak"),crate::package::sibling(&main,".new"),crate::package::sibling(&main,".reservecopy")])
            .map_err(WriteError::before)
    }

    /// The caller supplies current version/fingerprint/grants and the guest
    /// creation metadata. A reserve failure retains the committed main state.
    pub fn commit_runtime_permissions(
        &mut self,
        user: u32,
        state: &RuntimePermissions,
        inode: GuestInode,
    ) -> Result<(), WriteError> {
        self.commit_runtime_permissions_using(user, state, inode, |file, bytes| {
            file.write_all(bytes)
        })
    }

    fn commit_runtime_permissions_using(
        &mut self,
        user: u32,
        state: &RuntimePermissions,
        inode: GuestInode,
        write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    ) -> Result<(), WriteError> {
        let bytes = state.serialize().map_err(WriteError::before)?;
        let mode = inode
            .mode
            .ok_or_else(|| WriteError::before("missing runtime permission creation mode"))?;
        if inode.uid.is_none() || inode.gid.is_none() || mode & !0o7777 != 0 {
            return Err(WriteError::before(
                "invalid runtime permission creation owner/mode",
            ));
        }
        let claim = self
            .runtime_claims
            .get_mut(&user)
            .ok_or_else(|| WriteError::before("runtime permission files are not claimed"))?;
        claim.check().map_err(WriteError::before)?;
        let [main, backup, temp, reserve] = claim.paths.clone();
        let mut opened = Vec::new();
        let mut committed = false;
        let mut started = false;
        let result = (|| -> io::Result<()> {
            if backup.exists() {
                fs::rename(&backup, &main)?;
            }
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temp)?;
            started = true;
            opened.push(OwnedFile {
                file: file.try_clone()?,
                payload: std::sync::Arc::from([]),
            });
            file.set_permissions(fs::Permissions::from_mode(mode))?;
            guest_inode::record(&temp, inode)?;
            write(&mut file, &bytes)?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, &main)?;
            committed = true;
            remove(&reserve)?;
            let mut copy = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&reserve)?;
            opened.push(OwnedFile {
                file: copy.try_clone()?,
                payload: std::sync::Arc::from([]),
            });
            copy.set_permissions(fs::Permissions::from_mode(mode))?;
            guest_inode::record(&reserve, inode)?;
            copy.write_all(&bytes)?;
            copy.sync_all()?;
            Ok(())
        })();
        let result = if let Err(error) = result {
            let cleanup = if committed || !started {
                Ok(())
            } else {
                remove(&temp)
            };
            Err(WriteError {
                committed,
                message: match cleanup {
                    Ok(()) => error.to_string(),
                    Err(cleanup) => format!("{error}; rollback: {cleanup}"),
                },
            })
        } else {
            Ok(())
        };
        let refresh = claim.retain_outputs(opened);
        if committed {
            let mut persisted = state.clone();
            for (_, permissions) in persisted
                .packages
                .iter_mut()
                .chain(&mut persisted.shared_users)
            {
                for permission in permissions {
                    permission.granted &= permission.flags & (1 << 16) == 0;
                }
            }
            self.state
                .users
                .iter_mut()
                .find(|(id, _)| *id == user)
                .unwrap()
                .1
                .runtime_permissions = Some(persisted);
        }
        if let Err(message) = refresh {
            return Err(WriteError {
                committed,
                message: match &result {
                    Ok(()) => message,
                    Err(error) => format!("{}; ownership refresh: {message}", error.message),
                },
            });
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Data;
    use super::*;

    #[test]
    fn runtime_handoff_claims_all_users_before_write_without_reseeding_inputs(){
        let data=Data::new();data.settings();
        let mut store=Store::open(&data.0,&[0,10]).unwrap().unwrap();
        let path=data.0.join("misc_de/0/apexdata/com.android.permission/runtime-permissions.xml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let previous=RuntimePermissions{version:4,fingerprint:Some("previous-owner".into()),..Default::default()}.serialize().unwrap();
        fs::write(&path,&previous).unwrap();
        let desired=RuntimePermissions{version:7,fingerprint:Some("current-owner".into()),..Default::default()};
        let error=store.commit_runtime_permissions(0,&desired,inode()).unwrap_err();
        assert!(!error.committed);assert!(error.message.contains("not claimed"));
        assert!(store.claim_runtime_permission_inventory(&[0]).is_err());
        assert_eq!(fs::read(&path).unwrap(),previous);
        store.claim_runtime_permission_inventory(&[0,10]).unwrap();
        assert_eq!(fs::read(&path).unwrap(),previous,"handoff must observe original inputs without reseeding");
        assert!(store.claim_runtime_permission_inventory(&[0,10]).is_err(),"second handoff must not silently re-claim inputs");
        for user in [0,10]{store.commit_runtime_permissions(user,&desired,inode()).unwrap();}
        let outside=RuntimePermissions{version:99,..Default::default()}.serialize().unwrap();fs::write(&path,&outside).unwrap();
        let error=store.commit_runtime_permissions(0,&desired,inode()).unwrap_err();assert!(!error.committed);
        assert_eq!(fs::read(&path).unwrap(),outside,"external writes remain an ownership error");
        store.register_package_user(11).unwrap();
        store.commit_runtime_permissions(11,&desired,inode()).unwrap();
    }

    fn inode() -> GuestInode {
        GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        }
    }
    #[test]
    fn runtime_atomic_writer_recovers_backup_rolls_back_and_rejects_external_changes() {
        let data = Data::new();
        data.settings();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        let dir = data.0.join("misc_de/0/apexdata/com.android.permission");
        fs::create_dir_all(&dir).unwrap();
        let main = dir.join("runtime-permissions.xml");
        let backup = crate::package::sibling(&main, ".bak");
        let temp = crate::package::sibling(&main, ".new");
        let reserve = crate::package::sibling(&main, ".reservecopy");
        fs::write(&main, b"old main").unwrap();
        fs::write(&backup, b"old backup").unwrap();
        fs::write(&reserve, b"old reserve").unwrap();
        store.claim_runtime_permissions(0).unwrap();
        let desired = RuntimePermissions {
            version: 7,
            ..Default::default()
        };
        let error = store
            .commit_runtime_permissions_using(0, &desired, inode(), |file, _| {
                file.write_all(b"partial")?;
                Err(io::Error::other("injected"))
            })
            .unwrap_err();
        assert!(!error.committed);
        assert_eq!(fs::read(&main).unwrap(), b"old backup");
        assert!(!temp.exists());
        assert_eq!(fs::read(&reserve).unwrap(), b"old reserve");
        store
            .commit_runtime_permissions(0, &desired, inode())
            .unwrap();
        assert_eq!(fs::read(&main).unwrap(), fs::read(&reserve).unwrap());
        assert_eq!(
            store.state.users[0].1.runtime_permissions,
            Some(desired.clone())
        );
        fs::write(&main, b"external").unwrap();
        assert!(
            !store
                .commit_runtime_permissions(0, &desired, inode())
                .unwrap_err()
                .committed
        );
        assert_eq!(fs::read(&main).unwrap(), b"external");
    }
    #[test]
    fn runtime_main_commit_is_published_when_reserve_copy_fails() {
        let data = Data::new();
        data.settings();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        store.claim_runtime_permissions(0).unwrap();
        let reserve = data
            .0
            .join("misc_de/0/apexdata/com.android.permission/runtime-permissions.xml.reservecopy");
        let desired = RuntimePermissions {
            version: 9,
            ..Default::default()
        };
        let error = store
            .commit_runtime_permissions_using(0, &desired, inode(), |file, bytes| {
                file.write_all(bytes)?;
                fs::create_dir(&reserve)?;
                Ok(())
            })
            .unwrap_err();
        assert!(error.committed);
        assert_eq!(store.state.users[0].1.runtime_permissions, Some(desired));
    }
}
