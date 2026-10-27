//! `IPackageEnableBridge` for the app host: the native package boot's
//! feature configuration and installer storage primitive, which
//! system_server's `PackageEnableBridge` serves, to the system uid only.
//! Install updates clear installd code caches from the published inode receipts.
//! ART profile cleanup follows AppDataHelper's uninitialized-owner guard (#2063).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_ipackageenablebridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;
/// `android.content.pm.Flags.quarantinedEnabled`: a read-only flag the
/// pinned release builds off.
const QUARANTINED_ENABLED: bool = false;

pub struct PackageEnable {
    host: Arc<AppHost>,
}

impl PackageEnable {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageEnable { host }
    }

    fn clear_code_cache(
        &self,
        package: Option<&str>,
        users: Option<Vec<i32>>,
    ) -> Result<(), Exception> {
        let capture = self.host.package_capture()?;
        // Validate the full inventory before looking up installd or changing data.
        let requests = cache_requests(capture.state(), package, users)?;
        let installd = self
            .host
            .installd()
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        for request in requests {
            let mut data = Parcel::new();
            request.write(&mut data);
            let reply = installd
                .transact(
                    aim_service_aidl::android_os_iinstalld::CLEAR_APP_DATA,
                    &data,
                    false,
                )
                .map_err(|status| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("clearCodeCache installd transport: {status}"),
                    )
                })?;
            let mut reader = reply.reader();
            aim_service_aidl::android_os_iinstalld::read_clear_app_data_reply(&mut reader)
                .map_err(|status| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("clearCodeCache installd reply: {status}"),
                    )
                })??;
            if reader.remaining() != 0 {
                return Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "clearCodeCache installd reply tail",
                ));
            }
        }
        // AppDataHelper.destroyAppProfilesLIF skips while ART is uninitialized:
        // the runtime ignores stale/invalid profiles. ART itself remains #2063.
        Ok(())
    }
}

/// AppDataHelper.clearAppDataLIF(UserHandle.USER_ALL,
/// DE | CE | EXTERNAL | CLEAR_CODE_CACHE_ONLY), with one retained capture.
fn cache_requests(
    state: &crate::package::model::State,
    package: Option<&str>,
    users: Option<Vec<i32>>,
) -> Result<Vec<aim_service_aidl::android_os_iinstalld::ClearAppData>, Exception> {
    let package = package.ok_or_else(|| Exception::illegal_argument("Missing cache package"))?;
    let users = users.ok_or_else(|| Exception::illegal_argument("Missing cache users"))?;
    let setting = state.packages.get(package).ok_or_else(|| {
        Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,
            "Package not published",
        )
    })?;
    let mut seen = std::collections::BTreeSet::new();
    let mut requests = Vec::new();
    for user in users {
        if !seen.insert(user) || !state.users.contains_key(&user) {
            return Err(Exception::illegal_argument("Invalid cache user inventory"));
        }
        requests.push(aim_service_aidl::android_os_iinstalld::ClearAppData {
            uuid: setting.volume_uuid.clone(),
            package_name: Some(package.into()),
            user_id: user,
            flags: 1 | 2 | 4 | 0x20,
            ce_data_inode: crate::package::info::user_state(setting, user).ce_data_inode,
        });
    }
    Ok(requests)
}

impl Service for PackageEnable {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `Bridge.enforceSystemUid`.
        let system = call.sender_euid == SYSTEM_UID;
        let caller = || {
            if system {
                Ok(())
            } else {
                Err(Exception::security(
                    "native package leaf requires system UID",
                ))
            }
        };
        match call.code {
            bridge::QUARANTINE_ENABLED => {
                bridge::QuarantineEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().map(|()| {
                    let mut p = Parcel::new();
                    bridge::write_quarantine_enabled_reply(&mut p, QUARANTINED_ENABLED);
                    p
                }))
            }
            bridge::CLEAR_CODE_CACHE => {
                let args = bridge::ClearCodeCache::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    caller()
                        .and_then(|()| {
                            self.clear_code_cache(args.package_name.as_deref(), args.users)
                        })
                        .map(|()| {
                            let mut p = Parcel::new();
                            bridge::write_clear_code_cache_reply(&mut p);
                            p
                        }),
                )
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::EX_SECURITY;

    #[test]
    fn update_cache_requests_use_each_users_published_inode_and_original_flags() {
        use crate::package::model::{PackageState, PackageUserState, State, User};
        let state = State {
            users: [
                (
                    0,
                    User {
                        id: 0,
                        ..Default::default()
                    },
                ),
                (
                    10,
                    User {
                        id: 10,
                        ..Default::default()
                    },
                ),
            ]
            .into(),
            packages: [(
                "p".into(),
                PackageState {
                    name: "p".into(),
                    volume_uuid: Some("volume".into()),
                    users: [
                        (
                            0,
                            PackageUserState {
                                ce_data_inode: 711,
                                ..Default::default()
                            },
                        ),
                        (
                            10,
                            PackageUserState {
                                ce_data_inode: 988,
                                ..Default::default()
                            },
                        ),
                    ]
                    .into(),
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        let requests = cache_requests(&state, Some("p"), Some(vec![10, 0])).unwrap();
        let values = requests
            .into_iter()
            .map(|request| {
                let mut parcel = Parcel::new();
                request.write(&mut parcel);
                let decoded = aim_service_aidl::android_os_iinstalld::ClearAppData::read(
                    &mut parcel.reader(),
                )
                .unwrap();
                (
                    decoded.uuid,
                    decoded.package_name,
                    decoded.user_id,
                    decoded.flags,
                    decoded.ce_data_inode,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![
                (Some("volume".into()), Some("p".into()), 10, 0x27, 988),
                (Some("volume".into()), Some("p".into()), 0, 0x27, 711),
            ]
        );
        for users in [vec![-1], vec![0, 0], vec![0, 11]] {
            assert!(cache_requests(&state, Some("p"), Some(users)).is_err());
        }
        assert!(cache_requests(&state, Some("absent"), Some(vec![0])).is_err());
        assert!(cache_requests(&state, None, Some(vec![0])).is_err());
        assert!(cache_requests(&state, Some("p"), None).is_err());
    }

    /// The quarantine flag is the release's, to the system only.
    #[test]
    fn quarantine_follows_the_release() {
        let node = |euid: u32| {
            let process = crate::test_process(euid);
            let host = AppHost::new(process.clone());
            let aim_binder_host::parcel::Binder::Local(ptr) =
                process.add_service(Arc::new(PackageEnable::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let quarantine = |euid: u32| {
            let mut p = Parcel::new();
            bridge::QuarantineEnabled {}.write(&mut p);
            let reply = node(euid)
                .transact(bridge::QUARANTINE_ENABLED, &p, false)
                .unwrap();
            bridge::read_quarantine_enabled_reply(&mut reply.reader()).unwrap()
        };
        assert!(!quarantine(SYSTEM_UID).unwrap());
        assert_eq!(quarantine(10000).unwrap_err().code, EX_SECURITY);
    }
}
