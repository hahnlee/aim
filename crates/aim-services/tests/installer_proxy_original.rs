//! Original ART consumer of the native true-mode typed Binder proxy capability.
use aim_binder_driver::{Credentials, Device, GuestProcess};
use aim_binder_host::{local::LocalProcess, parcel::Binder};
use aim_services::package::{
    installer::{
        self,
        native::NativeOwners,
        policy::{DevicePolicy, UserPolicy},
    },
    model::{PackageState, PackageUserState, State, User},
    pkg::AndroidPackage,
    system_config::SystemConfig,
};
use std::{
    fs,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
mod common {
    pub mod java;
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};

// SET_CONTEXT_MGR_EXT reads only its inline object; no guest pointers or files.
struct Inline;
impl GuestProcess for Inline {
    fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), i32> {
        Err(libc::EFAULT)
    }
    fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), i32> {
        Err(libc::EFAULT)
    }
    fn get_file(&mut self, _: u32) -> Result<aim_binder_driver::File, i32> {
        Err(libc::EBADF)
    }
    fn install_file(&mut self, _: aim_binder_driver::File) -> Result<u32, i32> {
        Err(libc::EBADF)
    }
    fn close_fd(&mut self, _: u32) {
        unreachable!()
    }
}
fn isolated_client(boot: &Boot, name: &str) -> Command {
    let original = boot.client(10100);
    let mut client = Command::new(original.get_program());
    client.env_clear();
    for (key, value) in original.get_envs() {
        if let Some(value) = value {
            client.env(key, value);
        }
    }
    let mut args = original.get_args();
    while let Some(arg) = args.next() {
        if arg == "--binder" {
            args.next();
            client.arg(arg).arg(name);
        } else {
            client.arg(arg);
        }
    }
    client
}
struct ClientGuard(Option<Child>);
impl Drop for ClientGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            if child.try_wait().unwrap().is_none() {
                child.kill().unwrap();
            }
            child.wait().unwrap();
        }
    }
}
fn original_client(command: &mut Command) -> std::process::Output {
    let mut guard = ClientGuard(Some(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let deadline = Instant::now() + Duration::from_secs(60);
    while guard.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "original proxy consumer exceeded 60 seconds"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    guard.0.take().unwrap().wait_with_output().unwrap()
}
#[test]
#[ignore = "requires rebuilt wire-v2 runtime, pinned image, aimctl, JDK and d8; explicit true-mode test owner"]
fn original_art_consumes_native_installer_proxy_capability() {
    let directory = std::env::temp_dir().join(format!("aim-proxy-original-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let data = Data(directory);
    let repo = aim_paths::root();
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(
            &repo.join("java/device-services/stubs"),
        )));
    let api = data.0.join("api/android");
    fs::create_dir_all(api.join("os")).unwrap();
    fs::create_dir_all(api.join("system")).unwrap();
    fs::create_dir_all(api.join("content/pm")).unwrap();
    fs::write(api.join("content/pm/PackageInstaller.java"), "package android.content.pm; public final class PackageInstaller { public static class SessionParams implements android.os.Parcelable { public long sizeBytes; public String appPackageName; public SessionParams(int mode) { throw new RuntimeException(); } public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException(); } public int describeContents() { throw new RuntimeException(); } } }").unwrap();
    let internal = data.0.join("api/com/android/internal/os");
    fs::create_dir_all(&internal).unwrap();
    fs::write(internal.join("BinderInternal.java"), "package com.android.internal.os; public final class BinderInternal { public static android.os.IBinder getContextObject() { throw new RuntimeException(); } }").unwrap();
    let pfd = fs::read_to_string(repo.join("java/device-services/stubs/android/os/ParcelFileDescriptor.java")).unwrap().replace("    public static class AutoCloseInputStream", "    public static class AutoCloseOutputStream extends java.io.FileOutputStream { public AutoCloseOutputStream(ParcelFileDescriptor fd) { super(fd.getFileDescriptor()); } }\n    public static class AutoCloseInputStream");
    let pfd = pfd.replace("    public FileDescriptor getFileDescriptor()", "    public int getFd() { throw new RuntimeException(); }\n    public int detachFd() { throw new RuntimeException(); }\n    public static ParcelFileDescriptor adoptFd(int fd) { throw new RuntimeException(); }\n    public FileDescriptor getFileDescriptor()");
    fs::write(api.join("os/ParcelFileDescriptor.java"), pfd).unwrap();
    fs::write(
        api.join("system/StructStat.java"),
        "package android.system; public final class StructStat { public long st_size; }",
    )
    .unwrap();
    fs::write(api.join("system/ErrnoException.java"), "package android.system; public final class ErrnoException extends Exception { public final int errno; public ErrnoException(String functionName, int errno) { this.errno = errno; } }").unwrap();
    fs::write(api.join("system/StructMsghdr.java"), "package android.system; public final class StructMsghdr { public StructCmsghdr[] msg_control; public int msg_flags; public StructMsghdr(java.net.SocketAddress name, java.nio.ByteBuffer[] iov, StructCmsghdr[] control, int flags) { throw new RuntimeException(); } }").unwrap();
    fs::write(api.join("system/StructCmsghdr.java"), "package android.system; public final class StructCmsghdr { public int cmsg_level, cmsg_type; public byte[] cmsg_data; public StructCmsghdr(int level, int type, byte[] value) { throw new RuntimeException(); } }").unwrap();
    fs::write(api.join("system/Os.java"), "package android.system; public final class Os { public static void socketpair(int domain,int type,int protocol,java.io.FileDescriptor a,java.io.FileDescriptor b) throws ErrnoException { throw new RuntimeException(); } public static int sendmsg(java.io.FileDescriptor fd,StructMsghdr msg,int flags) throws ErrnoException,java.net.SocketException { throw new RuntimeException(); } public static int recvmsg(java.io.FileDescriptor fd,StructMsghdr msg,int flags) throws ErrnoException,java.net.SocketException { throw new RuntimeException(); } public static int fcntlInt(java.io.FileDescriptor fd,int cmd,int arg) throws ErrnoException { throw new RuntimeException(); } public static void execv(String path,String[] argv) throws ErrnoException { throw new RuntimeException(); } public static java.io.FileDescriptor dup(java.io.FileDescriptor fd) throws ErrnoException { throw new RuntimeException(); } public static void close(java.io.FileDescriptor fd) throws ErrnoException { throw new RuntimeException(); } public static void fsync(java.io.FileDescriptor fd) throws ErrnoException { throw new RuntimeException(); } public static long lseek(java.io.FileDescriptor fd,long offset,int whence) throws ErrnoException { throw new RuntimeException(); } public static StructStat fstat(java.io.FileDescriptor fd) throws ErrnoException { throw new RuntimeException(); } public static int write(java.io.FileDescriptor fd,byte[] b,int off,int len) throws ErrnoException { throw new RuntimeException(); } public static int pread(java.io.FileDescriptor fd,byte[] b,int off,int len,long pos) throws ErrnoException { throw new RuntimeException(); } }").unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .arg("-classpath")
        .arg(&stubs)
        .args(common::java::sources(&data.0.join("api"))));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(repo.join("crates/aim-services/tests/fixtures/InstallerProxyOracle.java")));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(java.join("build-tools-36.0.0/android-16/lib/d8.jar"))
        .args([
            "com.android.tools.r8.D8",
            "--release",
            "--min-api",
            "36",
            "--lib",
        ])
        .arg(&jdk)
        .arg("--classpath")
        .arg(&stubs)
        .arg("--output")
        .arg(&dex)
        .args(
            fs::read_dir(&classes)
                .unwrap()
                .map(|entry| entry.unwrap().path()),
        ));
    common::java::check_linkage(&dex.join("classes.dex"), &[]).unwrap();
    let boot = Boot {
        ctl: repo.join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    run(boot.command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let output = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1" {
            break;
        }
        assert!(Instant::now() < deadline, "proxy oracle boot incomplete");
        std::thread::sleep(Duration::from_secs(1));
    }
    fs::copy(
        dex.join("classes.dex"),
        boot.data.join("data/local/tmp/proxy-oracle.dex"),
    )
    .unwrap();
    let name = format!("dev.aim.proxy-oracle.{}", std::process::id());
    let server = aim_binder_host::server::Server::start(&name).unwrap();
    let process = LocalProcess::open(
        server.driver(),
        Device::Binder,
        Credentials {
            pid: std::process::id() as i32,
            euid: 1000,
            security_context: None,
        },
    );
    let mut state = State::default();
    state.users.insert(
        0,
        User {
            id: 0,
            ..Default::default()
        },
    );
    state.packages.insert(
        "fixture".into(),
        PackageState {
            name: "fixture".into(),
            app_id: 10100,
            pkg: Some(Arc::new(AndroidPackage {
                package_name: "fixture".into(),
                uid: 10100,
                target_sdk_version: 35,
                ..Default::default()
            })),
            users: [(0, PackageUserState::default())].into(),
            ..Default::default()
        },
    );
    let state = Arc::new(state);
    let sessions = Arc::new(installer::Sessions::default());
    let native_data = data.0.join("native");
    for directory in ["system", "app", "app-staging"] {
        fs::create_dir_all(native_data.join(directory)).unwrap();
    }
    let inode = aim_storage::guest_inode::GuestInode {
        uid: Some(1000),
        gid: Some(1000),
        mode: Some(0o600),
    };
    let disk = installer::storage::Store::open(
        native_data.clone(),
        inode,
        aim_storage::guest_inode::GuestInode {
            mode: Some(0o775),
            ..inode
        },
        Arc::new(|path, guest| {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            let label = if guest.starts_with("/data/app/") {
                c"u:object_r:apk_tmp_file:s0"
            } else {
                c"u:object_r:system_data_file:s0"
            };
            if unsafe {
                libc::setxattr(
                    path.as_ptr(),
                    c"dev.aim.xattr.security.selinux".as_ptr(),
                    label.as_ptr().cast(),
                    label.to_bytes_with_nul().len(),
                    0,
                    libc::XATTR_NOFOLLOW,
                )
            } != 0
            {
                return Err(installer::storage::Error {
                    committed: false,
                    message: std::io::Error::last_os_error().to_string(),
                });
            }
            Ok(())
        }),
    )
    .unwrap();
    let publisher = process.clone();
    let owners = NativeOwners::open(
        sessions.clone(),
        Arc::new(move || Ok(state.clone())),
        Arc::new(|_, _| {
            Ok(DevicePolicy {
                debuggable: false,
                apex_supported: false,
                rollback_lifetime: true,
                users: [(
                    0,
                    UserPolicy {
                        disallow_install_apps: false,
                        disallow_debugging_features: false,
                        organization_managed: false,
                    },
                )]
                .into(),
                adopted_shell_uids: Default::default(),
                verifier_uid: None,
            })
        }),
        SystemConfig::default(),
        disk,
        Arc::new(move |node| Ok(publisher.add_service(node))),
        Arc::new(|_| {}),
        process.clone(),
    )
    .unwrap();
    let callback = owners.take_callback_worker().unwrap();
    owners
        .configure_writer(
            Arc::new(|| Ok(true)),
            Arc::new(|_, _, _| panic!("negative length must not allocate")),
        )
        .unwrap();
    let io = owners.take_io_worker_guard().unwrap();
    let installer = process.add_service(Arc::new(installer::endpoint::Endpoint {
        sessions: sessions.clone(),
        owners: owners.clone(),
    }));
    let Binder::Local(ptr) = installer else {
        unreachable!()
    };
    use aim_binder_driver::uapi::*;
    let mut object = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: FLAT_BINDER_FLAG_ACCEPTS_FDS,
        binder: ptr,
        cookie: ptr,
    }
    .encode();
    server
        .driver()
        .ioctl(
            process.proc_handle(),
            1,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut Inline,
        )
        .unwrap();
    process.start();
    use aim_service_aidl::{
        android_content_pm_ipackageinstaller as api,
        android_content_pm_ipackageinstallersession as session,
    };
    let output = original_client(
        isolated_client(&boot, &name)
            .args([
                "/system/bin/app_process",
                "-Djava.class.path=/data/local/tmp/proxy-oracle.dex",
                "/system/bin",
                "InstallerProxyOracle",
            ])
            .args(
                [
                    api::CREATE_SESSION,
                    api::OPEN_SESSION,
                    session::OPEN_WRITE,
                    session::ABANDON,
                    session::OPEN_READ,
                ]
                .map(|code| code.to_string()),
            ),
    );
    assert!(
        output.status.success(),
        "{}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(
        "PROXY original AutoCloseOutputStream Binder typed fd dup seek fsync fstat revoke"
    ));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("PROXY original SCM_RIGHTS retransmit exec classification revoke")
    );
    assert!(
        sessions
            .records()
            .iter()
            .all(|(session, _)| session.destroyed)
    );
    owners.shutdown_callbacks();
    owners.shutdown_io();
    drop(io);
    drop(callback);
    assert!(owners.take_errors().is_empty());
    drop(boot);
}
