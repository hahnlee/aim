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

    pub(crate) fn internal_has_cross_user_permission(&self, uid: i32, user: i32, full: bool) -> Thrown<bool> {
        match self.internal_enforce_cross_user(uid, user, full, false, "")? {
            Ok(()) => Ok(Ok(true)),
            Err(error) if error.code == aim_binder_host::parcel::EX_SECURITY => Ok(Ok(false)),
            Err(error) => Ok(Err(error)),
        }
    }

    pub(crate) fn internal_activity_supports_intent(&self, resolver: Option<&ComponentName>,
            component: Option<&ComponentName>, intent: Option<&super::super::intent::Intent>,
            resolved_type: Option<&str>, user: i32) -> Thrown<bool> {
        if let Err(error) = self.enforce_cross_user(user, false, false, "activitySupportsIntentAsUser")? { return Ok(Err(error)); }
        let Some(component) = component else {
            return Ok(Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "component is null")));
        };
        if resolver == Some(component) { return Ok(Ok(true)); }
        let Some((_, package)) = self.package_of(&component.package) else { return Ok(Ok(false)); };
        let Some(activity) = package.activities.iter().find(|activity| activity.main.component.name == component.class) else { return Ok(Ok(false)); };
        let name = self.internal_resolve_name(&component.package, VERSION_CODE_HIGHEST)?;
        let Some(state) = self.state.packages.get(&name) else { return Ok(Ok(false)); };
        if self.filtered_component(Some(state), component, 1, self.calling_uid, user)? { return Ok(Ok(false)); }
        let Some(intent) = intent else {
            return Ok(Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "intent is null")));
        };
        for info in &activity.main.component.intents {
            match info.filter.matches(intent.action.as_deref(), resolved_type,
                    intent.data.as_ref().and_then(|data| data.scheme()), intent.data.as_ref(),
                    intent.categories.as_deref(), false, None) {
                Ok(value) if value >= 0 => return Ok(Ok(true)),
                Ok(_) => {},
                Err(error) => return match error.binder_exception() {
                    Some(exception) => Ok(Err(exception)),
                    None => Err(NotModelled("activity URI matching exception has no Parcel envelope")),
                },
            }
        }
        Ok(Ok(false))
    }

    pub(crate) fn internal_activity_info(&self, component: Option<&ComponentName>, flags: i64,
            uid: i32, user: i32) -> Thrown<Option<info::ActivityInfo>> {
        if let Some(component) = component { return self.activity_info(component, flags, uid, user); }
        if self.user(user).is_none() { return Ok(Ok(None)); }
        let _flags = self.update_flags_for_component(flags, user)?;
        if let Err(error) = self.enforce_cross_user(user, false, true, "get activity info")? { return Ok(Err(error)); }
        Ok(Ok(None))
    }

    pub(crate) fn internal_activity_info_cross_profile(&self, component: Option<&ComponentName>, flags:i64,
            user:i32) -> Thrown<Option<info::ActivityInfo>> {
        if self.user(user).is_none(){return Ok(Ok(None));}
        let flags=self.update_flags_for_component(flags,user)? | MATCH_QUARANTINED_COMPONENTS;
        let Some(component)=component else{return Ok(Ok(None));};
        let Some((state,package))=self.package_of(&component.package) else{return Ok(Ok(None));};
        let Some(activity)=package.activities.iter().find(|activity|activity.main.component.name==component.class) else{return Ok(Ok(None));};
        if !info::is_enabled_and_matches(state,&activity.main,flags,user)
            || self.filtered_component(Some(state),component,1,self.calling_uid,user)?{return Ok(Ok(None));}
        let user_state=user_state(state,user);let target=self.target(state,package,&user_state,user);
        Ok(Ok(info::generate_activity_info(&target,activity,flags,None)))
    }

    pub(crate) fn internal_sync_providers(&self, safe_mode:bool)->Result<Vec<(String,info::ProviderInfo)>,NotModelled> {
        if apps_filter::instant_app_package_name(self.state,self.calling_uid)?.is_some(){return Ok(Vec::new());}
        let registry=self.state.package_registry.as_ref().ok_or(NotModelled("native provider authority registry unavailable"))?;
        let user=apps_filter::user_id(self.calling_uid);let mut result=Vec::new();
        for (name,registered) in registry.ordered_authorities().into_iter().rev() {
            if !registered.value.syncable {continue;}
            let Some((package,code))=self.package_of(&registered.package) else{continue;};
            if safe_mode&&!package.is.system{continue;}
            let state=user_state(package,user);let target=self.target(package,code,&state,user);
            let Some(application)=info::generate_application_info(&target,0) else{continue;};
            let Some(provider)=info::generate_provider_info(&target,&registered.value,0,Some(std::sync::Arc::new(application))) else{continue;};
            let component=ComponentName{package:registered.package.clone(),class:registered.value.main.component.name.clone()};
            if self.filtered_component(Some(package),&component,4,self.calling_uid,user)?{continue;}
            result.push((name.to_owned(),provider));
        }
        Ok(result)
    }

    pub(crate) fn internal_can_access_component(&self, component: Option<&ComponentName>, uid: i32,
            user: i32) -> Thrown<bool> {
        let Some(component) = component else {
            return Ok(Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "component is null")));
        };
        let name = self.internal_resolve_name(&component.package, VERSION_CODE_HIGHEST)?;
        let Some(state) = self.state.packages.get(&name) else { return Ok(Ok(false)); };
        Ok(Ok(!self.filtered_component(Some(state), component, 0, uid, user)?))
    }

    pub(crate) fn internal_visibility_allow_list(&self, name: Option<&str>, user: i32, check_only: bool) -> Result<Option<Vec<i32>>, NotModelled> {
        let Some(name) = name else { return Ok(None); };
        let system = Query { state: self.state, filter: self.filter, calling_uid: SYSTEM_UID };
        let name = system.internal_resolve_name(name, VERSION_CODE_HIGHEST)?;
        let Some(target) = self.state.packages.get(&name) else { return Ok(None); };
        if self.filter.is_force_queryable(target.app_id) { return Ok(None); }
        if check_only { return Ok(Some(Vec::new())); }
        let mut ids = std::collections::BTreeSet::new();
        for package in self.state.packages.values() {
            if package.app_id < 10000 || ids.contains(&package.app_id) { continue; }
            if !self.filter.should_filter(self.state, apps_filter::uid(user, package.app_id), target, user) {
                ids.insert(package.app_id);
            }
        }
        Ok(Some(ids.into_iter().collect()))
    }

    pub(crate) fn internal_library_users(&self, name: Option<&str>, version: i64, kind: i32,
            flags: i64, uid: i32, user: i32) -> Result<(Option<Vec<(String, i64)>>, Option<Vec<bool>>), NotModelled> {
        let mut users = Vec::new();
        let mut optional: Option<Vec<bool>> = None;
        for state in self.packages_in_order() {
            if !info::is_available(&user_state(state, user), flags) { continue; }
            let sdk = kind == 3;
            if kind == 2 || sdk {
                let matched = if sdk {
                    state.uses_sdk_libraries.iter().find(|library| Some(library.name.as_str()) == name)
                        .filter(|library| library.version_major == version).map(|library| library.optional)
                } else {
                    state.uses_static_libraries.iter().find(|(library, _)| Some(library.as_str()) == name)
                        .filter(|(_, expected)| *expected == version).map(|_| false)
                };
                let Some(is_optional) = matched else { continue; };
                if self.filtered(Some(state), uid, user)? { continue; }
                let dependent = match state.pkg.as_ref() {
                    Some(package) if package.static_shared_library_name.is_some() => package.manifest_package_name.clone()
                        .ok_or(NotModelled("static dependent manifest package owner unavailable"))?,
                    _ => state.name.clone(),
                };
                users.push((dependent, state.version_code));
                optional.get_or_insert_with(Vec::new).push(is_optional);
            } else if let Some(package) = state.pkg.as_deref() {
                if package.uses_libraries.iter().chain(&package.uses_optional_libraries).any(|library| Some(library.as_str()) == name)
                        && !self.filtered(Some(state), uid, user)? {
                    users.push((state.name.clone(), state.version_code));
                }
            }
        }
        Ok(((!users.is_empty()).then_some(users), optional))
    }

    pub(crate) fn internal_filter_candidate(&self, kind: i32, owner: Option<&str>, name: Option<&str>,
            id: i32, uid: i32, user: i32, uninstalled: bool) -> Result<bool, NotModelled> {
        let candidate = match kind {
            -1 if name.is_none() => None,
            0 => Some(name.and_then(|name| self.state.packages.get(name))
                .ok_or(NotModelled("current visibility candidate owner unavailable"))?),
            1 => Some(name.and_then(|name| self.state.disabled_system_packages.get(name))
                .ok_or(NotModelled("factory visibility candidate owner unavailable"))?),
            2 => match setting(self.state, id) {
                Some(Setting::Package(state)) if Some(state.name.as_str()) == name => Some(state),
                _ => return Err(NotModelled("detached UID visibility candidate owner unavailable")),
            },
            3 => Some(owner.and_then(|owner| self.state.shared_users.get(owner))
                .and_then(|group| group.native_packages.as_deref())
                .and_then(|members| members.iter().rev().find(|state| Some(state.name.as_str()) == name))
                .ok_or(NotModelled("retained shared visibility candidate owner unavailable"))?),
            _ => return Err(NotModelled("unknown package visibility candidate owner")),
        };
        apps_filter::should_filter_application_with_permission(self.state, self.filter, candidate, uid, self.calling_uid, user, uninstalled, true)
    }

    pub(crate) fn internal_caller_same_app(&self, name: Option<&str>, mut uid: i32,
            resolve_isolated: bool) -> Thrown<bool> {
        if apps_filter::is_sdk_sandbox(uid) {
            let owner = self.state.system.sdk_sandbox_package.as_ref()
                .ok_or(NotModelled("SDK sandbox package owner"))?;
            return Ok(Ok(name.is_some() && owner.as_deref() == name));
        }
        let parsed = name.and_then(|name| self.state.packages.get(name)).and_then(|ps| ps.pkg.as_deref());
        if resolve_isolated && apps_filter::is_isolated(uid) {
            let Some((_, owner)) = self.state.system.isolated_owners.iter().find(|(id, _)| *id == uid) else {
                return Ok(Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("No owner UID found for isolated UID {uid}"))));
            };
            uid = *owner;
        }
        Ok(Ok(parsed.is_some_and(|pkg| app_id(uid) == pkg.uid)))
    }

    pub(crate) fn internal_instant_package_name(&self, mut uid: i32) -> Thrown<Option<String>> {
        if apps_filter::is_isolated(uid) {
            let Some((_, owner)) = self.state.system.isolated_owners.iter().find(|(id, _)| *id == uid) else {
                return Ok(Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("No owner UID found for isolated UID {uid}"))));
            };
            uid = *owner;
        }
        if let Some(Setting::Package(state)) = setting(self.state, app_id(uid)) {
            if user_state(state, user_id(uid)).instant_app {
                let Some(pkg) = state.pkg.as_deref() else {
                    return Ok(Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,
                        "instant package parsed owner is null")));
                };
                return Ok(Ok(Some(pkg.package_name.clone())));
            }
        }
        Ok(Ok(None))
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

    pub(crate) fn internal_package_lookup_uid(&self, uid: i32, known_compute: bool) -> Thrown<i32> {
        let mut uid = self.internal_setting_uid(uid)?;
        if known_compute {
            let Some((_, owner)) = self.state.system.isolated_owners.iter().find(|(id, _)| *id == uid) else {
                return Ok(Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("No owner UID found for isolated UID {uid}"))));
            };
            uid = *owner;
        }
        Ok(Ok(uid))
    }

    pub(crate) fn internal_setting_uid(&self, mut uid: i32) -> Result<i32, NotModelled> {
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
        Ok(uid)
    }

    pub(crate) fn internal_uid_target_sdk(&self, uid: i32) -> Result<i32, NotModelled> {
        let uid = self.internal_setting_uid(uid)?;
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
