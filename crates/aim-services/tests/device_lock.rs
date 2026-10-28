//! The DeviceLock APEX owns both the feature and its original Binder service.
use aim_android_image::{dex::Dex, system_server::dex_files};
use aim_services::package::system_config::SystemConfig;
use std::{fs, path::PathBuf, process::Command, time::Duration};

#[test]
fn pinned_image_declares_device_lock_and_carries_its_original_service() {
    let image = aim_paths::original_image();
    assert!(image.is_dir(), "NOT RUN: pinned original image missing");
    let config = SystemConfig::read(&image, &|_| None);
    assert!(
        config
            .features
            .iter()
            .any(|(name, _)| name == "android.software.device_lock")
    );
    let jar =
        fs::read(image.join("apex/com.android.devicelock/javalib/service-devicelock.jar")).unwrap();
    let dex = dex_files(&jar).unwrap();
    let dex = Dex::parse(dex[0]).unwrap();
    let service = dex
        .class("Lcom/android/server/devicelock/DeviceLockService;")
        .unwrap();
    assert_eq!(
        service.superclass.as_deref(),
        Some("Lcom/android/server/SystemService;")
    );
    let implementation = dex
        .class("Lcom/android/server/devicelock/DeviceLockServiceImpl;")
        .unwrap();
    assert_eq!(
        implementation.superclass.as_deref(),
        Some("Landroid/devicelock/IDeviceLockService$Stub;")
    );
}

struct Boot(PathBuf);

impl Boot {
    fn command(&self) -> Command {
        let mut command = Command::new(aim_paths::host_bin("aimctl"));
        command.arg("--data").arg(&self.0);
        command
    }
}

impl Drop for Boot {
    fn drop(&mut self) {
        let _ = self.command().arg("stop").status();
    }
}

#[test]
#[ignore = "boots the app host on fresh disposable data; run explicitly"]
fn fresh_app_host_publishes_the_pinned_device_lock_aidl() {
    let directory = std::env::temp_dir().join(format!("aim-device-lock-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let boot = Boot(directory.join("data"));
    let output = boot
        .command()
        .arg("--image")
        .arg(aim_paths::out().join("app-host/root"))
        .arg("start")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    loop {
        // service list asks the named Binder for its interface descriptor;
        // this is the original android-16.0.0_r1 IDeviceLockService Stub.
        let output = boot
            .command()
            .args(["shell", "service", "list"])
            .output()
            .unwrap();
        if output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .contains("device_lock: [android.devicelock.IDeviceLockService]")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "device_lock unavailable: {output:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    drop(boot);
    fs::remove_dir_all(directory).unwrap();
}
