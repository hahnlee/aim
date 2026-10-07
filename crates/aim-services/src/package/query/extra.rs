//! Additional snapshot queries from the pinned ComputerEngine and PMS owner.
use super::*;

struct VersionedPackage {
    name: Option<String>,
    version: i64,
}
impl ReadParcelable for VersionedPackage {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self {
            name: r.read_string8()?,
            version: r.read_i64()?,
        })
    }
}

impl Query<'_> {
    pub(super) fn extra_package(&self, code: u32, r: &mut Reader<'_>) -> Answered {
        match code {
            pm::CHECK_PERMISSION => {
                let a = args(pm::CheckPermission::read(r))?;
                let value = self.check_package_permission(
                    a.pkg_name.as_deref(),
                    a.perm_name.as_deref(),
                    a.user_id,
                )?;
                Ok(reply(|p| pm::write_check_permission_reply(p, value)))
            }
            pm::CHECK_UID_PERMISSION => {
                let a = args(pm::CheckUidPermission::read(r))?;
                let value = self.check_uid_permission(a.uid, a.perm_name.as_deref())?;
                Ok(reply(|p| pm::write_check_uid_permission_reply(p, value)))
            }
            pm::GET_APP_OP_PERMISSION_PACKAGES => {
                let a = args(pm::GetAppOpPermissionPackages::read(r))?;
                thrown(
                    self.appop_packages(a.permission_name.as_deref(), a.user_id),
                    |p, v| pm::write_get_app_op_permission_packages_reply(p, &Some(v)),
                )
            }
            pm::GET_SYSTEM_SHARED_LIBRARY_NAMES => {
                args(pm::GetSystemSharedLibraryNames::read(r))?;
                let names = self.system_library_names()?;
                Ok(reply(|p| {
                    pm::write_get_system_shared_library_names_reply(p, &names)
                }))
            }
            pm::IS_INSTANT_APP => {
                let a = args(pm::IsInstantApp::read(r))?;
                thrown(
                    self.is_instant(a.package_name.as_deref().unwrap_or_default(), a.user_id),
                    pm::write_is_instant_app_reply,
                )
            }
            pm::CURRENT_TO_CANONICAL_PACKAGE_NAMES => {
                let a = args(pm::CurrentToCanonicalPackageNames::read(r))?;
                let names = self.translate_names(a.names.as_deref(), false)?;
                Ok(reply(|p| {
                    pm::write_current_to_canonical_package_names_reply(p, &names)
                }))
            }
            pm::CANONICAL_TO_CURRENT_PACKAGE_NAMES => {
                let a = args(pm::CanonicalToCurrentPackageNames::read(r))?;
                let names = self.translate_names(a.names.as_deref(), true)?;
                Ok(reply(|p| {
                    pm::write_canonical_to_current_package_names_reply(p, &names)
                }))
            }
            pm::GET_FLAGS_FOR_UID => {
                let a = args(pm::GetFlagsForUid::read(r))?;
                let value = self.uid_flags(a.uid, false)?;
                Ok(reply(|p| pm::write_get_flags_for_uid_reply(p, value)))
            }
            pm::GET_PRIVATE_FLAGS_FOR_UID => {
                let a = args(pm::GetPrivateFlagsForUid::read(r))?;
                let value = self.uid_flags(a.uid, true)?;
                Ok(reply(|p| {
                    pm::write_get_private_flags_for_uid_reply(p, value)
                }))
            }
            pm::GET_SPLASH_SCREEN_THEME => {
                let a = args(pm::GetSplashScreenTheme::read(r))?;
                let value = match self.enforce_cross_user(
                    a.user_id,
                    false,
                    false,
                    "getSplashScreenTheme",
                )? {
                    Err(e) => {
                        return thrown(Ok(Err(e)), |p, v: Option<String>| {
                            pm::write_get_splash_screen_theme_reply(p, &v)
                        });
                    }
                    Ok(()) => self
                        .visible_user_state(
                            a.package_name.as_deref().unwrap_or_default(),
                            a.user_id,
                        )?
                        .and_then(|u| u.splash_screen_theme),
                };
                Ok(reply(|p| {
                    pm::write_get_splash_screen_theme_reply(p, &value)
                }))
            }
            pm::GET_USER_MIN_ASPECT_RATIO => {
                let a = args(pm::GetUserMinAspectRatio::read(r))?;
                let value = self
                    .visible_user_state(a.package_name.as_deref().unwrap_or_default(), a.user_id)?
                    .map_or(0, |u| u.min_aspect_ratio);
                Ok(reply(|p| {
                    pm::write_get_user_min_aspect_ratio_reply(p, value)
                }))
            }
            pm::GET_APPLICATION_HIDDEN_SETTING_AS_USER => {
                let a = args(pm::GetApplicationHiddenSettingAsUser::read(r))?;
                thrown(
                    self.hidden_setting(a.package_name.as_deref().unwrap_or_default(), a.user_id),
                    pm::write_get_application_hidden_setting_as_user_reply,
                )
            }
            pm::GET_ALL_PACKAGES => {
                args(pm::GetAllPackages::read(r))?;
                thrown(self.all_packages(), |p, v| {
                    pm::write_get_all_packages_reply(p, &Some(v))
                })
            }
            pm::GET_PACKAGE_INFO_VERSIONED => {
                let a = args(pm::GetPackageInfoVersioned::<VersionedPackage>::read(r))?;
                let versioned = a
                    .versioned_package
                    .ok_or(NotModelled("a null versioned package"))?;
                thrown(
                    self.package_info(
                        versioned.name.as_deref().unwrap_or_default(),
                        versioned.version,
                        a.flags,
                        a.user_id,
                    ),
                    |p, v| pm::write_get_package_info_versioned_reply(p, v.as_ref()),
                )
            }
            pm::GET_PACKAGES_HOLDING_PERMISSIONS => {
                let a = args(pm::GetPackagesHoldingPermissions::read(r))?;
                let permissions = a
                    .permissions
                    .ok_or(NotModelled("a null permission array"))?;
                thrown(
                    self.packages_holding_permissions(&permissions, a.flags, a.user_id),
                    |p, v| {
                        pm::write_get_packages_holding_permissions_reply(
                            p,
                            Some(&ListSlice {
                                creator: "android.content.pm.PackageInfo".into(),
                                items: v,
                            }),
                        )
                    },
                )
            }
            pm::IS_PACKAGE_AVAILABLE => {
                let a = args(pm::IsPackageAvailable::read(r))?;
                thrown(
                    self.package_available(
                        a.package_name.as_deref().unwrap_or_default(),
                        a.user_id,
                    ),
                    pm::write_is_package_available_reply,
                )
            }
            pm::GET_PACKAGE_GIDS => {
                let a = args(pm::GetPackageGids::read(r))?;
                thrown(
                    self.package_gids(
                        a.package_name.as_deref().unwrap_or_default(),
                        a.flags,
                        a.user_id,
                    ),
                    |p, v| pm::write_get_package_gids_reply(p, &v),
                )
            }
            pm::GET_TARGET_SDK_VERSION => {
                let a = args(pm::GetTargetSdkVersion::read(r))?;
                let value =
                    self.target_sdk_version(a.package_name.as_deref().unwrap_or_default())?;
                Ok(reply(|p| pm::write_get_target_sdk_version_reply(p, value)))
            }
            pm::GET_UID_FOR_SHARED_USER => {
                let a = args(pm::GetUidForSharedUser::read(r))?;
                let value = self.shared_uid(a.shared_user_name.as_deref())?;
                Ok(reply(|p| pm::write_get_uid_for_shared_user_reply(p, value)))
            }
            pm::IS_UID_PRIVILEGED => {
                let a = args(pm::IsUidPrivileged::read(r))?;
                let value = self.uid_privileged(a.uid)?;
                Ok(reply(|p| pm::write_is_uid_privileged_reply(p, value)))
            }
            pm::GET_INSTALLER_PACKAGE_NAME => {
                let a = args(pm::GetInstallerPackageName::read(r))?;
                thrown(
                    self.installer_package(a.package_name.as_deref().unwrap_or_default()),
                    |p, v| pm::write_get_installer_package_name_reply(p, &v),
                )
            }
            pm::GET_INSTALL_REASON => {
                let a = args(pm::GetInstallReason::read(r))?;
                thrown(
                    self.install_reason(a.package_name.as_deref().unwrap_or_default(), a.user_id),
                    pm::write_get_install_reason_reply,
                )
            }
            pm::GET_MIME_GROUP => {
                let a = args(pm::GetMimeGroup::read(r))?;
                thrown(
                    self.mime_group(
                        a.package_name.as_deref().unwrap_or_default(),
                        a.group.as_deref(),
                    ),
                    |p, v| pm::write_get_mime_group_reply(p, &Some(v)),
                )
            }
            pm::HAS_SIGNING_CERTIFICATE => {
                let a = args(pm::HasSigningCertificate::read(r))?;
                let value = self.package_certificate(
                    a.package_name.as_deref().unwrap_or_default(),
                    a.signing_certificate.as_deref(),
                    a.flags,
                )?;
                Ok(reply(|p| pm::write_has_signing_certificate_reply(p, value)))
            }
            pm::HAS_UID_SIGNING_CERTIFICATE => {
                let a = args(pm::HasUidSigningCertificate::read(r))?;
                let details = self.uid_signatures(a.uid)?;
                let value = details
                    .as_ref()
                    .is_some_and(|s| certificate(s, a.signing_certificate.as_deref(), a.flags));
                Ok(reply(|p| {
                    pm::write_has_uid_signing_certificate_reply(p, value)
                }))
            }
            pm::CHECK_UID_SIGNATURES => {
                let a = args(pm::CheckUidSignatures::read(r))?;
                let (s1, s2) = (self.uid_signatures(a.uid1)?, self.uid_signatures(a.uid2)?);
                let value = match (s1, s2) {
                    (Some(s1), Some(s2)) => check_signatures(
                        &AndroidPackage {
                            signing_details: Some(s1),
                            ..Default::default()
                        },
                        &AndroidPackage {
                            signing_details: Some(s2),
                            ..Default::default()
                        },
                    ),
                    _ => SIGNATURE_UNKNOWN_PACKAGE,
                };
                Ok(reply(|p| pm::write_check_uid_signatures_reply(p, value)))
            }
            _ => Err(NotModelled("a method not modelled")),
        }
    }

    fn check_package_permission(
        &self,
        name: Option<&str>,
        permission: Option<&str>,
        user: i32,
    ) -> Result<i32, NotModelled> {
        if self.user(user).is_none() {
            return Ok(-1);
        }
        let ps = name.and_then(|name| self.state.packages.get(name));
        if ps.is_none() || self.filtered_including_uninstalled(ps, user)? {
            return Ok(-1);
        }
        let grants = user_state(ps.unwrap(), user).granted_permissions;
        Ok(if permission.is_some_and(|p| granted(&grants, p)) {
            0
        } else {
            -1
        })
    }

    fn check_uid_permission(&self, uid: i32, permission: Option<&str>) -> Result<i32, NotModelled> {
        let user = user_id(uid);
        if self.user(user).is_none() {
            return Ok(-1);
        }
        if apps_filter::is_sdk_sandbox(uid) || apps_filter::is_isolated(uid) {
            return Err(NotModelled("permission uid owner mapping"));
        }
        let packages: Vec<&PackageState> = match setting(self.state, app_id(uid)) {
            Some(Setting::Package(ps)) => vec![ps],
            Some(Setting::Shared(su)) => apps_filter::shared_packages(self.state, su).collect(),
            None => Vec::new(),
        };
        if let Some(ps) = packages
            .into_iter()
            .find(|ps| user_state(ps, user).installed && ps.pkg.is_some())
        {
            return Ok(
                if permission.is_some_and(|p| granted(&user_state(ps, user).granted_permissions, p))
                {
                    0
                } else {
                    -1
                },
            );
        }
        Err(NotModelled(
            "SystemConfig system-UID permission assignments",
        ))
    }

    fn appop_packages(&self, permission: Option<&str>, user: i32) -> Thrown<Vec<Option<String>>> {
        if let Err(e) = self.enforce_cross_user(user, false, false, "getAppOpPermissionPackages")? {
            return Ok(Err(e));
        }
        if permission.is_none()
            || self.user(user).is_none()
            || apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some()
        {
            return Ok(Ok(Vec::new()));
        }
        let mut names = Vec::new();
        for ps in self.packages_in_order() {
            let Some(pkg) = ps.pkg.as_ref() else {
                continue;
            };
            if pkg.is2(APEX)
                || !pkg
                    .requested_permissions
                    .iter()
                    .any(|p| Some(p.as_str()) == permission)
            {
                continue;
            }
            if !self.filtered_including_uninstalled(Some(ps), user)? {
                names.push(Some(pkg.package_name.clone()));
            }
        }
        Ok(Ok(names))
    }

    fn system_library_names(&self) -> Result<Option<Vec<Option<String>>>, NotModelled> {
        let libraries = self
            .state
            .shared_libraries
            .as_ref()
            .ok_or(NotModelled("the finalized shared library registry"))?;
        let mut names = Vec::new();
        for library in libraries {
            let visible = if library.kind != super::super::libraries::TYPE_STATIC {
                true
            } else {
                let ps = library
                    .package_name
                    .as_ref()
                    .and_then(|name| self.state.packages.get(name));
                ps.is_some()
                    && !self.filter_shared_lib(
                        ps.unwrap(),
                        user_id(self.calling_uid),
                        MATCH_STATIC_SHARED_AND_SDK_LIBRARIES,
                    )?
            };
            if visible && !names.contains(&library.name) {
                names.push(library.name.clone());
            }
        }
        names.sort_by_key(|name| name.as_deref().map_or(0, super::super::info::java_hash));
        Ok((!names.is_empty()).then_some(names))
    }

    fn is_instant(&self, name: &str, user: i32) -> Thrown<bool> {
        if let Err(e) = self.enforce_full_cross_user(user, "isInstantApp")? {
            return Ok(Err(e));
        }
        if apps_filter::is_isolated(self.calling_uid) {
            return Err(NotModelled("an isolated caller's owner"));
        }
        let Some(ps) = self.state.packages.get(name) else {
            return Ok(Ok(false));
        };
        if apps_filter::is_caller_same_app(self.state, Some(name), self.calling_uid)?
            || self.can_view_instant_apps()?
        {
            return Ok(Ok(user_state(ps, user).instant_app));
        }
        Err(NotModelled("instant app access grants"))
    }

    fn can_view_instant_apps(&self) -> Result<bool, NotModelled> {
        if self.calling_uid < 10000
            || self
                .uid_has_permission(self.calling_uid, "android.permission.ACCESS_INSTANT_APPS")?
        {
            return Ok(true);
        }
        if self.uid_has_permission(self.calling_uid, "android.permission.VIEW_INSTANT_APPS")? {
            return Err(NotModelled(
                "the default launcher and app prediction service",
            ));
        }
        Ok(false)
    }

    fn translate_names(
        &self,
        names: Option<&[Option<String>]>,
        canonical: bool,
    ) -> Result<Option<Vec<Option<String>>>, NotModelled> {
        let names = names.ok_or(NotModelled("a null package name array"))?;
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(Some(names.to_vec()));
        }
        let user = user_id(self.calling_uid);
        let can_view = self.can_view_instant_apps()?;
        let renamed = if canonical {
            Some(
                self.state
                    .renamed_packages
                    .as_ref()
                    .ok_or(NotModelled("the Settings renamed package table"))?,
            )
        } else {
            None
        };
        let mut result = Vec::with_capacity(names.len());
        for name in names {
            let ps = name.as_ref().and_then(|name| self.state.packages.get(name));
            let replacement = if let Some(renamed) = renamed {
                renamed
                    .iter()
                    .find(|(key, _)| Some(key) == name.as_ref())
                    .map(|(_, value)| value.clone())
            } else {
                ps.map(|ps| {
                    ps.real_name
                        .as_ref()
                        .ok_or(NotModelled("the package real-name owner"))
                })
                .transpose()?
                .and_then(Clone::clone)
            };
            let instant = ps.is_some_and(|ps| user_state(ps, user).instant_app);
            if replacement.is_some() && instant && !can_view {
                return Err(NotModelled("instant app access grants"));
            }
            result.push(replacement.or_else(|| name.clone()));
        }
        Ok(Some(result))
    }

    fn base_sdk_sandbox_uid(&self, uid: i32) -> Result<i32, NotModelled> {
        if !apps_filter::is_sdk_sandbox(uid) {
            return Ok(uid);
        }
        let name = self
            .state
            .system
            .sdk_sandbox_package
            .as_ref()
            .and_then(|n| n.as_deref())
            .ok_or(NotModelled("the base SDK sandbox package"))?;
        self.state
            .packages
            .get(name)
            .and_then(|ps| ps.pkg.as_ref())
            .map(|pkg| pkg.uid)
            .ok_or(NotModelled("the base SDK sandbox package code"))
    }

    fn uid_flags(&self, uid: i32, private: bool) -> Result<i32, NotModelled> {
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(0);
        }
        let uid = self.base_sdk_sandbox_uid(uid)?;
        let user = user_id(self.calling_uid);
        Ok(match setting(self.state, app_id(uid)) {
            Some(Setting::Shared(su)) if !self.shared_filtered(su, user, true)? => {
                if private {
                    su.private_flags
                } else {
                    su.flags
                }
            }
            Some(Setting::Package(ps))
                if !self.filtered_including_uninstalled(Some(ps), user)? =>
            {
                let (flags, private_flags) = ps
                    .setting_flags
                    .ok_or(NotModelled("raw package SettingBase flags"))?;
                if private { private_flags } else { flags }
            }
            _ => 0,
        })
    }

    fn visible_user_state(
        &self,
        name: &str,
        user: i32,
    ) -> Result<Option<PackageUserState>, NotModelled> {
        let ps = self.state.packages.get(name);
        if ps.is_none() || self.filtered_including_uninstalled(ps, user)? {
            return Ok(None);
        }
        Ok(Some(user_state(ps.unwrap(), user)))
    }

    fn hidden_setting(&self, name: &str, user: i32) -> Thrown<bool> {
        if !matches!(app_id(self.calling_uid), ROOT_UID | SYSTEM_UID)
            && !self.uid_has_permission(self.calling_uid, "android.permission.MANAGE_USERS")?
        {
            return Ok(Err(Exception::security("MANAGE_USERS permission required")));
        }
        if let Err(e) = self.enforce_full_cross_user(user, "getApplicationHidden for user")? {
            return Ok(Err(e));
        }
        Ok(Ok(self
            .visible_user_state(name, user)?
            .is_none_or(|u| u.hidden)))
    }

    fn all_packages(&self) -> Thrown<Vec<Option<String>>> {
        if !is_system_or_root_or_shell(self.calling_uid) {
            return Ok(Err(Exception::security(
                "getAllPackages is limited to privileged callers",
            )));
        }
        let user = user_id(self.calling_uid);
        let can_view = self.can_view_instant_apps()?;
        let mut result = Vec::new();
        for ps in self
            .packages_in_order()
            .into_iter()
            .filter(|ps| ps.pkg.is_some())
        {
            if !can_view && user_state(ps, user).instant_app {
                return Err(NotModelled("instant app access grants"));
            }
            result.push(Some(ps.pkg.as_ref().unwrap().package_name.clone()));
        }
        Ok(Ok(result))
    }

    pub(super) fn packages_holding_permissions(
        &self,
        permissions: &[Option<String>],
        flags: i64,
        user: i32,
    ) -> Thrown<Vec<PackageInfo>> {
        if self.user(user).is_none() {
            return Ok(Ok(Vec::new()));
        }
        let flags = match self.update_flags_for_package(flags, user)? {
            Ok(f) => f,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = self.enforce_full_cross_user(user, "get packages holding permissions")? {
            return Ok(Err(e));
        }
        let mut result = Vec::new();
        for ps in self.packages_in_order() {
            if ps.pkg.is_none() && flags & (MATCH_KNOWN_PACKAGES | MATCH_ARCHIVED_PACKAGES) == 0 {
                continue;
            }
            let state = user_state(ps, user);
            let granted: Vec<String> = permissions
                .iter()
                .filter_map(|p| p.as_ref())
                .filter(|p| state.granted_permissions.contains(p))
                .cloned()
                .collect();
            if granted.is_empty() {
                continue;
            }
            if let Some(mut info) = self.generate_package_info(ps, flags, user)? {
                if flags & GET_PERMISSIONS == 0 {
                    info.requested_permissions = Some(granted);
                }
                result.push(info);
            }
        }
        Ok(Ok(result))
    }

    fn package_available(&self, name: &str, user: i32) -> Thrown<bool> {
        if self.user(user).is_none() {
            return Ok(Ok(false));
        }
        if let Err(e) = self.enforce_cross_user(user, false, false, "is package available")? {
            return Ok(Err(e));
        }
        let Some((ps, _)) = self.package_of(name) else {
            return Ok(Ok(false));
        };
        let state = user_state(ps, user);
        Ok(Ok(!self.filtered(Some(ps), self.calling_uid, user)?
            && state.installed
            && !state.hidden))
    }

    fn package_gids(&self, name: &str, flags: i64, user: i32) -> Thrown<Option<Vec<i32>>> {
        if self.user(user).is_none() {
            return Ok(Ok(None));
        }
        let flags = match self.update_flags_for_package(flags, user)? {
            Ok(f) => f,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = self.enforce_cross_user(user, false, false, "getPackageGids")? {
            return Ok(Err(e));
        }
        let Some(ps) = self.state.packages.get(name) else {
            return Ok(Ok(None));
        };
        let u = user_state(ps, user);
        let system_match = flags & MATCH_SYSTEM_ONLY == 0 || ps.is.system;
        let available = (ps.pkg.is_some() && u.installed)
            || flags & (MATCH_KNOWN_PACKAGES | MATCH_ARCHIVED_PACKAGES) != 0;
        Ok(Ok((system_match
            && available
            && !self.filtered(Some(ps), self.calling_uid, user)?)
        .then(|| u.gids.clone())))
    }

    fn shared_uid(&self, name: Option<&str>) -> Result<i32, NotModelled> {
        if name.is_none()
            || apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some()
        {
            return Ok(-1);
        }
        match self.state.shared_users.get(name.unwrap()) {
            Some(su) if !self.shared_filtered(su, user_id(self.calling_uid), true)? => {
                Ok(su.app_id)
            }
            _ => Ok(-1),
        }
    }

    fn uid_privileged(&self, uid: i32) -> Result<bool, NotModelled> {
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(false);
        }
        let uid = self.base_sdk_sandbox_uid(uid)?;
        Ok(match setting(self.state, app_id(uid)) {
            Some(Setting::Package(ps)) => ps.is.privileged,
            Some(Setting::Shared(su)) => {
                apps_filter::shared_packages(self.state, su).any(|ps| ps.is.privileged)
            }
            None => false,
        })
    }

    fn installer_package(&self, name: &str) -> Thrown<Option<String>> {
        let Some(source) = self.install_source(name, user_id(self.calling_uid))? else {
            return Ok(Err(Exception::illegal_argument(format!(
                "Unknown package: {name}"
            ))));
        };
        let installer = source.and_then(|ps| ps.install_source.installer.as_ref());
        let Some(installer) = installer else {
            return Ok(Ok(None));
        };
        let ps = self.state.packages.get(installer);
        Ok(Ok((ps.is_some()
            && !self.filtered_including_uninstalled_not_archived(
                ps,
                self.calling_uid,
                user_id(self.calling_uid),
            )?)
        .then(|| installer.clone())))
    }

    fn enforce_full_cross_user(&self, user: i32, message: &str) -> Thrown<()> {
        if user < 0 {
            return Ok(Err(Exception::illegal_argument(format!(
                "Invalid userId {user}"
            ))));
        }
        if user_id(self.calling_uid) == user
            || matches!(app_id(self.calling_uid), ROOT_UID | SYSTEM_UID)
            || self.uid_has_permission(self.calling_uid, INTERACT_ACROSS_USERS_FULL)?
        {
            return Ok(Ok(()));
        }
        Ok(Err(Exception::security(format!(
            "{message}: UID {} requires {INTERACT_ACROSS_USERS_FULL} to access user {user}.",
            self.calling_uid
        ))))
    }

    fn install_reason(&self, name: &str, user: i32) -> Thrown<i32> {
        if let Err(e) = self.enforce_full_cross_user(user, "get install reason")? {
            return Ok(Err(e));
        }
        let ps = self.state.packages.get(name);
        if ps.is_none() || self.filtered_including_uninstalled(ps, user)? {
            return Ok(Ok(0));
        }
        Ok(Ok(user_state(ps.unwrap(), user).install_reason))
    }

    fn mime_group(&self, name: &str, group: Option<&str>) -> Thrown<Vec<Option<String>>> {
        if app_id(self.calling_uid) != SYSTEM_UID {
            let packages = self.packages_for_uid(self.calling_uid)?;
            if !packages
                .as_ref()
                .is_some_and(|p| p.iter().any(|p| p.as_deref() == Some(name)))
            {
                return Ok(Err(Exception::security(format!(
                    "Calling uid {} does not own package {name}",
                    self.calling_uid
                ))));
            }
            match self.package_info(name, VERSION_CODE_HIGHEST, 0, user_id(self.calling_uid))? {
                Err(e) => return Ok(Err(e)),
                Ok(None) => {
                    return Ok(Err(Exception::illegal_argument(format!(
                        "Unknown package {name} on user {}",
                        user_id(self.calling_uid)
                    ))));
                }
                Ok(Some(_)) => {}
            }
        }
        let Some(ps) = self.state.packages.get(name) else {
            return Ok(Ok(Vec::new()));
        };
        match ps.mime_groups.iter().find(|(n, _)| n.as_deref() == group) {
            Some((_, types)) => Ok(Ok(types.clone())),
            None => Ok(Err(Exception::illegal_argument(format!(
                "Unknown MIME group {} for package {name}",
                group.unwrap_or("null")
            )))),
        }
    }

    fn package_certificate(
        &self,
        name: &str,
        bytes: Option<&[u8]>,
        kind: i32,
    ) -> Result<bool, NotModelled> {
        let Some((ps, pkg)) = self.package_of(name) else {
            return Ok(false);
        };
        if self.filtered_including_uninstalled(Some(ps), user_id(self.calling_uid))? {
            return Ok(false);
        }
        Ok(pkg
            .signing_details
            .as_ref()
            .is_some_and(|s| certificate(s, bytes, kind)))
    }

    fn uid_signatures(
        &self,
        uid: i32,
    ) -> Result<Option<super::super::pkg::SigningDetails>, NotModelled> {
        let user = user_id(self.calling_uid);
        let signatures = match setting(self.state, app_id(uid)) {
            Some(Setting::Shared(su)) if !self.shared_filtered(su, user, true)? => {
                Some(su.signatures.as_ref())
            }
            Some(Setting::Package(ps))
                if !self.filtered_including_uninstalled(Some(ps), user)? =>
            {
                Some(ps.signatures.as_ref())
            }
            _ => None,
        };
        Ok(signatures.map(|s| match s {
            None => super::super::pkg::SigningDetails::default(),
            Some(s) => super::super::pkg::SigningDetails {
                signatures: Some(s.signatures.clone()),
                past_signing_certificates: s
                    .past_signatures
                    .as_ref()
                    .map(|p| p.iter().map(|(s, _)| s.clone()).collect()),
                scheme_version: s.scheme_version,
                ..Default::default()
            },
        }))
    }
}

fn certificate(s: &super::super::pkg::SigningDetails, bytes: Option<&[u8]>, kind: i32) -> bool {
    let Some(bytes) = bytes else {
        return false;
    };
    let matches = |cert: &[u8]| match kind {
        0 => cert == bytes,
        1 => {
            use sha2::Digest;
            sha2::Sha256::digest(cert).as_slice() == bytes
        }
        _ => false,
    };
    s.past_signing_certificates
        .as_ref()
        .is_some_and(|past| past.len() > 1 && past[..past.len() - 1].iter().any(|c| matches(c)))
        || matches!(s.signatures.as_deref(), Some([one]) if matches(one))
}

fn granted(grants: &[String], permission: &str) -> bool {
    let fuller = match permission {
        "android.permission.ACCESS_COARSE_LOCATION" => {
            Some("android.permission.ACCESS_FINE_LOCATION")
        }
        "android.permission.INTERACT_ACROSS_USERS" => Some(INTERACT_ACROSS_USERS_FULL),
        _ => None,
    };
    grants
        .iter()
        .any(|p| p == permission || Some(p.as_str()) == fuller)
}
