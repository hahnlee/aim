//! Process starts through zygote, as system_server's `ZygoteProcess`
//! sends them (`startViaZygote`, `zygoteSendArgsAndGetResult` at
//! `android-16.0.0_r1`), over the app host agent (`aim-app-host-agent`,
//! docs/app-host.md): zygote takes commands only from the system uid.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

/// `Zygote.MOUNT_EXTERNAL_*` (`StorageManager.MOUNT_MODE_EXTERNAL_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountExternal {
    None,
    Default,
    Installer,
    PassThrough,
    AndroidWritable,
}

/// What `ProcessList.startProcess` passes `Process.start` for a regular
/// zygote start.
#[derive(Clone, Debug, PartialEq)]
pub struct Start {
    pub uid: u32,
    pub gids: Vec<u32>,
    pub runtime_flags: i32,
    pub mount_external: MountExternal,
    pub target_sdk_version: i32,
    pub nice_name: String,
    pub seinfo: String,
    pub instruction_set: Option<String>,
    pub app_data_dir: Option<String>,
    /// `--start-child-zygote`.
    pub start_child_zygote: bool,
    pub package_name: Option<String>,
    pub is_top_app: bool,
    /// App data isolation (`ProcessList.startProcess`): the app's packages'
    /// and the allowlisted ones' volume and CE data inode, in their
    /// `ArrayMap`'s order; whether zygote binds the app's data and storage
    /// directories.
    pub pkg_data_info: Vec<DataInfo>,
    pub allowlisted_data_info: Vec<DataInfo>,
    pub bind_mount_app_storage_dirs: bool,
    pub bind_mount_app_data_dirs: bool,
    pub disabled_compat_changes: Vec<i64>,
    /// The class whose `main` runs (`android.app.ActivityThread` for an
    /// app), and the arguments after it: an app's start sequence
    /// (`ProcessList.PROC_START_SEQ_IDENT`), a child zygote's socket.
    pub process_class: String,
    pub extra_args: Vec<String>,
}

/// `ActivityThread`, and its argument `PROC_START_SEQ_IDENT` for a start.
pub const ACTIVITY_THREAD: &str = "android.app.ActivityThread";
pub fn start_seq_arg(seq: i64) -> String {
    format!("seq={seq}")
}

/// A package's data: its name, its volume's UUID, its CE data inode.
#[derive(Clone, Debug, PartialEq)]
pub struct DataInfo {
    pub package: String,
    pub volume_uuid: Option<String>,
    pub ce_data_inode: i64,
}

/// `name=package,volume,inode,...`, as `startViaZygote` joins a map (a
/// null volume as `null`).
fn data_info_arg(name: &str, infos: &[DataInfo]) -> String {
    let entries: Vec<String> = infos
        .iter()
        .map(|i| format!("{},{},{}", i.package, i.volume_uuid.as_deref().unwrap_or("null"), i.ce_data_inode))
        .collect();
    format!("{name}={}", entries.join(","))
}

impl Start {
    /// `startViaZygote`'s arguments, in its order.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec![
            "--runtime-args".to_string(),
            format!("--setuid={}", self.uid),
            format!("--setgid={}", self.uid),
            format!("--runtime-flags={}", self.runtime_flags),
        ];
        match self.mount_external {
            MountExternal::None => {}
            MountExternal::Default => args.push("--mount-external-default".into()),
            MountExternal::Installer => args.push("--mount-external-installer".into()),
            MountExternal::PassThrough => args.push("--mount-external-pass-through".into()),
            MountExternal::AndroidWritable => args.push("--mount-external-android-writable".into()),
        }
        args.push(format!("--target-sdk-version={}", self.target_sdk_version));
        if !self.gids.is_empty() {
            let gids: Vec<String> = self.gids.iter().map(u32::to_string).collect();
            args.push(format!("--setgroups={}", gids.join(",")));
        }
        args.push(format!("--nice-name={}", self.nice_name));
        args.push(format!("--seinfo={}", self.seinfo));
        if let Some(isa) = &self.instruction_set {
            args.push(format!("--instruction-set={isa}"));
        }
        if let Some(dir) = &self.app_data_dir {
            args.push(format!("--app-data-dir={dir}"));
        }
        if self.start_child_zygote {
            args.push("--start-child-zygote".into());
        }
        if let Some(package) = &self.package_name {
            args.push(format!("--package-name={package}"));
        }
        if self.is_top_app {
            args.push("--is-top-app".into());
        }
        if !self.pkg_data_info.is_empty() {
            args.push(data_info_arg("--pkg-data-info-map", &self.pkg_data_info));
        }
        if !self.allowlisted_data_info.is_empty() {
            args.push(data_info_arg("--allowlisted-data-info-map", &self.allowlisted_data_info));
        }
        if self.bind_mount_app_storage_dirs {
            args.push("--bind-mount-storage-dirs".into());
        }
        if self.bind_mount_app_data_dirs {
            args.push("--bind-mount-data-dirs".into());
        }
        if !self.disabled_compat_changes.is_empty() {
            let ids: Vec<String> = self
                .disabled_compat_changes
                .iter()
                .map(i64::to_string)
                .collect();
            args.push(format!("--disabled-compat-changes={}", ids.join(",")));
        }
        args.push(self.process_class.clone());
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

