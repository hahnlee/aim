use super::resources::Framework;
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
    let data = std::env::temp_dir().join(format!("aim-resources-{}", std::process::id()));
    std::fs::create_dir_all(data.join("system")).unwrap();
    std::fs::write(data.join("system/packages.xml"), b"invalid package state").unwrap();
    let _cleanup = Data(data.clone());
    let process = LocalProcess::open(
        &aim_binder_driver::Driver::new(),
        aim_binder_driver::Device::Binder,
        aim_binder_driver::Credentials {
            pid: std::process::id() as i32,
            euid: SYSTEM_UID,
            security_context: None,
        },
    );
    let host = AppHost::new(process.clone());
    host.configure(
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
        Box::new(|_, _| {}),
        Box::new(|| Err(std::io::Error::other("no zygote"))),
    )
    .unwrap();
    let framework = host.framework().unwrap();
    let Binder::Local(ptr) = process.add_service(Arc::new(
        permission_manager::PermissionManager::new(host.clone()),
    )) else {
        unreachable!()
    };
    let reply = process
        .local_service(ptr)
        .unwrap()
        .transact(
            aim_service_aidl::android_permission_ipermissionmanager::GET_SPLIT_PERMISSIONS,
            &Parcel::new(),
            false,
        )
        .unwrap();
    reply.reader().read_exception().unwrap().unwrap();
    assert!(host.inputs.get().is_none());
    assert!(host.model.lock().unwrap().is_none());
    assert!(std::ptr::eq(framework, host.framework().unwrap()));
}
