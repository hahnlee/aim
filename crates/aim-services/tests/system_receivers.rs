//! Identify the original framework receivers reached by the app host's boot
//! broadcast without booting a guest or changing the pinned image.
use aim_android_image::{dex::Dex, system_server::dex_files};
use aim_services::package::parse::{self, Platform};

#[test]
fn pinned_framework_boot_receivers_belong_to_the_system_process() {
    let original = aim_paths::original_image();
    assert!(original.is_dir(), "NOT RUN: pinned original image missing");
    let platform = Platform::load(&original, Default::default()).unwrap();
    let path = "/system/framework/framework-res.apk";
    let package = parse::parse(
        &original.join(&path[1..]),
        path,
        parse::PARSE_IS_SYSTEM_DIR,
        &platform,
    )
    .unwrap();
    assert_eq!(package.package_name, "android");
    assert_eq!(
        package.shared_user_id.as_deref(),
        Some("android.uid.system")
    );
    let receivers: Vec<_> = package
        .receivers
        .iter()
        .filter(|receiver| {
            receiver.main.component.intents.iter().any(|intent| {
                intent
                    .filter
                    .actions
                    .iter()
                    .any(|action| action == "android.intent.action.BOOT_COMPLETED")
            })
        })
        .collect();
    let names: Vec<_> = receivers
        .iter()
        .map(|receiver| receiver.main.component.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "com.android.server.BootReceiver",
            "com.android.server.updates.CertPinInstallReceiver"
        ]
    );
    let services = std::fs::read(original.join("system/framework/services.jar")).unwrap();
    let dexes = dex_files(&services).unwrap();
    let dexes: Vec<_> = dexes.iter().map(|dex| Dex::parse(dex).unwrap()).collect();
    for name in names {
        let descriptor = format!("L{};", name.replace('.', "/"));
        assert!(dexes.iter().any(|dex| dex.class(&descriptor).is_some()));
    }
    for receiver in receivers {
        assert_eq!(receiver.main.process_name.as_deref(), Some("system"));
        assert!(receiver.main.exported);
    }
    let pins = package
        .receivers
        .iter()
        .find(|receiver| {
            receiver.main.component.name == "com.android.server.updates.CertPinInstallReceiver"
        })
        .unwrap();
    assert_eq!(
        pins.permission.as_deref(),
        Some("android.permission.UPDATE_CONFIG")
    );
}
