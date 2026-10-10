//! Settings mutation decisions ported from AOSP android-16.0.0_r1.
//! Copyright The Android Open Source Project, Apache License 2.0.
use super::{apps_filter, query::Query};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Reader};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

#[derive(Clone, Debug, PartialEq)]
pub struct BlockUninstall {
    pub package: Option<String>,
    pub blocked: bool,
    pub user: i32,
}
impl BlockUninstall {
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, i32> {
        let args = pm::SetBlockUninstallForUser::read(reader)?;
        if reader.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        Ok(Self {
            package: args.package_name,
            blocked: args.block_uninstall,
            user: args.user_id,
        })
    }
    /// The original has no cross-user, user-existence or package-existence gate.
    pub fn decide(
        &self,
        query: &Query<'_>,
    ) -> Result<Result<bool, Exception>, apps_filter::NotModelled> {
        let uid = query.calling_uid;
        if !matches!(
            apps_filter::app_id(uid),
            apps_filter::ROOT_UID | apps_filter::SYSTEM_UID
        ) && (apps_filter::is_isolated(uid)
            || !query.uid_has_permission(uid, "android.permission.DELETE_PACKAGES")?)
        {
            return Ok(Err(Exception::security(
                "setBlockUninstallForUser requires DELETE_PACKAGES",
            )));
        }
        let package = self
            .package
            .as_ref()
            .and_then(|name| query.state.packages.get(name));
        Ok(Ok(!package
            .and_then(|package| package.pkg.as_ref())
            .is_some_and(|package| {
                package.sdk_library_name.is_some() || package.static_shared_library_name.is_some()
            })))
    }
}

