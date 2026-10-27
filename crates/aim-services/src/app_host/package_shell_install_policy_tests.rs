use super::*;
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_ARGUMENT, EX_SECURITY};
use aim_service_aidl::{WriteParcelable, dev_aim_server_ipackagebootstrapbridge as bootstrap};
use std::{
    fs,
    io::{Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
};

struct File(aim_binder_driver::File);
impl WriteParcelable for File {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0);
        p.write_file(self.0.clone());
    }
}

fn compile(
    dir: &Path,
    root: &Path,
    name: &str,
    application: &str,
    children: &str,
    bitcode: bool,
) -> PathBuf {
    let manifest = dir.join(format!("{name}.xml"));
    fs::write(&manifest, format!(r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="fixture.{name}">{children}<application {application}/></manifest>"#)).unwrap();
    let apk = dir.join(format!("{name}.apk"));
    let result =
        Command::new(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/aapt2"))
            .args(["link", "--manifest"])
            .arg(manifest)
            .arg("-I")
            .arg(root.join("system/framework/framework-res.apk"))
            .arg("-o")
            .arg(&apk)
            .output()
            .expect("pinned aapt2");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut paths = vec![
        "lib/arm64-v8a/libone.so",
        "lib/armeabi-v7a/libone.so",
        "lib/x86/libone.so",
        "lib/arm64-v8a/nested/libignored.so",
    ];
    if bitcode {
        paths.push("assets/code.bc");
    }
    for (path, size) in paths.iter().zip([17, 31, 47, 101, 7]) {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'a'; size]).unwrap();
    }
    let result = Command::new("/usr/bin/zip")
        .current_dir(dir)
        .arg("-q")
        .arg(&apk)
        .args(paths)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    apk
}

/// PackageManagerShellCommand's install argument/size contract through
/// the generated AIDL.
/// Java leaf: PackageShellInstallPolicy; pinned owners: ApkLiteParseUtils,
/// InstallLocationUtils, NativeLibraryHelper and ART DexoptParams/Utils.
#[test]
fn shell_install_policy_uses_transferred_apk_and_platform_owners() {
    let root =
        aim_paths::original_image_with("system/framework/framework-res.apk").expect("pinned image");
    let data = crate::app_host::test_temp_dir("shell-install");
    let process = |euid| {
        aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid,
                security_context: None,
            },
        )
    };
    let process = process(1000);
    let host = AppHost::new(process.clone());
    let default_filter = Arc::new(std::sync::Mutex::new(Some("speed-profile".to_owned())));
    let filter_property = default_filter.clone();
    host.configure(
        &root,
        &data,
        &data,
        &data,
        Box::new(move |name| {
            if name == "pm.dexopt.install" {
                return filter_property.lock().unwrap().clone();
            }
            Some(
                match name {
                    "ro.build.version.sdk" => "36",
                    "ro.build.version.codename" | "ro.build.version.all_codenames" => "REL",
                    "ro.product.cpu.abilist" => "arm64-v8a,armeabi-v7a,x86",
                    "ro.product.cpu.abilist64" => "arm64-v8a",
                    "ro.product.cpu.abilist32" => "armeabi-v7a,x86",
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
    let Binder::Local(ptr) = process.add_service(Arc::new(
        super::super::package_bootstrap::PackageBootstrap::new(host.clone()),
    )) else {
        unreachable!()
    };
    let mut request = Parcel::new();
    bootstrap::GetPackageShellInstallPolicy {}.write(&mut request);
    let answer = process
        .local_service(ptr)
        .unwrap()
        .transact(bootstrap::GET_PACKAGE_SHELL_INSTALL_POLICY, &request, false)
        .unwrap();
    let Some(Binder::Local(ptr)) =
        bootstrap::read_get_package_shell_install_policy_reply(&mut answer.reader())
            .unwrap()
            .unwrap()
    else {
        panic!("local leaf")
    };
    let node = process.local_service(ptr).unwrap();
    let transact = |code, request: &Parcel| node.transact(code, request, false).unwrap();
    for (abi, expected) in [
        (None, Some("Missing ABI argument")),
        (Some(""), Some("Missing ABI argument")),
        (Some("-"), None),
        (Some("arm64-v8a"), None),
        (Some("x86"), None),
        (
            Some("riscv64"),
            Some("ABI riscv64 not supported on this device"),
        ),
    ] {
        let mut p = Parcel::new();
        policy::ValidateAbi {
            abi: abi.map(Into::into),
            calling_uid: 2000,
            calling_pid: 0,
        }
        .write(&mut p);
        let answer = transact(policy::VALIDATE_ABI, &p);
        let result = policy::read_validate_abi_reply(&mut answer.reader()).unwrap();
        match expected {
            None => result.unwrap(),
            Some(message) => {
                let e = result.unwrap_err();
                assert_eq!((e.code, e.message.as_str()), (EX_ILLEGAL_ARGUMENT, message));
            }
        }
    }
    for filter in [
        "",
        "skip",
        "assume-verified",
        "verify",
        "space",
        "space-profile",
        "speed",
        "speed-profile",
        "everything",
        "everything-profile",
        "verify-none",
        "interpret-only",
        "verify-profile",
        "verify-at-runtime",
        "balanced",
        "time",
        "extract",
        "quicken",
        "run-from-apk",
        "speed ",
        "Speed",
    ] {
        let mut p = Parcel::new();
        policy::ValidateCompilerFilter {
            filter: Some(filter.into()),
            calling_uid: 0,
            calling_pid: 1,
        }
        .write(&mut p);
        let answer = transact(policy::VALIDATE_COMPILER_FILTER, &p);
        let result = policy::read_validate_compiler_filter_reply(&mut answer.reader()).unwrap();
        if ["run-from-apk", "speed ", "Speed"].contains(&filter) {
            let e = result.unwrap_err();
            assert_eq!(e.code, EX_ILLEGAL_ARGUMENT);
            assert_eq!(e.message, format!("Invalid compiler filter '{filter}'"));
        } else {
            result.unwrap();
        }
    }
    for (value, code, message) in [
        (
            None,
            EX_ILLEGAL_ARGUMENT,
            "No compiler filter for reason 'install'",
        ),
        (
            Some("invalid"),
            EX_ILLEGAL_STATE,
            "Got invalid compiler filter 'invalid' for reason 'install'",
        ),
    ] {
        *default_filter.lock().unwrap() = value.map(Into::into);
        let mut p = Parcel::new();
        policy::ValidateCompilerFilter {
            filter: Some(String::new()),
            calling_uid: 2000,
            calling_pid: 42,
        }
        .write(&mut p);
        let answer = transact(policy::VALIDATE_COMPILER_FILTER, &p);
        let e = policy::read_validate_compiler_filter_reply(&mut answer.reader())
            .unwrap()
            .unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (code, message));
    }
    let mut p = Parcel::new();
    policy::IsDependencyInstallerEnabled {
        calling_uid: 2000,
        calling_pid: 42,
    }
    .write(&mut p);
    let answer = transact(policy::IS_DEPENDENCY_INSTALLER_ENABLED, &p);
    assert_eq!(
        policy::read_is_dependency_installer_enabled_reply(&mut answer.reader())
            .unwrap()
            .unwrap(),
        host.framework()
            .unwrap()
            .0
            .flags
            .get("android.content.pm.sdk_dependency_installer")
            .copied()
            .unwrap_or(false)
    );

    let size = |apk: &Path, abi: Option<&str>| {
        let mut file = fs::File::open(apk).unwrap();
        file.seek(SeekFrom::Start(7)).unwrap();
        let mut p = Parcel::new();
        policy::CalculateInstalledSize {
            file: Some(File(
                aim_binder_host::server::file_from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap(),
            )),
            path: Some("/data/local/tmp/absent.apk".into()),
            abi: abi.map(Into::into),
            calling_uid: 2000,
            calling_pid: 42,
        }
        .write(&mut p);
        let answer = transact(policy::CALCULATE_INSTALLED_SIZE, &p);
        assert_eq!(file.stream_position().unwrap(), 7);
        policy::read_calculate_installed_size_reply(&mut answer.reader()).unwrap()
    };
    for (name, flags, children, bitcode, cases) in [
        (
            "single",
            "",
            "",
            false,
            vec![
                (None, 17),
                (Some("-"), 17),
                (Some("x86"), 47),
                (Some("absent"), 0),
            ],
        ),
        (
            "multi",
            "android:multiArch=\"true\"",
            "",
            false,
            vec![(None, 48), (Some("x86"), 48), (Some("-"), 48)],
        ),
        (
            "direct",
            "android:extractNativeLibs=\"false\"",
            "",
            false,
            vec![(None, 0), (Some("x86"), 0)],
        ),
        (
            "bitcode",
            "",
            "",
            true,
            vec![(None, 31), (Some("-"), 31), (Some("arm64-v8a"), 17)],
        ),
    ] {
        let apk = compile(&data, &root, name, flags, children, bitcode);
        let bytes = fs::read(&apk).unwrap();
        for (abi, native) in cases {
            assert_eq!(
                size(&apk, abi).unwrap(),
                bytes.len() as i64 + native,
                "{name} {abi:?}"
            );
        }
        assert_eq!(fs::read(&apk).unwrap(), bytes);
    }
    let malformed = data.join("malformed.apk");
    fs::write(&malformed, b"not an APK").unwrap();
    let e = size(&malformed, None).unwrap_err();
    assert_eq!(e.code, EX_ILLEGAL_ARGUMENT);
    assert_eq!(
        e.message,
        "Error: Failed to parse APK file: /data/local/tmp/absent.apk: Failed to parse /data/local/tmp/absent.apk"
    );
    let newer = compile(
        &data,
        &root,
        "newer",
        "",
        "<uses-sdk android:minSdkVersion=\"37\"/>",
        false,
    );
    assert_eq!(
        size(&newer, None).unwrap_err().message,
        "Error: Failed to parse APK file: /data/local/tmp/absent.apk: Requires newer sdk version #37 (current version is #36)"
    );

    // Every method enforces both the transport and forwarded identity,
    // before nullable inputs or any platform owner is consulted.
    for transport in [1000, 2000] {
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: transport,
                security_context: None,
            },
        );
        let Binder::Local(ptr) =
            process.add_service(Arc::new(ShellInstallPolicy(AppHost::new(process.clone()))))
        else {
            unreachable!()
        };
        let node = process.local_service(ptr).unwrap();
        for (uid, pid) in [(1000, 42), (10001, 42), (2000, -1), (2000, 42), (0, 0)] {
            if transport == 1000 && matches!(uid, 0 | 2000) && pid >= 0 {
                continue;
            }
            for code in [
                policy::CALCULATE_INSTALLED_SIZE,
                policy::VALIDATE_ABI,
                policy::VALIDATE_COMPILER_FILTER,
                policy::IS_DEPENDENCY_INSTALLER_ENABLED,
            ] {
                let mut p = Parcel::new();
                p.write_interface_token(policy::DESCRIPTOR);
                match code {
                    policy::CALCULATE_INSTALLED_SIZE => {
                        p.write_i32(0);
                        p.write_string16(None);
                        p.write_string16(None);
                    }
                    policy::VALIDATE_ABI | policy::VALIDATE_COMPILER_FILTER => {
                        p.write_string16(None)
                    }
                    _ => {}
                }
                p.write_i32(uid);
                p.write_i32(pid);
                let answer = node.transact(code, &p, false).unwrap();
                let e = answer.reader().read_exception().unwrap().unwrap_err();
                assert_eq!(
                    (e.code, e.message.as_str()),
                    (EX_SECURITY, "Invalid native shell policy caller")
                );
            }
        }
    }
    fs::remove_dir_all(data).unwrap();
}
