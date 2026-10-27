//! Process-start inputs retained from one published package generation.

use std::sync::Arc;

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use crate::package::{self, scan_snapshot::query_state::Capture};

pub(crate) struct Inputs {
    pub capture: Arc<Capture>,
    pub application: package::info::ApplicationInfo,
    pub permission_gids: Vec<u32>,
    pub flags: i32,
}

impl Inputs {
    pub(crate) fn capture(capture: Arc<Capture>, name: &str) -> Result<Self, Exception> {
        let (application, permission_gids, flags) = project(capture.state(), name)?;
        Ok(Self {
            capture,
            application,
            permission_gids,
            flags,
        })
    }
}

fn project(
    state: &package::model::State,
    name: &str,
) -> Result<(package::info::ApplicationInfo, Vec<u32>, i32), Exception> {
    let fail = |what: &str| Exception::new(EX_ILLEGAL_STATE, format!("{name}: {what}"));
    let ps = state
        .packages
        .get(name)
        .ok_or_else(|| Exception::illegal_argument(format!("unknown package {name}")))?;
    let user = ps.users.get(&0).ok_or_else(|| fail("no user 0 state"))?;
    let target = package::info::Target {
        sys: &state.system,
        pkg: ps
            .pkg
            .as_deref()
            .ok_or_else(|| fail("no executable code"))?,
        ps,
        state: user,
        user: 0,
    };
    let application = package::info::generate_application_info(&target, super::STOCK_PM_FLAGS)
        .ok_or_else(|| fail("no application info"))?;
    if application.uid < 0 {
        return Err(fail("no process UID"));
    }
    if application.se_info.is_none() {
        return Err(fail("no seInfo"));
    }
    let flags = ps.setting_flags.ok_or_else(|| fail("no setting flags"))?.0;
    let gids = user
        .gids
        .iter()
        .map(|gid| u32::try_from(*gid).map_err(|_| fail("negative permission GID")))
        .collect::<Result<_, _>>()?;
    Ok((application, gids, flags))
}

#[cfg(test)]
mod tests {
    use super::*;
    use package::{
        model::{PackageState, PackageUserState, State},
        pkg::AndroidPackage,
    };

    fn state() -> State {
        let mut pkg = AndroidPackage::default();
        pkg.package_name = "example.app".into();
        pkg.uid = 10123;
        pkg.target_sdk_version = 35;
        let mut ps = PackageState {
            name: pkg.package_name.clone(),
            app_id: pkg.uid,
            seinfo: Some("default:targetSdkVersion=35".into()),
            primary_cpu_abi: Some("arm64-v8a".into()),
            setting_flags: Some((i32::MIN, 0)),
            pkg: Some(Arc::new(pkg)),
            ..Default::default()
        };
        ps.users.insert(
            0,
            PackageUserState {
                installed: true,
                gids: vec![3003, 3002],
                ..Default::default()
            },
        );
        let mut state = State::default();
        state.packages.insert(ps.name.clone(), ps);
        state
    }

    #[test]
    fn launch_projection_uses_published_code_identity_and_gids() {
        let mut state = state();
        let (old, gids, flags) = project(&state, "example.app").unwrap();
        assert_eq!(old.uid, 10123);
        assert_eq!(old.target_sdk_version, 35);
        assert_eq!(old.primary_cpu_abi.as_deref(), Some("arm64-v8a"));
        assert_eq!(old.se_info.as_deref(), Some("default:targetSdkVersion=35"));
        assert_eq!(old.data_dir.as_deref(), Some("/data/user/0/example.app"));
        assert_eq!(gids, [3003, 3002]);
        assert_eq!(flags, i32::MIN);
        state
            .packages
            .get_mut("example.app")
            .unwrap()
            .users
            .get_mut(&0)
            .unwrap()
            .gids = vec![3006];
        assert_eq!(project(&state, "example.app").unwrap().1, [3006]);
        assert_eq!(gids, [3003, 3002]);
    }

    #[test]
    fn launch_projection_rejects_missing_owners() {
        let mut state = state();
        assert_eq!(
            project(&state, "unknown").err().unwrap().message,
            "unknown package unknown"
        );
        state.packages.get_mut("example.app").unwrap().seinfo = None;
        assert_eq!(
            project(&state, "example.app").err().unwrap().message,
            "example.app: no seInfo"
        );
    }
}
