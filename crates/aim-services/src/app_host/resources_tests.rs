use super::*;

struct Data(std::path::PathBuf);
impl Drop for Data {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

/// Resource-only boot queries work even if unrelated package persistence
/// cannot be parsed; the native permission front consumes the same owner.
#[test]
fn framework_requests_do_not_load_packages() {
    let Some(root) = aim_paths::input(aim_paths::derived_image(), "derived-image") else {
        return;
    };
    let data = crate::app_host::test_temp_dir("resources");
    std::fs::create_dir_all(data.join("system")).unwrap();
    std::fs::write(data.join("system/packages.xml"), b"invalid package state").unwrap();
    let _cleanup = Data(data.clone());
    let process = crate::test_process(SYSTEM_UID);
    let host = AppHost::new(process.clone());
    crate::app_host::configure_test_host(
        &host,
        &root,
        &data,
        &data,
        Box::new(|name| {
            Some(
                match name {
                    "ro.build.version.sdk" => "36",
                    "ro.build.version.codename" => "REL",
                    "persist.sys.locale" => "en-US",
                    "ro.sf.lcd_density" => "320",
                    _ => return None,
                }
                .into(),
            )
        }),
    );
    let framework = host.framework().unwrap();
    let Binder::Local(ptr) = process.add_service(Arc::new(
        permission_manager::PermissionManager::new(host.clone()),
    )) else {
        unreachable!()
    };
    use aim_service_aidl::android_permission_ipermissionmanager as pm;
    let mut request = Parcel::new();
    pm::GetSplitPermissions {}.write(&mut request);
    let reply = process
        .local_service(ptr)
        .unwrap()
        .transact(pm::GET_SPLIT_PERMISSIONS, &request, false)
        .unwrap();
    reply.reader().read_exception().unwrap().unwrap();
    assert!(std::ptr::eq(framework, host.framework().unwrap()));
}
