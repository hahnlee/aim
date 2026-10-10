//! Replaced APK resource cleanup, ported from android-16.0.0_r1
//! RemovePackageHelper, AppDataHelper and PackageCacher (#816/#798).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use crate::package::{restrictions::UserState, scan::User, settings::Package};
use crate::system::System;
use aim_binder_host::local::LocalProcess;
use aim_service_aidl::android_os_iinstalld as installd;
use std::collections::{BTreeMap, BTreeSet};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Writable data and installer operations share one install lock. APK read
/// mappings are never used as deletion destinations. The caller owns this
/// data directory; tests supply disposable data, never original image paths.
pub struct CodeResources {
    system: Arc<System>,
    data: PathBuf,
    parser_cache: Option<PathBuf>,
    pending_cleanup: Mutex<BTreeSet<String>>,
}

impl CodeResources {
    pub fn new(process: Arc<LocalProcess>, data: PathBuf, parser_cache: Option<PathBuf>) -> Self {
        Self::with_system(System::new(process, &[]), data, parser_cache)
    }

    /// Native boot supplies its existing System so all install operations share one lock.
    pub(crate) fn with_system(
        system: Arc<System>,
        data: PathBuf,
        parser_cache: Option<PathBuf>,
    ) -> Self {
        Self {
            system,
            data,
            parser_cache,
            pending_cleanup: Mutex::new(BTreeSet::new()),
        }
    }

    pub fn reconcile_sdk_data(
        &self,
        args: super::sdk_data::SdkData,
    ) -> Result<(), aim_binder_host::parcel::Exception> {
        self.system.reconcile_package_sdk_data(args)
    }

    pub fn clean(&self, code_path: &str, incremental: bool) -> Result<(), String> {
        let _install = self.system.package_install_guard();
        let mut pending = self.pending_cleanup.lock().unwrap();
        clean(
            &self.data,
            self.parser_cache.as_deref(),
            code_path,
            incremental,
            &mut pending,
            |name, path| {
                let request = installd::RmPackageDir {
                    package_name: Some(name.into()),
                    package_dir: Some(path.into()),
                };
                self.system
                    .call(
                        "installd",
                        installd::RM_PACKAGE_DIR,
                        |p| request.write(p),
                        installd::read_rm_package_dir_reply,
                    )
                    .map_err(|e| format!("rmPackageDir {path}: {e:?}"))
            },
        )
    }

    /// PMS construction has not initialized ART Service: AppDataHelper omits
    /// separate ART-service profile clearing. Destroy CE/DE/external storage for
    /// every resolved user, preserving installer failures and earlier effects.
    /// Domain, dex/dynamic-code, permission and keystore state belong to their
    /// owners; this method does not remove a package setting (#798).
    pub fn destroy_boot_app_storage(
        &self,
        package: &Package,
        users: &[User],
        states: &BTreeMap<i32, UserState>,
    ) -> Result<(), String> {
        if package.name.is_empty()
            || matches!(package.name.as_str(), "." | "..")
            || package.name.contains(['/', '\\', '\0'])
        {
            return Err("invalid app storage package name".into());
        }
        if package.volume_uuid.is_some() {
            return Err(
                "private-volume app storage deletion requires its storage owner (#816)".into(),
            );
        }
        let mut ids = BTreeSet::new();
        if users.is_empty() || users.iter().any(|u| u.id < 0 || !ids.insert(u.id)) {
            return Err("app storage deletion requires distinct resolved users".into());
        }
        let _install = self.system.package_install_guard();
        for user in users {
            let request = installd::DestroyAppData {
                uuid: package.volume_uuid.clone(),
                package_name: Some(package.name.clone()),
                user_id: user.id,
                // FLAG_STORAGE_DE | FLAG_STORAGE_CE | FLAG_STORAGE_EXTERNAL.
                flags: 1 | 2 | 4,
                ce_data_inode: states.get(&user.id).map_or(0, |s| s.ce_data_inode),
            };
            self.system
                .call(
                    "installd",
                    installd::DESTROY_APP_DATA,
                    |p| request.write(p),
                    installd::read_destroy_app_data_reply,
                )
                .map_err(|e| format!("destroyAppData {} user {}: {e:?}", package.name, user.id))?;
        }
        Ok(())
    }
}

