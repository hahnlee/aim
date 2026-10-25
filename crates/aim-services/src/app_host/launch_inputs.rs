//! Inputs consumed by a process start. The legacy acquisition still reads
//! Settings and packages.list; the native capture replaces that backing in #2023.

use std::path::Path;

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use crate::package;

pub(super) struct Inputs {
    pub uid: u32,
    pub permission_gids: Vec<u32>,
    pub seinfo: String,
    pub data_dir: String,
    pub flags: i32,
    pub target_sdk: i32,
    pub primary_cpu_abi: Option<String>,
}

impl Inputs {
    pub(super) fn legacy(data: &Path, name: &str) -> Result<Self, Exception> {
        let state = package::State::read(data, &[0])
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("package state: {e}")))?
            .ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "no package state: Android never booted on this data",
                )
            })?;
        let setting = state
            .settings
            .packages
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| Exception::illegal_argument(format!("unknown package {name}")))?;
        let listed = state.list.iter().find(|e| e.name == name).ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, format!("{name} not in packages.list"))
        })?;
        Ok(Self {
            uid: listed.uid,
            permission_gids: listed.gids.clone(),
            seinfo: listed.seinfo.clone(),
            data_dir: listed.data_dir.clone(),
            flags: setting.flags,
            target_sdk: setting.target_sdk_version,
            primary_cpu_abi: setting.primary_cpu_abi.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_host::{PER_USER_RANGE, SEINFO_COMPLETE, instruction_set, process_gids, zygote};

    struct Data(std::path::PathBuf);
    impl Data {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let data = std::env::temp_dir().join(format!(
                "aim-launch-inputs-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(data.join("system")).unwrap();
            Self(data)
        }
        fn settings(&self) {
            let xml = aim_android_xml::read(
                br#"<packages>
                <package name="example.app" codePath="/data/app/example" userId="10213"
                  primaryCpuAbi="arm64-v8a" publicFlags="-2147483648" targetSdkVersion="35"/>
                </packages>"#,
            )
            .unwrap();
            std::fs::write(
                self.0.join("system/packages.xml"),
                aim_android_xml::abx::write(&xml).unwrap(),
            )
            .unwrap();
        }
        fn list(&self, uid: u32) {
            std::fs::write(self.0.join("system/packages.list"), format!(
                "example.app {uid} 0 /data/user/0/legacy default:targetSdkVersion=35 3003,3002 0 7 0 @null\n"
            )).unwrap();
        }
    }
    impl Drop for Data {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    /// Pin the old join's authority for every Start input, including a
    /// differing list UID/data path and the per-user process GID projection.
    #[test]
    fn legacy_launch_inputs_match_the_disk_join() {
        let data = Data::new();
        data.settings();
        data.list(PER_USER_RANGE + 10_123);
        let baseline = package::State::read(&data.0, &[0]).unwrap().unwrap();
        let setting = &baseline.settings.packages[0];
        let listed = &baseline.list[0];
        let input = Inputs::legacy(&data.0, "example.app").unwrap();
        assert_eq!(input.uid, listed.uid);
        assert_eq!(input.permission_gids, listed.gids);
        assert_eq!(input.seinfo, listed.seinfo);
        assert_eq!(input.data_dir, listed.data_dir);
        assert_eq!(input.flags, setting.flags);
        assert_eq!(input.target_sdk, setting.target_sdk_version);
        assert_eq!(input.primary_cpu_abi, setting.primary_cpu_abi);
        assert_eq!(
            input
                .primary_cpu_abi
                .as_deref()
                .map(instruction_set)
                .as_deref(),
            Some("arm64")
        );
        assert_eq!(
            format!("{}{SEINFO_COMPLETE}", input.seinfo),
            "default:targetSdkVersion=35:complete"
        );
        assert_eq!(
            process_gids(
                input.permission_gids.clone(),
                input.uid,
                zygote::MountExternal::Default,
                false
            ),
            vec![3003, 3002, 50_123, 20_123, 109_997]
        );
        // Each launch still reads disk; an already acquired input retains its values.
        data.list(10_124);
        assert_eq!(Inputs::legacy(&data.0, "example.app").unwrap().uid, 10_124);
        assert_eq!(input.uid, PER_USER_RANGE + 10_123);
    }

    #[test]
    fn legacy_launch_input_errors_remain_explicit() {
        let data = Data::new();
        let error = Inputs::legacy(&data.0, "example.app").err().unwrap();
        assert_eq!(error.code, EX_ILLEGAL_STATE);
        assert_eq!(
            error.message,
            "no package state: Android never booted on this data"
        );
        data.settings();
        assert_eq!(
            Inputs::legacy(&data.0, "unknown").err().unwrap().message,
            "unknown package unknown"
        );
        assert_eq!(
            Inputs::legacy(&data.0, "example.app")
                .err()
                .unwrap()
                .message,
            "example.app not in packages.list"
        );
        std::fs::write(data.0.join("system/packages.xml"), b"invalid").unwrap();
        assert!(
            Inputs::legacy(&data.0, "example.app")
                .err()
                .unwrap()
                .message
                .starts_with("package state:")
        );
    }
}
