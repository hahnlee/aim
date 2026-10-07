//! PackageManagerInternal/Computer identity reads, android-16.0.0_r1 AOSP (Apache 2.0).
use super::*;
impl Query<'_> {
    pub(crate) fn internal_resolve_name(
        &self,
        name: &str,
        version: i64,
    ) -> Result<String, NotModelled> {
        let renamed = self
            .state
            .renamed_packages
            .as_ref()
            .ok_or(NotModelled("the Settings renamed package table"))?;
        let name = renamed
            .iter()
            .find(|(from, _)| from == name)
            .map_or(name, |(_, to)| to.as_str());
        Ok(self.resolve_internal_package_name(name, version))
    }

    pub(crate) fn internal_filtered_package_name(
        &self,
        name: &str,
        user: i32,
    ) -> Result<Option<String>, NotModelled> {
        let name = self.internal_resolve_name(name, VERSION_CODE_HIGHEST)?;
        let state = self.state.packages.get(&name);
        if self.filtered(state, self.calling_uid, user)? {
            return Ok(None);
        }
        Ok(state.map(|_| name))
    }

    pub(crate) fn internal_uid_target_sdk(&self, mut uid: i32) -> Result<i32, NotModelled> {
        if apps_filter::is_sdk_sandbox(uid) {
            let name = self
                .state
                .system
                .sdk_sandbox_package
                .as_ref()
                .and_then(|name| name.as_deref())
                .ok_or(NotModelled("SDK sandbox parsed package owner"))?;
            let name = self.internal_resolve_name(name, VERSION_CODE_HIGHEST)?;
            uid = self
                .state
                .packages
                .get(&name)
                .and_then(|state| state.pkg.as_deref())
                .ok_or(NotModelled("SDK sandbox parsed package owner"))?
                .uid;
        }
        Ok(match setting(self.state, app_id(uid)) {
            Some(Setting::Package(state)) => state
                .pkg
                .as_deref()
                .map_or(10000, |pkg| pkg.target_sdk_version),
            Some(Setting::Shared(shared)) => apps_filter::shared_packages(self.state, shared)
                .filter_map(|state| state.pkg.as_deref())
                .fold(10000, |sdk, pkg| sdk.min(pkg.target_sdk_version)),
            None => 10000,
        })
    }

    pub(crate) fn internal_same_app(
        &self,
        name: Option<&str>,
        flags: i64,
        comparison: i32,
        user: i32,
    ) -> Thrown<bool> {
        let Some(name) = name else {
            return Ok(Ok(false));
        };
        if apps_filter::is_sdk_sandbox(comparison) {
            let owner = self
                .state
                .system
                .sdk_sandbox_package
                .as_ref()
                .ok_or(NotModelled("SDK sandbox package owner"))?;
            return Ok(Ok(owner.as_deref() == Some(name)));
        }
        self.package_uid(name, flags, user).map(|result| {
            result.map(|uid| uid >= 0 && comparison >= 0 && app_id(uid) == app_id(comparison))
        })
    }
    pub(crate) fn internal_filter_uid(
        &self,
        target: i32,
        caller: i32,
    ) -> Result<bool, NotModelled> {
        if apps_filter::is_sdk_sandbox(target) {
            if caller == target {
                return Ok(false);
            }
            // The pinned client-UID comparison never matches the distinct sandbox UID.
            return Ok(true);
        }
        let query = Query {
            state: self.state,
            filter: self.filter,
            calling_uid: caller,
        };
        let user = user_id(target);
        match setting(self.state, app_id(target)) {
            Some(Setting::Package(ps)) => query.filtered_including_uninstalled(Some(ps), user),
            Some(Setting::Shared(shared)) => query.shared_filtered(shared, user, true),
            None => Ok(true),
        }
    }
    pub(crate) fn internal_can_query(&self, query_uid: i32, target: Option<&str>) -> Thrown<bool> {
        let Some(target) = target else {
            return Ok(Ok(true));
        };
        if query_uid == ROOT_UID {
            return Ok(Ok(true));
        }
        let Some(querying) = setting(self.state, app_id(query_uid)) else {
            return Ok(Ok(false));
        };
        let user = user_id(query_uid);
        let target_uid = match self.package_uid(target, 0, user)? {
            Ok(uid) => uid,
            Err(error) => return Ok(Err(error)),
        };
        if target_uid != -1 {
            let query = Query {
                state: self.state,
                filter: self.filter,
                calling_uid: query_uid,
            };
            return Ok(Ok(match setting(self.state, app_id(target_uid)) {
                Some(Setting::Package(ps)) => !query.filtered(Some(ps), query_uid, user)?,
                Some(Setting::Shared(shared)) => !query.shared_filtered(shared, user, false)?,
                None => false,
            }));
        }
        let declared = |ps: &PackageState| {
            ps.pkg.as_deref().is_some_and(|pkg| {
                pkg.queries_packages.iter().any(|name| name == target)
                    || pkg
                        .requested_permissions
                        .iter()
                        .any(|name| name == "android.permission.QUERY_ALL_PACKAGES")
            })
        };
        Ok(Ok(match querying {
            Setting::Package(ps) => declared(ps),
            Setting::Shared(shared) => {
                apps_filter::shared_packages(self.state, shared).any(declared)
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::model::{SharedUser, UidOwner};

    fn parsed(name: &str, app: i32, sdk: i32) -> PackageState {
        PackageState {
            name: name.into(),
            app_id: app,
            pkg: Some(Arc::new(AndroidPackage {
                package_name: name.into(),
                uid: app,
                target_sdk_version: sdk,
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    #[test]
    fn state_name_uses_one_rename_then_caller_visible_library_resolution() {
        let mut state = State::default();
        state.renamed_packages = Some(vec![("alias".into(), "library".into())]);
        let mut low = parsed("lib.old", 10101, 30);
        low.version_code = 10;
        let pkg = Arc::make_mut(low.pkg.as_mut().unwrap());
        pkg.static_shared_library_name = Some("library".into());
        pkg.static_shared_lib_version = 1;
        pkg.version_code = 10;
        let mut high = low.clone();
        high.name = "lib.new".into();
        high.version_code = 20;
        let pkg = Arc::make_mut(high.pkg.as_mut().unwrap());
        pkg.package_name = high.name.clone();
        pkg.static_shared_lib_version = 2;
        pkg.version_code = 20;
        state.packages.insert(low.name.clone(), low);
        state.packages.insert(high.name.clone(), high);
        let filter = AppsFilter::default();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: SYSTEM_UID,
        };
        assert_eq!(query.internal_resolve_name("alias", -1).unwrap(), "lib.new");
        assert_eq!(query.internal_resolve_name("alias", 10).unwrap(), "lib.old");
        assert_eq!(query.internal_resolve_name("absent", -1).unwrap(), "absent");
        assert_eq!(
            query
                .internal_filtered_package_name("alias", 0)
                .unwrap()
                .as_deref(),
            Some("lib.new")
        );
        let mut caller = parsed("client", 10102, 35);
        caller.uses_static_libraries.push(("library".into(), 1));
        state.packages.insert(caller.name.clone(), caller);
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 10102,
        };
        assert_eq!(query.internal_resolve_name("alias", -1).unwrap(), "lib.old");
        state.renamed_packages = None;
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: SYSTEM_UID,
        };
        assert!(query.internal_resolve_name("alias", -1).is_err());
    }

    #[test]
    fn uid_target_sdk_uses_setting_owner_retained_members_and_sdk_base() {
        let mut state = State::default();
        let group = SharedUser {
            name: "group".into(),
            app_id: 10005,
            native_packages: Some(vec![
                parsed("retained", 10005, 28),
                parsed("active", 10005, 35),
                PackageState::default(),
            ]),
            ..Default::default()
        };
        state.shared_users.insert(group.name.clone(), group);
        let package = parsed("base", 10006, 33);
        state.packages.insert(package.name.clone(), package.clone());
        state.uid_owners = Some(
            [
                (10005, UidOwner::SharedUser("group".into())),
                (10006, UidOwner::Package(Box::new(package))),
            ]
            .into(),
        );
        state.renamed_packages = Some(Vec::new());
        state.system.sdk_sandbox_package = Some(Some("base".into()));
        let filter = AppsFilter::default();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: SYSTEM_UID,
        };
        assert_eq!(query.internal_uid_target_sdk(1010005).unwrap(), 28);
        assert_eq!(query.internal_uid_target_sdk(10006).unwrap(), 33);
        assert_eq!(query.internal_uid_target_sdk(20123).unwrap(), 33);
        assert_eq!(query.internal_uid_target_sdk(-1).unwrap(), 10000);
        state.system.sdk_sandbox_package = None;
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: SYSTEM_UID,
        };
        assert!(query.internal_uid_target_sdk(20123).is_err());
    }
}