fn writable_path(data: &Path, guest: &str) -> Result<PathBuf, String> {
    let suffix = guest
        .strip_prefix("/data/app/")
        .ok_or("cleanup requires installed /data/app code (#816)")?;
    if suffix.is_empty()
        || suffix
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err("invalid installed code path".into());
    }
    let mut path = data.join("app");
    for part in std::iter::once("").chain(suffix.split('/')) {
        if !part.is_empty() {
            path.push(part);
        }
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("code cleanup cannot follow a symlink".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(path)
}

fn clean(
    data: &Path,
    parser_cache: Option<&Path>,
    code_path: &str,
    incremental: bool,
    pending: &mut BTreeSet<String>,
    mut remove_dir: impl FnMut(&str, &str) -> Result<(), String>,
) -> Result<(), String> {
    let host = writable_path(data, code_path)?;
    if incremental {
        return Err("incremental resource cleanup requires its storage owner (#816)".into());
    }
    let metadata = match fs::metadata(&host) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{code_path}: {e}")),
    };
    if metadata.is_none() && !pending.contains(code_path) {
        return Ok(());
    }
    if metadata.as_ref().is_some_and(|m| !m.is_dir()) {
        return fs::remove_file(host).map_err(|e| format!("{code_path}: {e}"));
    }
    let (parent, name) = code_path.rsplit_once('/').ok_or("missing code parent")?;
    let parent_name = parent
        .rsplit('/')
        .next()
        .ok_or("missing code parent name")?;
    // Validate the configured cache before the first irreversible operation.
    let cache = parser_cache
        .filter(|_| parent_name.starts_with("~~"))
        .map(|relative| -> Result<PathBuf, String> {
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err("parser cache must be relative to writable data".into());
            }
            let path = data.join(relative);
            let canonical = path
                .canonicalize()
                .map_err(|e| format!("parser cache: {e}"))?;
            if !canonical.starts_with(data.canonicalize().map_err(|e| e.to_string())?) {
                return Err("parser cache escapes writable data".into());
            }
            Ok(canonical)
        })
        .transpose()?;
    // A failed parent/cache operation must remain retryable after the child
    // directory has already disappeared. Durable recovery remains #798/#816.
    pending.insert(code_path.into());
    if metadata.is_some() {
        remove_dir(name, code_path)?;
    }
    if parent_name.starts_with("~~") {
        if writable_path(data, parent)?.exists() {
            remove_dir(name, parent)?;
        }
        if let Some(cache) = cache {
            for entry in fs::read_dir(cache).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_name().to_string_lossy().starts_with(parent_name) {
                    fs::remove_file(entry.path()).map_err(|e| format!("parser cache: {e}"))?;
                }
            }
        }
    }
    pending.remove(code_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Data(PathBuf);
    impl Data {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "aim-code-cleanup-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn code(&self) {
            fs::create_dir_all(self.0.join("app/~~random/package/lib")).unwrap();
            fs::write(
                self.0.join("app/~~random/package/base.apk"),
                b"disposable code",
            )
            .unwrap();
        }
    }
    impl Drop for Data {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn sdk_execution_and_code_cleanup_wait_for_the_same_install_owner() {
        use aim_binder_driver::{Credentials, Device, Driver};
        use std::{sync::mpsc, time::Duration};
        let driver = Driver::new();
        let process = LocalProcess::open(
            &driver,
            Device::Binder,
            Credentials {
                pid: 98001,
                euid: 1000,
                security_context: None,
            },
        );
        let system = System::new(process.clone(), &[]);
        let data = Data::new();
        fs::create_dir(data.0.join("app")).unwrap();
        let apk = data.0.join("app/owned.apk");
        fs::write(&apk, b"disposable code").unwrap();
        let resources = Arc::new(CodeResources::with_system(
            system.clone(),
            data.0.clone(),
            None,
        ));
        let guard = system.package_install_guard();
        let (started, ready) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let cleanup = {
            let resources = resources.clone();
            let started = started.clone();
            let finished = finished.clone();
            std::thread::spawn(move || {
                started.send(()).unwrap();
                resources.clean("/data/app/owned.apk", false).unwrap();
                finished.send(()).unwrap();
            })
        };
        let sdk = std::thread::spawn(move || {
            started.send(()).unwrap();
            // No installd is registered: execution must fail after acquiring the lock.
            assert!(
                resources
                    .reconcile_sdk_data(super::super::sdk_data::SdkData {
                        uuid: None,
                        package_name: Some("fixture.sdk.client".into()),
                        sub_dir_names: Some(vec![]),
                        user_id: 0,
                        app_id: 19001,
                        previous_app_id: 0,
                        se_info: Some("default".into()),
                        flags: 3,
                    })
                    .is_err()
            );
            finished.send(()).unwrap();
        });
        for _ in 0..2 {
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert_eq!(
            done.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        assert!(apk.exists());
        drop(guard);
        for _ in 0..2 {
            done.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        cleanup.join().unwrap();
        sdk.join().unwrap();
        assert!(!apk.exists());
        driver.release(process.proc_handle());
    }

    #[test]
    fn directory_cleanup_uses_installer_order_and_keeps_data_and_other_cache() {
        let data = Data::new();
        data.code();
        fs::create_dir_all(data.0.join("user/0/package")).unwrap();
        fs::create_dir(data.0.join("cache")).unwrap();
        for name in ["~~random-0", "~~random-other", "unrelated"] {
            fs::write(data.0.join("cache").join(name), b"cache").unwrap();
        }
        let mut calls = Vec::new();
        let mut pending = BTreeSet::new();
        clean(
            &data.0,
            Some(Path::new("cache")),
            "/data/app/~~random/package",
            false,
            &mut pending,
            |name, path| {
                calls.push((name.to_owned(), path.to_owned()));
                fs::remove_dir_all(writable_path(&data.0, path)?).map_err(|e| e.to_string())
            },
        )
        .unwrap();
        assert_eq!(
            calls,
            [
                ("package".into(), "/data/app/~~random/package".into()),
                ("package".into(), "/data/app/~~random".into())
            ]
        );
        assert!(!data.0.join("app/~~random").exists());
        assert!(data.0.join("user/0/package").exists());
        assert!(data.0.join("cache/unrelated").exists());
        assert_eq!(fs::read_dir(data.0.join("cache")).unwrap().count(), 1);
        assert!(pending.is_empty());
    }

    #[test]
    fn partial_installer_failure_remains_retryable_after_child_removal() {
        let data = Data::new();
        data.code();
        let mut pending = BTreeSet::new();
        let result = clean(
            &data.0,
            None,
            "/data/app/~~random/package",
            false,
            &mut pending,
            |_, path| {
                if path.ends_with("/package") {
                    fs::remove_dir_all(writable_path(&data.0, path)?).map_err(|e| e.to_string())
                } else {
                    Err("installer rejected parent".into())
                }
            },
        );
        assert_eq!(result.unwrap_err(), "installer rejected parent");
        assert!(!data.0.join("app/~~random/package").exists());
        assert!(pending.contains("/data/app/~~random/package"));
        let mut calls = Vec::new();
        clean(
            &data.0,
            None,
            "/data/app/~~random/package",
            false,
            &mut pending,
            |_, path| {
                calls.push(path.to_owned());
                fs::remove_dir_all(writable_path(&data.0, path)?).map_err(|e| e.to_string())
            },
        )
        .unwrap();
        assert_eq!(calls, ["/data/app/~~random"]);
        assert!(pending.is_empty());
    }

    #[test]
    fn invalid_paths_incremental_and_cache_escape_reject_before_deletion() {
        let data = Data::new();
        data.code();
        let mut pending = BTreeSet::new();
        for path in [
            "/system/app/package",
            "/data/app/../user/package",
            "/data/app/",
            "/data/app//package",
        ] {
            assert!(
                clean(&data.0, None, path, false, &mut pending, |_, _| panic!(
                    "unexpected installer call"
                ))
                .is_err()
            );
        }
        assert!(
            clean(
                &data.0,
                None,
                "/data/app/~~random/package",
                true,
                &mut pending,
                |_, _| panic!("unexpected installer call")
            )
            .is_err()
        );
        assert!(
            clean(
                &data.0,
                Some(Path::new("../cache")),
                "/data/app/~~random/package",
                false,
                &mut pending,
                |_, _| panic!("unexpected installer call")
            )
            .is_err()
        );
        std::os::unix::fs::symlink(data.0.join("app/~~random/package"), data.0.join("app/link"))
            .unwrap();
        assert!(
            clean(
                &data.0,
                None,
                "/data/app/link",
                false,
                &mut pending,
                |_, _| panic!("unexpected installer call")
            )
            .is_err()
        );
        assert!(data.0.join("app/~~random/package/base.apk").exists());
        assert!(pending.is_empty());
    }

    #[test]
    fn monolithic_and_missing_code_do_not_call_installer_or_remove_parent() {
        let data = Data::new();
        data.code();
        let mut pending = BTreeSet::new();
        clean(
            &data.0,
            None,
            "/data/app/~~random/package/base.apk",
            false,
            &mut pending,
            |_, _| panic!("unexpected installer call"),
        )
        .unwrap();
        assert!(data.0.join("app/~~random/package/lib").exists());
        clean(
            &data.0,
            None,
            "/data/app/missing",
            false,
            &mut pending,
            |_, _| panic!("unexpected installer call"),
        )
        .unwrap();
    }
}
