//! PackageInstallerSession.maybeStageDexMetadataLocked/inheritFileLocked,
//! android-16.0.0_r1 (AOSP, Apache-2.0). Rename/link the real inode, never its bytes.
use super::{Error, before};
use std::{collections::BTreeSet, fs, path::Path};

fn name(apk: &str) -> String {
    format!("{}.dm", apk.strip_suffix(".apk").unwrap_or(apk))
}
fn present(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(before("Dex metadata must be a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(before(error)),
    }
}
pub(super) fn normalize(stage: &Path, session: i32, apks: &[(String, String)]) -> Result<(), Error> {
    let mut moves = Vec::new();
    let mut targets = BTreeSet::new();
    for (source, target) in apks {
        let (source, target) = (name(source), name(target));
        if present(&stage.join(&source))? {
            if !targets.insert(target.clone()) {
                return Err(before("Duplicate staged dex metadata association"));
            }
            moves.push((source, target));
        }
    }
    for (source, target) in &moves {
        if source != target && present(&stage.join(target))?
            && !moves.iter().any(|(source, _)| source == target)
        {
            return Err(before("Dex metadata target is occupied by an unmatched file"));
        }
    }
    let moves = moves.into_iter().enumerate().filter(|(_, (source, target))| source != target)
        .map(|(index, (source, target))| (stage.join(source), stage.join(target), stage.join(format!(".native-dm-{session}-{index}"))))
        .collect::<Vec<_>>();
    for (_, _, temporary) in &moves {
        require_absent(temporary)?;
    }
    move_files(&moves, &mut |source, target| fs::rename(source, target))
}
fn require_absent(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(before(format!("Dex metadata normalization path already exists: {}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(before(error)),
    }
}
fn move_files(
    moves: &[(std::path::PathBuf, std::path::PathBuf, std::path::PathBuf)],
    rename: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), Error> {
    // 0: original source, 1: parked temporary, 2: normalized target.
    let mut locations = vec![0u8; moves.len()];
    let result = (|| {
        for (index, (source, _, temporary)) in moves.iter().enumerate() {
            rename(source, temporary)?;
            locations[index] = 1;
        }
        for (index, (_, target, temporary)) in moves.iter().enumerate() {
            rename(temporary, target)?;
            locations[index] = 2;
        }
        Ok::<(), std::io::Error>(())
    })();
    let Err(error) = result else { return Ok(()); };
    let mut restoration_errors = Vec::new();
    // Repark every published target before restoring any original, including
    // swaps. A failed repark must never let another restore overwrite that inode.
    for (index, (_, target, temporary)) in moves.iter().enumerate().rev() {
        if locations[index] == 2 {
            let result = require_absent(temporary).and_then(|()| rename(target, temporary).map_err(before));
            match result {
                Ok(()) => locations[index] = 1,
                Err(error) => restoration_errors.push(format!("{} -> {}: {error}", target.display(), temporary.display())),
            }
        }
    }
    for (index, (source, _, temporary)) in moves.iter().enumerate().rev() {
        if locations[index] == 1 {
            let result = require_absent(source).and_then(|()| rename(temporary, source).map_err(before));
            match result {
                Ok(()) => locations[index] = 0,
                Err(error) => restoration_errors.push(format!("{} -> {}: {error}", temporary.display(), source.display())),
            }
        }
    }
    if restoration_errors.is_empty() {
        Err(before(error))
    } else {
        // No installed state has committed; expose both the admission failure
        // and all retained paths requiring recovery, as the staging owner does.
        Err(before(format!("Dex metadata rename: {error}; source restoration failed: {}", restoration_errors.join("; "))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    struct Data(std::path::PathBuf);
    impl Data {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!("aim-dex-metadata-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            fs::create_dir(&path).unwrap(); Self(path)
        }
    }
    impl Drop for Data { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }
    #[test]
    fn pinned_base_split_and_vdex_metadata_follow_real_normalized_code_paths() {
        let jar = aim_paths::fetched().join("cts-tradefed/android-cts/testcases/CtsDexMetadataHostTestCases/CtsDexMetadataHostTestCases.jar");
        let source = android_image_extract::source::FileSource::open(&jar).expect("Pinned CTS inputs required; missing is not a pass");
        let archive = android_image_extract::zip::Archive::open(&source).unwrap();
        for vdex in [false, true] {
            let data = Data::new();
            let names = if vdex { ["CtsDexMetadataSplitAppWithVdex", "CtsDexMetadataSplitAppFeatureAWithVdex"] }
                else { ["CtsDexMetadataSplitApp", "CtsDexMetadataSplitAppFeatureA"] };
            let mappings: Vec<_> = names.iter().zip(["base.apk", "split_feature_a.apk"]).map(|(source, target)| (format!("{source}.apk"), target.into())).collect();
            let mut proofs = Vec::new();
            for (source, _) in &mappings {
                let old = name(source);
                let bytes = archive.read(archive.find(old.as_bytes()).unwrap(), 32 << 20).unwrap();
                fs::write(data.0.join(&old), &bytes).unwrap();
                aim_storage::guest_inode::record(&data.0.join(&old), aim_storage::guest_inode::GuestInode { uid:Some(1000), gid:Some(1000), mode:Some(0o644) }).unwrap();
                proofs.push((bytes, fs::metadata(data.0.join(&old)).unwrap().ino()));
            }
            fs::write(data.0.join("unmatched.dm"), b"unassociated source stays unassociated").unwrap();
            normalize(&data.0, 7, &mappings).unwrap();
            let inherited = Data::new();
            for ((old, target), (bytes, inode)) in mappings.iter().zip(proofs) {
                let path = data.0.join(name(target));
                assert!(!data.0.join(name(old)).exists());
                assert_eq!(fs::read(&path).unwrap(), bytes);
                assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
                assert_eq!(aim_storage::guest_inode::read(&path).unwrap().unwrap().mode, Some(0o644));
                inherit(&data.0.join(target), &inherited.0.join(target)).unwrap();
                assert_eq!(fs::metadata(inherited.0.join(name(target))).unwrap().ino(), inode);
                assert_eq!(fs::read(inherited.0.join(name(target))).unwrap(), bytes);
            }
            assert_eq!(fs::read(data.0.join("unmatched.dm")).unwrap(), b"unassociated source stays unassociated");
        }
    }
    #[test]
    fn sidecar_swaps_preserve_bytes_and_missing_companions_are_not_created() {
        let data = Data::new();
        fs::write(data.0.join("base.dm"), b"feature").unwrap();
        fs::write(data.0.join("split_feature.dm"), b"base").unwrap();
        normalize(&data.0, 9, &[("base.apk".into(), "split_feature.apk".into()), ("split_feature.apk".into(), "base.apk".into()), ("missing.apk".into(), "split_missing.apk".into())]).unwrap();
        assert_eq!(fs::read(data.0.join("base.dm")).unwrap(), b"base");
        assert_eq!(fs::read(data.0.join("split_feature.dm")).unwrap(), b"feature");
        assert!(!data.0.join("split_missing.dm").exists());
        assert_eq!(name("extensionless"), "extensionless.dm");
    }
    #[test]
    fn nonregular_companions_and_occupied_targets_fail_before_mutation() {
        let data = Data::new();
        fs::write(data.0.join("input.dm"), b"source").unwrap();
        fs::write(data.0.join("base.dm"), b"occupied").unwrap();
        let mapping = [("input.apk".into(), "base.apk".into())];
        assert!(normalize(&data.0, 1, &mapping).is_err());
        assert_eq!(fs::read(data.0.join("input.dm")).unwrap(), b"source");
        assert_eq!(fs::read(data.0.join("base.dm")).unwrap(), b"occupied");
        fs::remove_file(data.0.join("input.dm")).unwrap();
        std::os::unix::fs::symlink("base.dm", data.0.join("input.dm")).unwrap();
        assert!(normalize(&data.0, 1, &mapping).is_err());
    }
    #[test]
    fn second_temporary_collision_leaves_every_original_association_unchanged() {
        let data = Data::new();
        fs::write(data.0.join("incoming_base.dm"), b"base metadata").unwrap();
        fs::write(data.0.join("incoming_split.dm"), b"split metadata").unwrap();
        fs::write(data.0.join(".native-dm-7-1"), b"existing recovery evidence").unwrap();
        let base_inode = fs::metadata(data.0.join("incoming_base.dm")).unwrap().ino();
        let split_inode = fs::metadata(data.0.join("incoming_split.dm")).unwrap().ino();
        assert!(normalize(&data.0, 7, &[("incoming_base.apk".into(), "base.apk".into()), ("incoming_split.apk".into(), "split_feature.apk".into())]).is_err());
        assert_eq!(fs::read(data.0.join("incoming_base.dm")).unwrap(), b"base metadata");
        assert_eq!(fs::read(data.0.join("incoming_split.dm")).unwrap(), b"split metadata");
        assert_eq!(fs::metadata(data.0.join("incoming_base.dm")).unwrap().ino(), base_inode);
        assert_eq!(fs::metadata(data.0.join("incoming_split.dm")).unwrap().ino(), split_inode);
        assert_eq!(fs::read(data.0.join(".native-dm-7-1")).unwrap(), b"existing recovery evidence");
        assert!(!data.0.join(".native-dm-7-0").exists());
        assert!(!data.0.join("base.dm").exists());
        assert!(!data.0.join("split_feature.dm").exists());
    }
    #[test]
    fn staging_and_publication_errors_restore_real_swapped_sources() {
        for failure_at in [2, 4] {
            let data = Data::new();
            fs::write(data.0.join("base.dm"), b"feature").unwrap();
            fs::write(data.0.join("split_feature.dm"), b"base").unwrap();
            let moves = [(data.0.join("base.dm"), data.0.join("split_feature.dm"), data.0.join(".native-dm-9-0")),
                (data.0.join("split_feature.dm"), data.0.join("base.dm"), data.0.join(".native-dm-9-1"))];
            let mut calls = 0;
            let error = move_files(&moves, &mut |source, target| {
                calls += 1;
                if calls == failure_at { return Err(std::io::Error::from_raw_os_error(libc::EIO)); }
                fs::rename(source, target)
            }).unwrap_err();
            assert!(!error.committed);
            assert!(!error.message.contains("restoration failed"));
            assert_eq!(fs::read(data.0.join("base.dm")).unwrap(), b"feature");
            assert_eq!(fs::read(data.0.join("split_feature.dm")).unwrap(), b"base");
            assert!(!moves[0].2.exists()); assert!(!moves[1].2.exists());
        }
    }
    #[test]
    fn failed_restoration_reports_retained_paths_without_overwriting_an_admitted_inode() {
        let data = Data::new();
        fs::write(data.0.join("base.dm"), b"feature").unwrap();
        fs::write(data.0.join("split_feature.dm"), b"base").unwrap();
        let moves = [(data.0.join("base.dm"), data.0.join("split_feature.dm"), data.0.join(".native-dm-9-0")),
            (data.0.join("split_feature.dm"), data.0.join("base.dm"), data.0.join(".native-dm-9-1"))];
        let mut calls = 0;
        let error = move_files(&moves, &mut |source, target| {
            calls += 1;
            if calls == 4 { return Err(std::io::Error::from_raw_os_error(libc::EIO)); }
            if calls == 5 { return Err(std::io::Error::from_raw_os_error(libc::EACCES)); }
            fs::rename(source, target)
        }).unwrap_err();
        assert!(error.message.contains("source restoration failed"));
        assert!(error.message.contains("split_feature.dm"));
        assert!(error.message.contains(".native-dm-9-1"));
        assert_eq!(fs::read(data.0.join("split_feature.dm")).unwrap(), b"feature");
        assert_eq!(fs::read(&moves[1].2).unwrap(), b"base");
        assert!(!data.0.join("base.dm").exists());
    }
}
pub(super) fn inherit(source_apk: &Path, target_apk: &Path) -> Result<(), Error> {
    let source = source_apk.with_extension("dm");
    if present(&source)? {
        // Original same-filesystem inheritance links the admitted inode. This
        // retains fs-verity, SELinux, guest ownership and the existing proof.
        fs::hard_link(source, target_apk.with_extension("dm")).map_err(before)?;
    }
    Ok(())
}