/// Policy queried from the retained original device/protected-package owners.
/// None means no live owner supplied it, never an allow decision.
#[derive(Clone, Copy, Debug)]
pub struct HiddenPolicy {
    pub active_device_admin: Option<bool>,
    pub protected: Option<bool>,
    pub shell_restricted: Option<bool>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Hidden {
    pub package: Option<String>,
    pub hidden: bool,
    pub user: i32,
}
impl Hidden {
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, i32> {
        let args = pm::SetApplicationHiddenSettingAsUser::read(reader)?;
        if reader.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        Ok(Self {
            package: args.package_name,
            hidden: args.hidden,
            user: args.user_id,
        })
    }
    /// A plan still requires real kill/broadcast/persistence effects before the
    /// Binder endpoint can return true; this decision publishes nothing.
    pub fn decide(
        &self,
        query: &Query<'_>,
        policy: HiddenPolicy,
    ) -> Result<Result<Option<super::write::mutation::Plan>, Exception>, apps_filter::NotModelled>
    {
        let uid = query.calling_uid;
        if !matches!(
            apps_filter::app_id(uid),
            apps_filter::ROOT_UID | apps_filter::SYSTEM_UID
        ) && (apps_filter::is_isolated(uid)
            || !query.uid_has_permission(uid, "android.permission.MANAGE_USERS")?)
        {
            return Ok(Err(Exception::security(
                "setApplicationHiddenSettingAsUser requires MANAGE_USERS",
            )));
        }
        if let Err(error) =
            query.full_cross_user_with_shell(self.user, true, policy.shell_restricted)?
        {
            return Ok(Err(error));
        }
        if self.hidden {
            if policy.active_device_admin.ok_or(apps_filter::NotModelled(
                "hidden active-device-admin owner unavailable",
            ))? {
                return Ok(Ok(None));
            }
        }
        if self.package.as_deref() == Some("android") {
            return Ok(Ok(None));
        }
        let Some(package) = self
            .package
            .as_ref()
            .and_then(|name| query.state.packages.get(name))
        else {
            return Ok(Ok(None));
        };
        let user = super::info::user_state(package, self.user);
        if user.hidden == self.hidden
            || !user.installed
            || query.filtered_including_uninstalled(Some(package), self.user)?
        {
            return Ok(Ok(None));
        }
        if package.pkg.as_ref().is_some_and(|package| {
            package.sdk_library_name.is_some() || package.static_shared_library_name.is_some()
        }) {
            return Ok(Ok(None));
        }
        if self.hidden
            && apps_filter::app_id(uid) != package.app_id
            && policy.protected.ok_or(apps_filter::NotModelled(
                "hidden protected-package owner unavailable",
            ))?
        {
            return Ok(Ok(None));
        }
        Ok(Ok(Some(super::write::mutation::Plan {
            package: package.name.clone(),
            user: Some(self.user),
            change: super::write::mutation::Change::Hidden(self.hidden),
        })))
    }
}

/// Settings' SparseArray<ArraySet<String>>, including runtime null names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UninstallBlocks(pub std::collections::BTreeMap<i32, Vec<Option<String>>>);
impl UninstallBlocks {
    pub fn set(&mut self, request: &BlockUninstall) {
        if request.blocked {
            let names = self.0.entry(request.user).or_default();
            if !names.contains(&request.package) {
                names.push(request.package.clone());
                // ArraySet orders by hash, preserving insertion order on collisions.
                names.sort_by_key(|name| name.as_deref().map(super::info::java_hash).unwrap_or(0));
            }
        } else if let Some(names) = self.0.get_mut(&request.user) {
            names.retain(|name| name != &request.package);
            if names.is_empty() {
                self.0.remove(&request.user);
            }
        }
    }
    pub fn get(&self, user: i32, package: Option<&str>) -> bool {
        self.0
            .get(&user)
            .is_some_and(|names| names.iter().any(|name| name.as_deref() == package))
    }
    pub fn from_state(state: &super::State) -> Self {
        let mut result = Self::default();
        for (id, user) in &state.users {
            for package in &user.restrictions.block_uninstall {
                result.set(&BlockUninstall {
                    package: Some(package.clone()),
                    blocked: true,
                    user: *id as i32,
                });
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_decision_requires_admin_owner_and_never_hides_platform_package() {
        let state = super::super::model::State::default();
        let filter = apps_filter::AppsFilter::new(&state, &apps_filter::Config::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 0,
        };
        let request = Hidden {
            package: Some("android".into()),
            hidden: true,
            user: 0,
        };
        let policy = HiddenPolicy {
            active_device_admin: None,
            protected: None,
            shell_restricted: None,
        };
        assert_eq!(
            request.decide(&query, policy).unwrap_err(),
            apps_filter::NotModelled("hidden active-device-admin owner unavailable")
        );
        assert!(
            request
                .decide(
                    &query,
                    HiddenPolicy {
                        active_device_admin: Some(false),
                        ..policy
                    }
                )
                .unwrap()
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn uninstall_block_library_veto_applies_even_when_unblocking() {
        let mut state = super::super::model::State::default();
        for sdk in [false, true] {
            state.packages.insert(
                "library".into(),
                super::super::model::PackageState {
                    name: "library".into(),
                    pkg: Some(std::sync::Arc::new(super::super::pkg::AndroidPackage {
                        sdk_library_name: sdk.then(|| "sdk".into()),
                        static_shared_library_name: (!sdk).then(|| "static".into()),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            );
            let filter =
                apps_filter::AppsFilter::new(&state, &apps_filter::Config::default()).unwrap();
            let query = Query {
                state: &state,
                filter: &filter,
                calling_uid: 0,
            };
            for blocked in [false, true] {
                assert!(
                    !BlockUninstall {
                        package: Some("library".into()),
                        blocked,
                        user: 0
                    }
                    .decide(&query)
                    .unwrap()
                    .unwrap()
                );
            }
        }
    }
    #[test]
    fn array_set_preserves_hash_collision_order_and_removes_empty_user() {
        let mut blocks = UninstallBlocks::default();
        for package in [Some("BB"), Some("Aa"), None, Some("BB")] {
            blocks.set(&BlockUninstall {
                package: package.map(String::from),
                blocked: true,
                user: -99,
            });
        }
        assert_eq!(
            blocks.0[&-99],
            vec![None, Some("BB".into()), Some("Aa".into())]
        );
        for package in [None, Some("Aa"), Some("BB")] {
            assert!(blocks.get(-99, package));
            blocks.set(&BlockUninstall {
                package: package.map(String::from),
                blocked: false,
                user: -99,
            });
        }
        assert!(!blocks.0.contains_key(&-99));
    }
    #[test]
    fn decoder_retains_nullable_name_and_rejects_extra_data() {
        let mut parcel = aim_binder_host::parcel::Parcel::new();
        pm::SetBlockUninstallForUser {
            package_name: None,
            block_uninstall: true,
            user_id: -99,
        }
        .write(&mut parcel);
        assert_eq!(
            BlockUninstall::read(&mut Reader::new(parcel.data(), &[]))
                .unwrap()
                .package,
            None
        );
        parcel.write_i32(7);
        assert_eq!(
            BlockUninstall::read(&mut Reader::new(parcel.data(), &[])).unwrap_err(),
            BAD_VALUE
        );
    }
    #[test]
    fn root_can_address_missing_package_and_user_without_cross_user_gate() {
        let state = super::super::model::State::default();
        let filter = apps_filter::AppsFilter::new(&state, &apps_filter::Config::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 0,
        };
        assert_eq!(
            BlockUninstall {
                package: Some("absent".into()),
                blocked: true,
                user: -99
            }
            .decide(&query)
            .unwrap()
            .unwrap(),
            true
        );
    }
}