/// `attemptZygoteSendArgsAndGetResult`'s framing: the count, then each
/// argument on its own line. Zygote reads arguments by line, so none may
/// hold a newline.
pub fn frame(args: &[String]) -> io::Result<Vec<u8>> {
    if args.iter().any(|a| a.contains('\n')) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "embedded newlines not allowed",
        ));
    }
    let mut out = format!("{}\n", args.len());
    for a in args {
        out.push_str(a);
        out.push('\n');
    }
    Ok(out.into_bytes())
}

/// A session with zygote through the relay.
pub struct Session {
    stream: UnixStream,
}

impl Session {
    pub fn new(stream: UnixStream) -> Self {
        Session { stream }
    }

    /// A session with the child zygote on abstract socket `name`, through
    /// the relay (its connection's first line names it).
    pub fn child(mut stream: UnixStream, name: &str) -> io::Result<Self> {
        if name.contains('\n') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "embedded newline"));
        }
        stream.write_all(format!("@{name}\n").as_bytes())?;
        Ok(Session { stream })
    }

    /// `ZygoteProcess.preloadApp`: the application of `info` (its
    /// `ApplicationInfo` parcel) preloaded, as zygote answers (`1` when
    /// it did).
    pub fn preload_app(&mut self, info_parcel: &[u8]) -> io::Result<i32> {
        let encoded = aim_android_xml::base64::encode(info_parcel);
        self.stream.write_all(format!("2\n--preload-app\n{encoded}\n").as_bytes())?;
        let mut reply = [0u8; 4];
        self.stream.read_exact(&mut reply)?;
        Ok(i32::from_be_bytes(reply))
    }

    /// Starts `start`: the new process's pid, as zygote answers it (a
    /// big-endian `int`, then the `boolean` of a wrapper start, which a
    /// regular start never has). Zygote answers a failed fork with a
    /// negative pid (`ZygoteProcess`: "fork() failed").
    pub fn start(&mut self, start: &Start) -> io::Result<i32> {
        self.stream.write_all(&frame(&start.args())?)?;
        let mut reply = [0u8; 5];
        self.stream.read_exact(&mut reply)?;
        let pid = i32::from_be_bytes(reply[..4].try_into().unwrap());
        if pid < 0 {
            return Err(io::Error::other("zygote: fork() failed"));
        }
        Ok(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calculator() -> Start {
        Start {
            uid: 10213,
            gids: vec![50213, 20213, 9997],
            runtime_flags: 0x2000,
            mount_external: MountExternal::Default,
            target_sdk_version: 35,
            nice_name: "com.android.calculator2".into(),
            seinfo: "default:targetSdkVersion=35:complete".into(),
            instruction_set: Some("arm64".into()),
            app_data_dir: Some("/data/user/0/com.android.calculator2".into()),
            start_child_zygote: false,
            package_name: Some("com.android.calculator2".into()),
            is_top_app: true,
            pkg_data_info: vec![DataInfo { package: "com.android.calculator2".into(), volume_uuid: None, ce_data_inode: 4242 }],
            allowlisted_data_info: vec![DataInfo { package: "com.google.android.gms".into(), volume_uuid: None, ce_data_inode: 77 }],
            bind_mount_app_storage_dirs: false,
            bind_mount_app_data_dirs: true,
            disabled_compat_changes: vec![1, 2],
            process_class: ACTIVITY_THREAD.into(),
            extra_args: vec![start_seq_arg(7)],
        }
    }

    #[test]
    fn arguments_in_start_via_zygote_order() {
        assert_eq!(
            calculator().args(),
            [
                "--runtime-args",
                "--setuid=10213",
                "--setgid=10213",
                "--runtime-flags=8192",
                "--mount-external-default",
                "--target-sdk-version=35",
                "--setgroups=50213,20213,9997",
                "--nice-name=com.android.calculator2",
                "--seinfo=default:targetSdkVersion=35:complete",
                "--instruction-set=arm64",
                "--app-data-dir=/data/user/0/com.android.calculator2",
                "--package-name=com.android.calculator2",
                "--is-top-app",
                "--pkg-data-info-map=com.android.calculator2,null,4242",
                "--allowlisted-data-info-map=com.google.android.gms,null,77",
                "--bind-mount-data-dirs",
                "--disabled-compat-changes=1,2",
                "android.app.ActivityThread",
                "seq=7",
            ]
        );
    }

    #[test]
    fn framed_by_lines_and_answered_with_a_pid() {
        let (host, mut zygote) = UnixStream::pair().unwrap();
        let answer = std::thread::spawn(move || {
            let mut text = String::new();
            let mut byte = [0u8];
            while !text.ends_with("seq=7\n") {
                zygote.read_exact(&mut byte).unwrap();
                text.push(byte[0] as char);
            }
            zygote.write_all(&[0, 0, 0x30, 0x39, 0]).unwrap();
            text
        });
        assert_eq!(Session::new(host).start(&calculator()).unwrap(), 12345);
        assert!(answer.join().unwrap().starts_with("19\n--runtime-args\n"));
        assert!(frame(&["a\nb".into()]).is_err());
    }
}
