//! `package` and `package_native`'s queries of packages and their parts,
//! answered from the model's state as `ComputerEngine`,
//! `IPackageManagerBase` and `PackageManagerNative` answer them at
//! `android-16.0.0_r1`: getPackageInfo, getApplicationInfo,
//! getInstallSourceInfo, the enabled settings, the system features, the
//! uid and name lookups, the component infos, the installed packages and
//! applications, checkSignatures and package_native's. Intent resolution
//! is the resolver's (`resolve.rs`). A path that depends on state the
//! feed does not give is not modelled: the call is reported, not
//! answered.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_binder_host::parcel::{EX_SECURITY, Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::ReadParcelable;
use aim_service_aidl::android_content_pm_ipackagemanager as pm;
use aim_service_aidl::android_content_pm_ipackagemanagernative as native;

use super::apps_filter::{
    self, AppsFilter, NotModelled, ROOT_UID, SYSTEM_UID, Setting, app_id,
    is_system_or_root_or_shell, setting, should_filter_application, user_id,
};
use super::feed::Feed;
use super::info::{
    self, ApplicationInfo, COMPONENT_ENABLED_STATE_DEFAULT, COMPONENT_ENABLED_STATE_DISABLED,
    COMPONENT_ENABLED_STATE_ENABLED, Extras, FLAG_SYSTEM, PackageInfo, Target, array_order,
    flags::*, generate_application_info, generate_package_info, user_state,
};
use super::intent::ComponentName;
use super::model::{PackageState, PackageUserState, State, System, User};
use super::pkg::{AndroidPackage, booleans, booleans2::APEX};
use super::resolve::Resolver;
use super::system_config::Properties;
use super::write::Writes;
use super::{reply, system_config};
use crate::shadow::{Answer, Check, ListSlice, ShadowCall, ShadowModel, Value};

/// How long a comparison waits for the feed to catch up with the
/// original.
const FRESH: Duration = Duration::from_secs(2);
/// `PackageManager.VERSION_CODE_HIGHEST`.
const VERSION_CODE_HIGHEST: i64 = -1;
/// `PackageManager.SIGNATURE_*`.
const SIGNATURE_MATCH: i32 = 0;
const SIGNATURE_NEITHER_SIGNED: i32 = 1;
const SIGNATURE_FIRST_NOT_SIGNED: i32 = -1;
const SIGNATURE_SECOND_NOT_SIGNED: i32 = -2;
const SIGNATURE_NO_MATCH: i32 = -3;
const SIGNATURE_UNKNOWN_PACKAGE: i32 = -4;
/// `IPackageManagerNative.LOCATION_*`.
const LOCATION_SYSTEM: i32 = 0x1;
const LOCATION_VENDOR: i32 = 0x2;
const LOCATION_PRODUCT: i32 = 0x4;
/// `ApplicationInfo.PRIVATE_FLAG_VENDOR`, `PRIVATE_FLAG_PRODUCT`.
const PRIVATE_FLAG_VENDOR: i32 = 1 << 18;
const PRIVATE_FLAG_PRODUCT: i32 = 1 << 19;
/// `UserInfo.FLAG_PROFILE`.
const USER_FLAG_PROFILE: i32 = 0x1000;
/// The aconfig flag `android.content.pm.remove_cross_user_permission_hack`.
const REMOVE_CROSS_USER_PERMISSION_HACK: &str =
    "android.content.pm.remove_cross_user_permission_hack";
/// The aconfig flag `android.content.pm.provide_info_of_apk_in_apex`.
const PROVIDE_INFO_OF_APK_IN_APEX: &str = "android.content.pm.provide_info_of_apk_in_apex";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const INSTALL_PACKAGES: &str = "android.permission.INSTALL_PACKAGES";

/// A query's answer: its reply, or the reason the original's path is not
/// modelled.
type Answered = Result<Parcel, NotModelled>;

impl ReadParcelable for ComponentName {
    /// `ComponentName(Parcel)`.
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(ComponentName {
            package: r.read_string16()?.unwrap_or_default(),
            class: r.read_string16()?.unwrap_or_default(),
        })
    }
}

/// The version of a library a package uses, by the library's name.
type UsedVersion = fn(&PackageState, &str) -> Option<i64>;

/// A package's install source as `getInstallSource` finds it: its state,
/// or none for an APEX (`InstallSource.EMPTY`).
type Source<'a> = Option<&'a PackageState>;

/// Where the model's state comes from: a state at least as new as the
/// original's at the instant given (`feed::Feed::fresh_since`), or none.
pub type States = Box<dyn Fn(Instant, Duration) -> Option<Arc<State>> + Send + Sync>;

/// `package` and `package_native`, modelled over the feed's state.
pub struct PackageModel {
    states: States,
    /// Intent resolution, and the apps filter of each state.
    resolver: Resolver,
    /// The same for the states answered from again, kept apart so that
    /// they do not evict the latest state's.
    before: Resolver,
    /// The writes, compared with the original's (slice B).
    writes: Writes,
    /// The not-modelled paths reported so far: native or not, the
    /// method's code, the reason.
    reported: Mutex<HashSet<(bool, u32, &'static str)>>,
}

impl PackageModel {
    pub fn new(states: States) -> Arc<PackageModel> {
        Arc::new(PackageModel {
            states,
            resolver: Resolver::default(),
            before: Resolver::default(),
            writes: Writes::default(),
            reported: Mutex::new(HashSet::new()),
        })
    }
}

/// Starts the package feed and the model of `package` and
/// `package_native` over it, for a shadow comparison of guest-init's
/// image `image` with the device's properties `props`; the feed's states
/// are written to `dump`, and installed packages are read through
/// `files`.
pub fn start(
    system: &Arc<crate::system::System>,
    image: &Path,
    props: Properties,
    dump: PathBuf,
    files: super::write::Files,
) -> Result<Arc<PackageModel>, String> {
    let platform = super::parse::Platform::load(image, Default::default())?;
    let join = Mutex::new(Join {
        framework: system_config::Framework::load(image)?,
        image: image.to_path_buf(),
        props,
        device: None,
        system: system.clone(),
        last: None,
        parsed: HashMap::new(),
    });
    let feed = Feed::start(system, Some(dump));
    let model = PackageModel::new(Box::new(move |since, t| {
        let fed = feed.fresh_since(since, t)?;
        join.lock().unwrap().state(fed)
    }));
    model.writes.watch_sessions(system);
    model
        .writes
        .read_apks(super::write::Apks { files, platform });
    Ok(model)
}

/// What the model joins to each fed state: the device's side (read when
/// the first state comes, when init has set the properties it needs),
/// the users' unlock state (UserManager's; a user unlocked stays so) and
/// each package's parsed package, decoded once per parcel.
struct Join {
    framework: system_config::Framework,
    image: PathBuf,
    props: Properties,
    device: Option<System>,
    system: Arc<crate::system::System>,
    /// The last fed state and the state made of it.
    last: Option<(Arc<State>, Arc<State>)>,
    /// Parsed packages by their parcel's address, with the parcel.
    parsed: HashMap<usize, (Arc<[u8]>, Arc<AndroidPackage>)>,
}

impl Join {
    fn state(&mut self, fed: Arc<State>) -> Option<Arc<State>> {
        let was = |last: &Option<(Arc<State>, Arc<State>)>, id: i32| {
            last.as_ref()
                .is_some_and(|(_, m)| m.users.get(&id).is_some_and(|u| u.unlocking_or_unlocked))
        };
        let mut unlocked = Vec::new();
        for &id in fed.users.keys() {
            if was(&self.last, id) || self.system.user_unlocking_or_unlocked(id).ok()? {
                unlocked.push(id);
            }
        }
        if let Some((f, m)) = &self.last
            && Arc::ptr_eq(f, &fed)
            && m.users
                .values()
                .all(|u| u.unlocking_or_unlocked == unlocked.contains(&u.id))
        {
            return Some(m.clone());
        }
        let device = self.device.get_or_insert_with(|| {
            system_config::system(&self.image, &self.props, &self.framework)
        });
        let mut state = (*fed).clone();
        state.system = System {
            implicit_access: fed.system.implicit_access.clone(),
            force_system_packages_queryable: fed.system.force_system_packages_queryable,
            force_queryable_packages: fed.system.force_queryable_packages.clone(),
            ..device.clone()
        };
        for user in state.users.values_mut() {
            user.unlocking_or_unlocked = unlocked.contains(&user.id);
        }
        let mut parsed = HashMap::new();
        let packages = state.packages.values_mut();
        for ps in packages.chain(state.disabled_system_packages.values_mut()) {
            let Some(parcel) = &ps.parcel else { continue };
            let key = parcel.as_ptr() as usize;
            let pkg = match self.parsed.remove(&key) {
                Some((_, pkg)) => Some(pkg),
                // A parcel that does not read leaves its package without
                // one: its answers differ, with the parcel in the dump.
                None => AndroidPackage::read_cache_entry(parcel).ok().map(Arc::new),
            };
            if let Some(pkg) = &pkg {
                parsed.insert(key, (parcel.clone(), pkg.clone()));
            }
            ps.pkg = pkg;
        }
        self.parsed = parsed;
        let state = Arc::new(state);
        self.last = Some((fed, state.clone()));
        Some(state)
    }
}

impl ShadowModel for PackageModel {
    fn answer(&self, call: &mut ShadowCall<'_>) -> Answer {
        let Some(state) = (self.states)(call.sent, FRESH) else {
            return Answer::NotModelled;
        };
        self.writes.observe(&state, call.dropped);
        if let Some(answer) = self.writes.answer(call) {
            return answer;
        }
        self.query(&self.resolver, &state, call)
    }

    fn answer_before(&self, call: &mut ShadowCall<'_>) -> Option<Answer> {
        let state = self.writes.state_before(call.sent)?;
        match self.query(&self.before, &state, call) {
            Answer::NotModelled => None,
            answer => Some(answer),
        }
    }

    fn decode_reply(
        &self,
        descriptor: &str,
        code: u32,
        r: &mut Reader<'_>,
    ) -> Option<ParcelResult<Value>> {
        if descriptor == pm::DESCRIPTOR
            && let Some(v) = self.resolver.decode_reply(code, r)
        {
            return Some(v);
        }
        reply::decode(descriptor, code, r)
    }

    fn checks(&self) -> Vec<Check> {
        self.writes.checks(&self.states)
    }
}

impl PackageModel {
    /// Answers a query from `state`, resolving intents with `resolver`.
    fn query(&self, resolver: &Resolver, state: &Arc<State>, call: &mut ShadowCall<'_>) -> Answer {
        if let Some(answer) = resolver.answer(state, call) {
            return answer;
        }
        let resolution = resolver.resolution(state);
        let q = Query {
            state,
            filter: &resolution.apps_filter,
            calling_uid: call.sender_euid as i32,
        };
        let answered = q.answer(call.descriptor, call.code, &mut call.data);
        match answered {
            Ok(reply) => Answer::Reply(reply),
            Err(NotModelled(reason)) => {
                let key = (call.descriptor == native::DESCRIPTOR, call.code, reason);
                if self.reported.lock().unwrap().insert(key) {
                    eprintln!(
                        "package shadow: {}#{} not modelled: {reason}",
                        call.descriptor, call.code
                    );
                }
                Answer::NotModelled
            }
        }
    }
}

/// A call's view of the state.
pub struct Query<'a> {
    pub state: &'a State,
    pub filter: &'a AppsFilter,
    /// `Binder.getCallingUid()`.
    pub calling_uid: i32,
}

/// A request's arguments, or not modelled when they do not read.
fn args<T>(read: ParcelResult<T>) -> Result<T, NotModelled> {
    read.map_err(|_| NotModelled("a call that does not read"))
}

fn exception(code: i32, message: String) -> Parcel {
    let mut p = Parcel::new();
    p.write_exception(&Exception::new(code, message));
    p
}

fn reply(write: impl FnOnce(&mut Parcel)) -> Parcel {
    let mut p = Parcel::new();
    write(&mut p);
    p
}

/// A query that may throw: its exception, as the original throws it.
type Thrown<T> = Result<Result<T, Exception>, NotModelled>;

/// The reply of a query that may throw.
fn thrown<T>(r: Thrown<T>, write: impl FnOnce(&mut Parcel, T)) -> Answered {
    Ok(match r? {
        Ok(v) => reply(|p| write(p, v)),
        Err(e) => exception(e.code, e.message),
    })
}

impl Query<'_> {
    /// Answers a package query with the caller's Binder identity. The
    /// reader includes the interface token, checked by generated AIDL.
    /// An unavailable state dependency remains an explicit error.
    pub fn answer(&self, descriptor: &str, code: u32, data: &mut Reader<'_>) -> Answered {
        match descriptor {
            pm::DESCRIPTOR => self.package(code, data),
            native::DESCRIPTOR => self.native(code, data),
            _ => Err(NotModelled("another interface")),
        }
    }

    fn package(&self, code: u32, r: &mut Reader<'_>) -> Answered {
        match code {
            pm::GET_PACKAGE_INFO => {
                let a = args(pm::GetPackageInfo::read(r))?;
                let name = a.package_name.unwrap_or_default();
                thrown(
                    self.package_info(&name, VERSION_CODE_HIGHEST, a.flags, a.user_id),
                    |p, v| pm::write_get_package_info_reply(p, v.as_ref()),
                )
            }
            pm::GET_APPLICATION_INFO => {
                let a = args(pm::GetApplicationInfo::read(r))?;
                let name = a.package_name.unwrap_or_default();
                thrown(self.application_info(&name, a.flags, a.user_id), |p, v| {
                    pm::write_get_application_info_reply(p, v.as_ref())
                })
            }
            pm::GET_INSTALL_SOURCE_INFO => {
                let a = args(pm::GetInstallSourceInfo::read(r))?;
                let name = a.package_name.unwrap_or_default();
                thrown(self.install_source_info(&name, a.user_id), |p, v| {
                    pm::write_get_install_source_info_reply(p, v.as_ref())
                })
            }
            pm::GET_COMPONENT_ENABLED_SETTING => {
                let a = args(pm::GetComponentEnabledSetting::<ComponentName>::read(r))?;
                thrown(
                    self.component_enabled_setting(a.component_name.as_ref(), a.user_id),
                    pm::write_get_component_enabled_setting_reply,
                )
            }
            pm::GET_APPLICATION_ENABLED_SETTING => {
                let a = args(pm::GetApplicationEnabledSetting::read(r))?;
                let name = a.package_name.unwrap_or_default();
                thrown(
                    self.application_enabled_setting(&name, a.user_id),
                    pm::write_get_application_enabled_setting_reply,
                )
            }
            pm::HAS_SYSTEM_FEATURE => {
                let a = args(pm::HasSystemFeature::read(r))?;
                let has = self.has_system_feature(a.name.as_deref(), a.version);
                Ok(reply(|p| pm::write_has_system_feature_reply(p, has)))
            }
            pm::GET_SYSTEM_AVAILABLE_FEATURES => {
                args(pm::GetSystemAvailableFeatures::read(r))?;
                let list = self.system_available_features();
                Ok(reply(|p| {
                    pm::write_get_system_available_features_reply(p, Some(&list))
                }))
            }
            pm::GET_PACKAGES_FOR_UID => {
                let a = args(pm::GetPackagesForUid::read(r))?;
                thrown(self.packages_for_uid_checked(a.uid), |p, v| {
                    pm::write_get_packages_for_uid_reply(p, &v)
                })
            }
            pm::GET_NAME_FOR_UID => {
                let a = args(pm::GetNameForUid::read(r))?;
                let name = self.name_for_uid(a.uid)?;
                Ok(reply(|p| pm::write_get_name_for_uid_reply(p, &name)))
            }
            pm::GET_NAMES_FOR_UIDS => {
                let a = args(pm::GetNamesForUids::read(r))?;
                let names = self.names_for_uids(a.uids.as_deref())?;
                Ok(reply(|p| pm::write_get_names_for_uids_reply(p, &names)))
            }
            pm::GET_PACKAGE_UID => {
                let a = args(pm::GetPackageUid::read(r))?;
                let name = a.package_name.unwrap_or_default();
                thrown(
                    self.package_uid(&name, a.flags, a.user_id),
                    pm::write_get_package_uid_reply,
                )
            }
            pm::GET_ACTIVITY_INFO => {
                let a = args(pm::GetActivityInfo::<ComponentName>::read(r))?;
                let c = a.class_name.ok_or(NotModelled("a null component"))?;
                thrown(
                    self.activity_info(&c, a.flags, self.calling_uid, a.user_id),
                    |p, v| pm::write_get_activity_info_reply(p, v.as_ref()),
                )
            }
            pm::GET_RECEIVER_INFO => {
                let a = args(pm::GetReceiverInfo::<ComponentName>::read(r))?;
                let c = a.class_name.ok_or(NotModelled("a null component"))?;
                thrown(self.receiver_info(&c, a.flags, a.user_id), |p, v| {
                    pm::write_get_receiver_info_reply(p, v.as_ref())
                })
            }
            pm::GET_SERVICE_INFO => {
                let a = args(pm::GetServiceInfo::<ComponentName>::read(r))?;
                let c = a.class_name.ok_or(NotModelled("a null component"))?;
                thrown(self.service_info(&c, a.flags, a.user_id), |p, v| {
                    pm::write_get_service_info_reply(p, v.as_ref())
                })
            }
            pm::GET_PROVIDER_INFO => {
                let a = args(pm::GetProviderInfo::<ComponentName>::read(r))?;
                let c = a.class_name.ok_or(NotModelled("a null component"))?;
                thrown(self.provider_info(&c, a.flags, a.user_id), |p, v| {
                    pm::write_get_provider_info_reply(p, v.as_ref())
                })
            }
            pm::GET_INSTALLED_PACKAGES => {
                let a = args(pm::GetInstalledPackages::read(r))?;
                thrown(self.installed_packages(a.flags, a.user_id), |p, v| {
                    let list = ListSlice {
                        creator: "android.content.pm.PackageInfo".into(),
                        items: v,
                    };
                    pm::write_get_installed_packages_reply(p, Some(&list))
                })
            }
            pm::GET_INSTALLED_APPLICATIONS => {
                let a = args(pm::GetInstalledApplications::read(r))?;
                thrown(self.installed_applications(a.flags, a.user_id), |p, v| {
                    let list = ListSlice {
                        creator: "android.content.pm.ApplicationInfo".into(),
                        items: v,
                    };
                    pm::write_get_installed_applications_reply(p, Some(&list))
                })
            }
            pm::CHECK_SIGNATURES => {
                let a = args(pm::CheckSignatures::read(r))?;
                let (p1, p2) = (a.pkg1.unwrap_or_default(), a.pkg2.unwrap_or_default());
                thrown(
                    self.check_signatures(&p1, &p2, a.user_id),
                    pm::write_check_signatures_reply,
                )
            }
            _ => Err(NotModelled("a method not modelled")),
        }
    }

    fn native(&self, code: u32, r: &mut Reader<'_>) -> Answered {
        let caller_user = user_id(self.calling_uid);
        match code {
            native::GET_NAMES_FOR_UIDS => {
                let a = args(native::GetNamesForUids::read(r))?;
                let names = self.names_for_uids(a.uids.as_deref())?.map(|names| {
                    names
                        .into_iter()
                        .map(|n| Some(n.unwrap_or_default()))
                        .collect()
                });
                Ok(reply(|p| native::write_get_names_for_uids_reply(p, &names)))
            }
            native::GET_PACKAGE_UID => {
                let a = args(native::GetPackageUid::read(r))?;
                let name = a.package_name.unwrap_or_default();
                thrown(
                    self.package_uid(&name, a.flags, a.user_id),
                    native::write_get_package_uid_reply,
                )
            }
            native::GET_INSTALLER_FOR_PACKAGE => {
                let a = args(native::GetInstallerForPackage::read(r))?;
                let name = a.package_name.unwrap_or_default();
                thrown(self.installer_for_package(&name, caller_user), |p, v| {
                    native::write_get_installer_for_package_reply(p, &Some(v))
                })
            }
            native::GET_VERSION_CODE_FOR_PACKAGE => {
                let a = args(native::GetVersionCodeForPackage::read(r))?;
                let name = a.package_name.unwrap_or_default();
                // Whatever getPackageInfo throws is 0.
                let version =
                    match self.package_info(&name, VERSION_CODE_HIGHEST, 0, caller_user)? {
                        Ok(Some(pi)) => {
                            (i64::from(pi.version_code_major) << 32)
                                | i64::from(pi.version_code as u32)
                        }
                        _ => 0,
                    };
                Ok(reply(|p| {
                    native::write_get_version_code_for_package_reply(p, version)
                }))
            }
            native::GET_LOCATION_FLAGS => {
                let a = args(native::GetLocationFlags::read(r))?;
                let name = a.package_name.unwrap_or_default();
                let Ok(Some(ai)) = self.application_info(&name, 0, caller_user)? else {
                    return Err(NotModelled("a RemoteException thrown by the service"));
                };
                let flags = info_flag(ai.flags & FLAG_SYSTEM != 0, LOCATION_SYSTEM)
                    | info_flag(ai.private_flags & PRIVATE_FLAG_VENDOR != 0, LOCATION_VENDOR)
                    | info_flag(
                        ai.private_flags & PRIVATE_FLAG_PRODUCT != 0,
                        LOCATION_PRODUCT,
                    );
                Ok(reply(|p| native::write_get_location_flags_reply(p, flags)))
            }
            native::GET_TARGET_SDK_VERSION_FOR_PACKAGE => {
                let a = args(native::GetTargetSdkVersionForPackage::read(r))?;
                let name = a.package_name.unwrap_or_default();
                let sdk = self.target_sdk_version(&name)?;
                if sdk == -1 {
                    return Err(NotModelled("a RemoteException thrown by the service"));
                }
                Ok(reply(|p| {
                    native::write_get_target_sdk_version_for_package_reply(p, sdk)
                }))
            }
            native::HAS_SHA256SIGNING_CERTIFICATE => {
                let a = args(native::HasSha256SigningCertificate::read(r))?;
                let name = a.package_name.unwrap_or_default();
                let has = self.has_sha256_certificate(&name, &a.certificate.unwrap_or_default())?;
                Ok(reply(|p| {
                    native::write_has_sha256signing_certificate_reply(p, has)
                }))
            }
            native::IS_PACKAGE_DEBUGGABLE => {
                let a = args(native::IsPackageDebuggable::read(r))?;
                let name = a.package_name.unwrap_or_default();
                let Ok(Some(ai)) = self.application_info(&name, 0, caller_user)? else {
                    return Err(NotModelled("a RemoteException thrown by the service"));
                };
                let debuggable = ai.flags & (1 << 1) != 0;
                Ok(reply(|p| {
                    native::write_is_package_debuggable_reply(p, debuggable)
                }))
            }
            native::HAS_SYSTEM_FEATURE => {
                let a = args(native::HasSystemFeature::read(r))?;
                let has = self.has_system_feature(a.feature_name.as_deref(), a.version);
                Ok(reply(|p| native::write_has_system_feature_reply(p, has)))
            }
            _ => Err(NotModelled("a method not modelled")),
        }
    }

    fn user(&self, user: i32) -> Option<&User> {
        self.state.users.get(&user)
    }

    /// `ComputerEngine.updateFlags`: the direct boot match flags of a
    /// caller that set none.
    pub fn update_flags(&self, mut flags: i64, user: i32) -> i64 {
        if flags & (MATCH_DIRECT_BOOT_UNAWARE | MATCH_DIRECT_BOOT_AWARE) == 0 {
            if self.user(user).is_some_and(|u| u.unlocking_or_unlocked) {
                flags |= MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE;
            } else {
                flags |= MATCH_DIRECT_BOOT_AWARE;
            }
        }
        flags
    }

    /// `updateFlagsForComponent`.
    pub fn update_flags_for_component(&self, flags: i64, user: i32) -> i64 {
        self.update_flags(flags, user)
    }

    /// `updateFlagsForPackage` (and `updateFlagsForApplication`).
    fn update_flags_for_package(&self, mut flags: i64, user: i32) -> Thrown<i64> {
        if flags & MATCH_ANY_USER != 0 {
            if let Err(e) = self.enforce_cross_user(
                user,
                true,
                true,
                "MATCH_ANY_USER flag requires INTERACT_ACROSS_USERS permission",
            )? {
                return Ok(Err(e));
            }
        } else if !self.aconfig(REMOVE_CROSS_USER_PERMISSION_HACK)
            && flags & MATCH_UNINSTALLED_PACKAGES != 0
            && user_id(self.calling_uid) == info::USER_SYSTEM
            && self.has_profile(info::USER_SYSTEM)
        {
            flags |= MATCH_ANY_USER;
        }
        Ok(Ok(self.update_flags(flags, user)))
    }

    fn aconfig(&self, name: &str) -> bool {
        self.state
            .system
            .flags
            .iter()
            .any(|(n, on)| n == name && *on)
    }

    /// `UserManagerService.hasProfile`.
    fn has_profile(&self, user: i32) -> bool {
        self.state
            .users
            .values()
            .any(|u| u.id != user && u.profile_group_id == user && u.flags & USER_FLAG_PROFILE != 0)
    }

    /// Whether `uid` holds `permission` in its user: granted to one of its
    /// packages (`checkUidPermission`).
    pub(crate) fn uid_has_permission(
        &self,
        uid: i32,
        permission: &str,
    ) -> Result<bool, NotModelled> {
        let user = user_id(uid);
        let granted = |ps: &PackageState| {
            ps.users
                .get(&user)
                .is_some_and(|u| u.granted_permissions.iter().any(|p| p == permission))
        };
        match setting(self.state, app_id(uid)) {
            Some(Setting::Package(ps)) => Ok(granted(ps)),
            Some(Setting::Shared(su)) => Ok(su
                .packages
                .iter()
                .filter_map(|n| self.state.packages.get(n))
                .any(granted)),
            None => Err(NotModelled("the permissions of a uid without packages")),
        }
    }

    /// `enforceCrossUserPermission` without its shell check; with
    /// `same_user`, the permission is needed for the caller's own user
    /// too. Where the recents may skip the check
    /// (`isRecentsAccessingChildProfiles`, the window manager's state), a
    /// denial is not modelled.
    pub(crate) fn enforce_cross_user(
        &self,
        user: i32,
        same_user: bool,
        recents: bool,
        message: &str,
    ) -> Thrown<()> {
        if user < 0 {
            return Ok(Err(Exception::illegal_argument(format!(
                "Invalid userId {user}"
            ))));
        }
        let uid = self.calling_uid;
        if (!same_user && user == user_id(uid)) || matches!(app_id(uid), ROOT_UID | SYSTEM_UID) {
            return Ok(Ok(()));
        }
        if self.uid_has_permission(uid, INTERACT_ACROSS_USERS_FULL)?
            || self.uid_has_permission(uid, INTERACT_ACROSS_USERS)?
        {
            return Ok(Ok(()));
        }
        if apps_filter::is_isolated(uid) {
            return Err(NotModelled("an isolated caller's owner"));
        }
        if recents {
            return Err(NotModelled("a cross-user call recents may make"));
        }
        Ok(Err(Exception::new(
            EX_SECURITY,
            format!(
                "{message}: UID {uid} requires {INTERACT_ACROSS_USERS_FULL} or \
                 {INTERACT_ACROSS_USERS} to access user {user}."
            ),
        )))
    }

    /// `shouldFilterApplication(ps, callingUid, userId)`.
    fn filtered(
        &self,
        ps: Option<&PackageState>,
        uid: i32,
        user: i32,
    ) -> Result<bool, NotModelled> {
        should_filter_application(self.state, self.filter, ps, uid, user, false, true)
    }

    /// `shouldFilterApplicationIncludingUninstalled`.
    pub(crate) fn filtered_including_uninstalled(
        &self,
        ps: Option<&PackageState>,
        user: i32,
    ) -> Result<bool, NotModelled> {
        should_filter_application(
            self.state,
            self.filter,
            ps,
            self.calling_uid,
            user,
            true,
            true,
        )
    }

    /// `shouldFilterApplicationIncludingUninstalledNotArchived`.
    fn filtered_including_uninstalled_not_archived(
        &self,
        ps: Option<&PackageState>,
        uid: i32,
        user: i32,
    ) -> Result<bool, NotModelled> {
        should_filter_application(self.state, self.filter, ps, uid, user, true, false)
    }

    /// `shouldFilterApplication` (and `...IncludingUninstalled`) of a
    /// shared user: filtered when every package of it is.
    fn shared_filtered(
        &self,
        packages: &[String],
        user: i32,
        including_uninstalled: bool,
    ) -> Result<bool, NotModelled> {
        let states: Vec<&PackageState> = packages
            .iter()
            .filter_map(|n| self.state.packages.get(n))
            .collect();
        let mut filter = true;
        for ps in states.iter().rev() {
            if !filter {
                break;
            }
            filter &= self.filtered(Some(ps), self.calling_uid, user)?;
        }
        if filter || !including_uninstalled {
            return Ok(filter);
        }
        if is_system_or_root_or_shell(self.calling_uid) {
            return Ok(false);
        }
        Ok(!states
            .iter()
            .any(|ps| user_state(ps, user).installed || ps.is.hidden_until_installed))
    }

    fn target<'b>(
        &'b self,
        ps: &'b PackageState,
        pkg: &'b AndroidPackage,
        state: &'b PackageUserState,
        user: i32,
    ) -> Target<'b> {
        Target {
            sys: &self.state.system,
            pkg,
            ps,
            state,
            user,
        }
    }

    /// A package of `mPackages`: one with its parsed package.
    fn package_of(&self, name: &str) -> Option<(&PackageState, &AndroidPackage)> {
        let ps = self.state.packages.get(name)?;
        Some((ps, ps.pkg.as_deref()?))
    }

    /// `resolveInternalPackageName`: a static shared library's package
    /// for its library name, as the caller may see it.
    fn resolve_internal_package_name(&self, name: &str, version_code: i64) -> String {
        let libs: Vec<(&PackageState, &AndroidPackage)> = self
            .state
            .packages
            .values()
            .filter_map(|ps| Some((ps, ps.pkg.as_deref()?)))
            .filter(|(_, p)| p.static_shared_library_name.as_deref() == Some(name))
            .collect();
        if libs.is_empty() {
            return name.to_string();
        }
        let visible: Option<Vec<i64>> =
            (!is_system_or_root_or_shell(self.calling_uid)).then(|| {
                self.package_names_for_app_id(app_id(self.calling_uid))
                    .iter()
                    .filter_map(|n| self.state.packages.get(n.as_str()))
                    .filter_map(|ps| {
                        ps.uses_static_libraries
                            .iter()
                            .find(|(lib, _)| lib == name)
                            .map(|(_, v)| *v)
                    })
                    .collect()
            });
        if visible.as_ref().is_some_and(Vec::is_empty) {
            return name.to_string();
        }
        let mut highest: Option<(&PackageState, i64)> = None;
        for (ps, p) in libs {
            if visible
                .as_ref()
                .is_some_and(|v| !v.contains(&p.static_shared_lib_version))
            {
                continue;
            }
            let declaring = ps.version_code;
            if version_code != VERSION_CODE_HIGHEST {
                if declaring == version_code {
                    return ps.name.clone();
                }
            } else if highest.is_none_or(|(_, v)| declaring > v) {
                highest = Some((ps, declaring));
            }
        }
        highest.map_or_else(|| name.to_string(), |(ps, _)| ps.name.clone())
    }

    /// The packages of an app id, as `getPackagesForUidInternal` lists
    /// them before filtering.
    fn package_names_for_app_id(&self, app_id: i32) -> Vec<String> {
        match setting(self.state, app_id) {
            Some(Setting::Package(ps)) => vec![ps.name.clone()],
            Some(Setting::Shared(su)) => su.packages.clone(),
            None => Vec::new(),
        }
    }

    /// `resolveExternalPackageName`.
    fn external_name(pkg: &AndroidPackage) -> String {
        if pkg.static_shared_library_name.is_some() {
            pkg.manifest_package_name
                .clone()
                .unwrap_or_else(|| pkg.package_name.clone())
        } else {
            pkg.package_name.clone()
        }
    }

    /// `filterSharedLibPackage`: a static shared or SDK library the caller
    /// does not use.
    fn filter_shared_lib(
        &self,
        ps: &PackageState,
        user: i32,
        flags: i64,
    ) -> Result<bool, NotModelled> {
        let uid = self.calling_uid;
        if flags & MATCH_STATIC_SHARED_AND_SDK_LIBRARIES != 0
            && (is_system_or_root_or_shell(app_id(uid))
                || self.uid_has_permission(uid, INSTALL_PACKAGES)?)
        {
            return Ok(false);
        }
        let Some(pkg) = ps.pkg.as_deref() else {
            return Ok(false);
        };
        let (name, version, uses): (&str, i64, UsedVersion) =
            if let Some(name) = &pkg.static_shared_library_name {
                (name, pkg.static_shared_lib_version, |ps, name| {
                    ps.uses_static_libraries
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, v)| *v)
                })
            } else if pkg.is(booleans::SDK_LIBRARY) {
                let Some(name) = &pkg.sdk_library_name else {
                    return Ok(false);
                };
                (name, i64::from(pkg.sdk_lib_version_major), |ps, name| {
                    ps.uses_sdk_libraries
                        .iter()
                        .find(|l| l.name == name)
                        .map(|l| l.version_major)
                })
            } else {
                return Ok(false);
            };
        let Some(names) = self.packages_for_uid(info::uid(user, app_id(uid)))? else {
            return Ok(true);
        };
        for n in names.iter().flatten() {
            if *n == ps.name {
                return Ok(false);
            }
            if let Some(other) = self.state.packages.get(n.as_str())
                && uses(other, name) == Some(version)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `getPackageInfoInternal`.
    pub fn package_info(
        &self,
        name: &str,
        version_code: i64,
        flags: i64,
        user: i32,
    ) -> Thrown<Option<PackageInfo>> {
        if self.user(user).is_none() {
            return Ok(Ok(None));
        }
        let flags = match self.update_flags_for_package(flags, user)? {
            Ok(f) => f,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = self.enforce_cross_user(user, false, false, "get package info")? {
            return Ok(Err(e));
        }
        let name = self.resolve_internal_package_name(name, version_code);
        let uid = self.calling_uid;
        let factory_only = flags & MATCH_FACTORY_ONLY != 0;
        let apex = flags & MATCH_APEX != 0;
        if factory_only && let Some(ps) = self.state.disabled_system_packages.get(&name) {
            if !apex && ps.pkg.as_deref().is_some_and(|p| p.is2(APEX)) {
                return Ok(Ok(None));
            }
            if self.filter_shared_lib(ps, user, flags)? || self.filtered(Some(ps), uid, user)? {
                return Ok(Ok(None));
            }
            return Ok(Ok(self.generate_package_info(ps, flags, user)?));
        }
        if let Some((ps, p)) = self.package_of(&name) {
            if factory_only && !ps.is.system {
                return Ok(Ok(None));
            }
            if !apex && p.is2(APEX) {
                return Ok(Ok(None));
            }
            if self.filter_shared_lib(ps, user, flags)? || self.filtered(Some(ps), uid, user)? {
                return Ok(Ok(None));
            }
            return Ok(Ok(self.generate_package_info(ps, flags, user)?));
        }
        if !factory_only
            && flags & (MATCH_KNOWN_PACKAGES | MATCH_ARCHIVED_PACKAGES) != 0
            && let Some(ps) = self.state.packages.get(&name)
        {
            if self.filter_shared_lib(ps, user, flags)? || self.filtered(Some(ps), uid, user)? {
                return Ok(Ok(None));
            }
            return Ok(Ok(self.generate_package_info(ps, flags, user)?));
        }
        Ok(Ok(None))
    }

    /// `ComputerEngine.generatePackageInfo`.
    fn generate_package_info(
        &self,
        ps: &PackageState,
        mut flags: i64,
        user: i32,
    ) -> Result<Option<PackageInfo>, NotModelled> {
        if self.user(user).is_none() || self.filtered(Some(ps), self.calling_uid, user)? {
            return Ok(None);
        }
        if flags & MATCH_UNINSTALLED_PACKAGES != 0 && ps.is.system {
            flags |= MATCH_ANY_USER;
        }
        let state = user_state(ps, user);
        let Some(p) = ps.pkg.as_deref() else {
            if flags & (MATCH_UNINSTALLED_PACKAGES | MATCH_ARCHIVED_PACKAGES) != 0
                && info::is_available(&state, flags)
            {
                return Err(NotModelled("the info of a package without code"));
            }
            return Ok(None);
        };
        let x = Extras {
            first_install_time: state.first_install_time,
            last_update_time: ps.last_update_time,
            gids: &state.gids,
            installed_permissions: &ps.installed_permissions,
            granted_permissions: if p.requested_permissions.is_empty() {
                &[]
            } else {
                &state.granted_permissions
            },
            external_name: &Self::external_name(p),
        };
        let Some(info) = generate_package_info(&self.target(ps, p, &state, user), &x, flags) else {
            return Ok(None);
        };
        let mut info = info;
        if self.aconfig(PROVIDE_INFO_OF_APK_IN_APEX)
            && let Some(module) = &ps.apex_module_name
        {
            // ApexManager.getActivePackageNameForApexModuleName: the
            // active APEX of the module.
            info.apex_package_name = self
                .state
                .packages
                .values()
                .find(|p| p.is.apex && p.apex_module_name.as_ref() == Some(module))
                .map(|p| p.name.clone());
        }
        Ok(Some(info))
    }

    /// `getApplicationInfoInternal`.
    pub fn application_info(
        &self,
        name: &str,
        flags: i64,
        user: i32,
    ) -> Thrown<Option<ApplicationInfo>> {
        if self.user(user).is_none() {
            return Ok(Ok(None));
        }
        let flags = match self.update_flags_for_package(flags, user)? {
            Ok(f) => f,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = self.enforce_cross_user(user, false, true, "get application info")? {
            return Ok(Err(e));
        }
        let name = self.resolve_internal_package_name(name, VERSION_CODE_HIGHEST);
        if let Some((ps, p)) = self.package_of(&name) {
            if flags & MATCH_APEX == 0 && p.is2(APEX) {
                return Ok(Ok(None));
            }
            if self.filter_shared_lib(ps, user, flags)?
                || self.filtered(Some(ps), self.calling_uid, user)?
            {
                return Ok(Ok(None));
            }
            let state = user_state(ps, user);
            let ai = generate_application_info(&self.target(ps, p, &state, user), flags).map(
                |mut ai| {
                    ai.item.package_name = Some(Self::external_name(p));
                    ai
                },
            );
            return Ok(Ok(ai));
        }
        if name == "android" || name == "system" {
            return Err(NotModelled("the android application without its package"));
        }
        if flags & (MATCH_KNOWN_PACKAGES | MATCH_ARCHIVED_PACKAGES) != 0
            && self.state.packages.contains_key(&name)
        {
            return Err(NotModelled("the info of a package without code"));
        }
        Ok(Ok(None))
    }

    /// `getInstallSource`.
    fn install_source(&self, name: &str, user: i32) -> Result<Option<Source<'_>>, NotModelled> {
        if self.package_of(name).is_some_and(|(_, p)| p.is2(APEX)) {
            // InstallSource.EMPTY.
            return Ok(Some(None));
        }
        let ps = self.state.packages.get(name);
        if ps.is_none()
            || self.filtered_including_uninstalled_not_archived(ps, self.calling_uid, user)?
        {
            return Ok(None);
        }
        Ok(Some(ps))
    }

    /// `getInstallSourceInfo`.
    fn install_source_info(
        &self,
        name: &str,
        user: i32,
    ) -> Thrown<Option<info::InstallSourceInfo>> {
        if let Err(e) = self.enforce_cross_user(user, false, false, "getInstallSourceInfo")? {
            return Ok(Err(e));
        }
        let Some(source) = self.install_source(name, user)? else {
            return Ok(Ok(None));
        };
        let Some(ps) = source else {
            return Ok(Ok(Some(info::InstallSourceInfo::default())));
        };
        let src = &ps.install_source;
        let visible = |n: &Option<String>| -> Result<Option<String>, NotModelled> {
            let Some(n) = n else { return Ok(None) };
            let other = self.state.packages.get(n.as_str());
            Ok(
                (other.is_some() && !self.filtered_including_uninstalled(other, user)?)
                    .then(|| n.clone()),
            )
        };
        let installer = visible(&src.installer)?;
        let update_owner = visible(&src.update_owner)?;
        if update_owner.is_some()
            && self.calling_uid != SYSTEM_UID
            && !apps_filter::is_caller_same_app(
                self.state,
                src.update_owner.as_deref(),
                self.calling_uid,
            )?
        {
            // Whether the user is managed is the device policy's state.
            return Err(NotModelled("an update owner shown to another app"));
        }
        let initiating = if src.initiating_package_uninstalled {
            let instant = apps_filter::instant_app_package_name(self.state, self.calling_uid)?;
            if instant.is_none()
                && apps_filter::is_caller_same_app(self.state, Some(name), self.calling_uid)?
            {
                src.initiating_package.clone()
            } else {
                None
            }
        } else if src.initiating_package == src.installer {
            installer.clone()
        } else {
            visible(&src.initiating_package)?
        };
        let mut originating = visible(&src.originating_package)?;
        if originating.is_some() && !self.uid_has_permission(self.calling_uid, INSTALL_PACKAGES)? {
            originating = None;
        }
        let signing = initiating
            .as_ref()
            .and(src.initiating_package_signatures.as_ref())
            .map(|s| info::SigningInfo {
                scheme_version: s.scheme_version,
                signatures: s.signatures.clone(),
                public_keys: s.public_keys.clone(),
                past_signing_certificates: s
                    .past_signatures
                    .as_ref()
                    .map(|p| p.iter().map(|(der, _)| der.clone()).collect()),
            });
        Ok(Ok(Some(info::InstallSourceInfo {
            initiating_package_name: initiating,
            initiating_package_signing_info: signing,
            originating_package_name: originating,
            installing_package_name: installer,
            update_owner_package_name: update_owner,
            package_source: src.package_source,
        })))
    }

    /// `getApplicationEnabledSetting`.
    fn application_enabled_setting(&self, name: &str, user: i32) -> Thrown<i32> {
        if self.user(user).is_none() {
            return Ok(Ok(COMPONENT_ENABLED_STATE_DISABLED));
        }
        if let Err(e) = self.enforce_cross_user(user, false, false, "get enabled")? {
            return Ok(Err(e));
        }
        let ps = self.state.packages.get(name);
        if self.filtered_including_uninstalled(ps, user)? {
            return Ok(Err(Exception::illegal_argument(format!(
                "Unknown package: {name}"
            ))));
        }
        let Some(ps) = ps else {
            return Ok(Err(Exception::illegal_argument(format!(
                "Unknown package: {name}"
            ))));
        };
        Ok(Ok(user_state(ps, user).enabled))
    }

    /// `getComponentEnabledSetting`.
    fn component_enabled_setting(&self, c: Option<&ComponentName>, user: i32) -> Thrown<i32> {
        if let Err(e) = self.enforce_cross_user(user, false, false, "getComponentEnabled")? {
            return Ok(Err(e));
        }
        let Some(c) = c else {
            return Ok(Ok(COMPONENT_ENABLED_STATE_DEFAULT));
        };
        if self.user(user).is_none() {
            return Ok(Ok(COMPONENT_ENABLED_STATE_DISABLED));
        }
        let unknown = || {
            Ok(Err(Exception::illegal_argument(format!(
                "Unknown component: ComponentInfo{{{}/{}}}",
                c.package, c.class
            ))))
        };
        let ps = self.state.packages.get(&c.package);
        if self.filtered_including_uninstalled(ps, user)? {
            return unknown();
        }
        let Some(ps) = ps else {
            return unknown();
        };
        let state = user_state(ps, user);
        Ok(Ok(if state.enabled_components.contains(&c.class) {
            COMPONENT_ENABLED_STATE_ENABLED
        } else if state.disabled_components.contains(&c.class) {
            COMPONENT_ENABLED_STATE_DISABLED
        } else {
            COMPONENT_ENABLED_STATE_DEFAULT
        }))
    }

    /// `PackageManagerService.hasSystemFeature`.
    fn has_system_feature(&self, name: Option<&str>, version: i32) -> bool {
        self.state
            .system
            .features
            .iter()
            .any(|(n, v)| Some(n.as_str()) == name && *v >= version)
    }

    /// `getSystemAvailableFeatures`: SystemConfig's features in their
    /// `ArrayMap`'s order, then the GL ES version.
    fn system_available_features(&self) -> ListSlice<super::pkg::FeatureInfo> {
        let features = array_order(self.state.system.features.clone(), |(n, _)| n);
        let mut items: Vec<super::pkg::FeatureInfo> = features
            .into_iter()
            .map(|(name, version)| super::pkg::FeatureInfo {
                name: Some(name),
                version,
                ..Default::default()
            })
            .collect();
        items.push(super::pkg::FeatureInfo {
            req_gl_es_version: self.state.system.gl_es_version,
            ..Default::default()
        });
        ListSlice {
            creator: "android.content.pm.FeatureInfo".into(),
            items,
        }
    }

    /// `IPackageManagerBase.getPackagesForUid`, with its cross-user check.
    fn packages_for_uid_checked(&self, uid: i32) -> Thrown<Option<Vec<Option<String>>>> {
        let user = user_id(uid);
        if user < 0 {
            return Ok(Err(Exception::illegal_argument(format!(
                "Invalid userId {user}"
            ))));
        }
        let caller = self.calling_uid;
        if user != user_id(caller)
            && !matches!(app_id(caller), ROOT_UID | SYSTEM_UID)
            && !self.uid_has_permission(caller, INTERACT_ACROSS_USERS_FULL)?
            && !self.uid_has_permission(caller, INTERACT_ACROSS_USERS)?
        {
            return Err(NotModelled("a cross-profile packages-for-uid call"));
        }
        Ok(Ok(self.packages_for_uid(uid)?))
    }

    /// `getPackagesForUidInternal`.
    pub fn packages_for_uid(&self, uid: i32) -> Result<Option<Vec<Option<String>>>, NotModelled> {
        if apps_filter::is_sdk_sandbox(uid) || apps_filter::is_isolated(uid) {
            return Err(NotModelled("a sandbox or isolated uid's packages"));
        }
        let caller_is_instant =
            apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some();
        let user = user_id(uid);
        Ok(match setting(self.state, app_id(uid)) {
            Some(Setting::Shared(su)) => {
                if caller_is_instant {
                    return Ok(None);
                }
                let mut names = Vec::new();
                for n in &su.packages {
                    if let Some(ps) = self.state.packages.get(n)
                        && user_state(ps, user).installed
                        && !self.filtered(Some(ps), self.calling_uid, user)?
                    {
                        names.push(Some(ps.name.clone()));
                    }
                }
                Some(names)
            }
            Some(Setting::Package(ps)) => (user_state(ps, user).installed
                && !self.filtered(Some(ps), self.calling_uid, user)?)
            .then(|| vec![Some(ps.name.clone())]),
            None => None,
        })
    }

    /// `getNameForUid`.
    fn name_for_uid(&self, uid: i32) -> Result<Option<String>, NotModelled> {
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(None);
        }
        if apps_filter::is_sdk_sandbox(uid) || apps_filter::is_isolated(uid) {
            return Err(NotModelled("a sandbox or isolated uid's name"));
        }
        let user = user_id(self.calling_uid);
        Ok(match setting(self.state, app_id(uid)) {
            Some(Setting::Shared(su)) => (!self.shared_filtered(&su.packages, user, true)?)
                .then(|| format!("{}:{}", su.name, su.app_id)),
            Some(Setting::Package(ps)) => {
                (!self.filtered_including_uninstalled(Some(ps), user)?).then(|| ps.name.clone())
            }
            None => None,
        })
    }

    /// `getNamesForUids`.
    fn names_for_uids(
        &self,
        uids: Option<&[i32]>,
    ) -> Result<Option<Vec<Option<String>>>, NotModelled> {
        let Some(uids) = uids.filter(|u| !u.is_empty()) else {
            return Ok(None);
        };
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(None);
        }
        let user = user_id(self.calling_uid);
        let mut names = vec![None; uids.len()];
        for (i, &uid) in uids.iter().enumerate().rev() {
            if apps_filter::is_sdk_sandbox(uid) || apps_filter::is_isolated(uid) {
                return Err(NotModelled("a sandbox or isolated uid's name"));
            }
            names[i] = match setting(self.state, app_id(uid)) {
                Some(Setting::Shared(su)) => (!self.shared_filtered(&su.packages, user, true)?)
                    .then(|| format!("shared:{}", su.name)),
                Some(Setting::Package(ps)) => {
                    (!self.filtered_including_uninstalled(Some(ps), user)?).then(|| ps.name.clone())
                }
                None => None,
            };
        }
        Ok(Some(names))
    }

    /// `getPackageUid`.
    fn package_uid(&self, name: &str, flags: i64, user: i32) -> Thrown<i32> {
        if self.user(user).is_none() {
            return Ok(Ok(-1));
        }
        let flags = match self.update_flags_for_package(flags, user)? {
            Ok(f) => f,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = self.enforce_cross_user(user, false, false, "getPackageUid")? {
            return Ok(Err(e));
        }
        let uid = self.calling_uid;
        if let Some((ps, p)) = self.package_of(name)
            && (flags & MATCH_SYSTEM_ONLY == 0 || ps.is.system)
        {
            let resolved =
                self.resolve_internal_package_name(&p.package_name, VERSION_CODE_HIGHEST);
            if let Some(rps) = self.state.packages.get(&resolved)
                && user_state(rps, user).installed
                && !self.filtered(Some(rps), uid, user)?
            {
                return Ok(Ok(info::uid(user, p.uid)));
            }
        }
        if flags & (MATCH_KNOWN_PACKAGES | MATCH_ARCHIVED_PACKAGES) != 0
            && let Some(ps) = self.state.packages.get(name)
            && (flags & MATCH_SYSTEM_ONLY == 0 || ps.is.system)
            && !self.filtered(Some(ps), uid, user)?
        {
            return Ok(Ok(info::uid(user, ps.app_id)));
        }
        Ok(Ok(-1))
    }

    /// `getActivityInfoInternal`, filtered for `filter_uid`.
    pub fn activity_info(
        &self,
        c: &ComponentName,
        flags: i64,
        filter_uid: i32,
        user: i32,
    ) -> Thrown<Option<info::ActivityInfo>> {
        if self.user(user).is_none() {
            return Ok(Ok(None));
        }
        let flags = self.update_flags_for_component(flags, user) | MATCH_QUARANTINED_COMPONENTS;
        if let Err(e) = self.enforce_cross_user(user, false, true, "get activity info")? {
            return Ok(Err(e));
        }
        if let Some((ps, p)) = self.package_of(&c.package)
            && let Some(a) = p
                .activities
                .iter()
                .find(|a| a.main.component.name == c.class)
        {
            if !info::is_enabled_and_matches(ps, &a.main, flags, user) {
                return Ok(Ok(None));
            }
            if self.filtered(Some(ps), filter_uid, user)? {
                return Ok(Ok(None));
            }
            let state = user_state(ps, user);
            let t = self.target(ps, p, &state, user);
            return Ok(Ok(info::generate_activity_info(&t, a, flags, None)));
        }
        if c.package == "android" && c.class == "com.android.internal.app.ResolverActivity" {
            return Err(NotModelled("the resolver activity without its package"));
        }
        Ok(Ok(None))
    }

    /// `getReceiverInfo`.
    pub fn receiver_info(
        &self,
        c: &ComponentName,
        flags: i64,
        user: i32,
    ) -> Thrown<Option<info::ActivityInfo>> {
        if self.user(user).is_none() {
            return Ok(Ok(None));
        }
        let flags = self.update_flags_for_component(flags, user);
        if let Err(e) = self.enforce_cross_user(user, false, false, "get receiver info")? {
            return Ok(Err(e));
        }
        let Some((ps, p)) = self.package_of(&c.package) else {
            return Ok(Ok(None));
        };
        let Some(a) = p
            .receivers
            .iter()
            .find(|a| a.main.component.name == c.class)
        else {
            return Ok(Ok(None));
        };
        if !info::is_enabled_and_matches(ps, &a.main, flags, user)
            || self.filtered(Some(ps), self.calling_uid, user)?
        {
            return Ok(Ok(None));
        }
        let state = user_state(ps, user);
        let t = self.target(ps, p, &state, user);
        Ok(Ok(info::generate_activity_info(&t, a, flags, None)))
    }

    /// `getServiceInfo`; its cross-profile check
    /// (`enforceCrossUserOrProfilePermission`'s `INTERACT_ACROSS_PROFILES`)
    /// is not modelled: a denial is reported as such.
    pub fn service_info(
        &self,
        c: &ComponentName,
        flags: i64,
        user: i32,
    ) -> Thrown<Option<info::ServiceInfo>> {
        if self.user(user).is_none() {
            return Ok(Ok(None));
        }
        let flags = self.update_flags_for_component(flags, user);
        if let Err(e) = self.enforce_cross_user(user, false, true, "get service info")? {
            return Ok(Err(e));
        }
        let Some((ps, p)) = self.package_of(&c.package) else {
            return Ok(Ok(None));
        };
        let Some(s) = p.services.iter().find(|s| s.main.component.name == c.class) else {
            return Ok(Ok(None));
        };
        if !info::is_enabled_and_matches(ps, &s.main, flags, user)
            || self.filtered(Some(ps), self.calling_uid, user)?
        {
            return Ok(Ok(None));
        }
        let state = user_state(ps, user);
        let t = self.target(ps, p, &state, user);
        Ok(Ok(info::generate_service_info(&t, s, flags, None)))
    }

    /// `getProviderInfo`.
    pub fn provider_info(
        &self,
        c: &ComponentName,
        flags: i64,
        user: i32,
    ) -> Thrown<Option<info::ProviderInfo>> {
        if self.user(user).is_none() {
            return Ok(Ok(None));
        }
        let flags = self.update_flags_for_component(flags, user);
        if let Err(e) = self.enforce_cross_user(user, false, false, "get provider info")? {
            return Ok(Err(e));
        }
        let Some((ps, p)) = self.package_of(&c.package) else {
            return Ok(Ok(None));
        };
        let Some(pr) = p
            .providers
            .iter()
            .find(|pr| pr.main.component.name == c.class)
        else {
            return Ok(Ok(None));
        };
        if !info::is_enabled_and_matches(ps, &pr.main, flags, user)
            || self.filtered(Some(ps), self.calling_uid, user)?
        {
            return Ok(Ok(None));
        }
        let state = user_state(ps, user);
        let t = self.target(ps, p, &state, user);
        let Some(app) = generate_application_info(&t, flags) else {
            return Ok(Ok(None));
        };
        Ok(Ok(info::generate_provider_info(
            &t,
            pr,
            flags,
            Some(Arc::new(app)),
        )))
    }

    /// The package states in `Settings.mPackages`' order.
    fn packages_in_order(&self) -> Vec<&PackageState> {
        array_order(self.state.packages.values().collect(), |ps| &ps.name)
    }

    /// `getInstalledPackages`.
    fn installed_packages(&self, flags: i64, user: i32) -> Thrown<Vec<PackageInfo>> {
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some()
            || self.user(user).is_none()
        {
            return Ok(Ok(Vec::new()));
        }
        let flags = match self.update_flags_for_package(flags, user)? {
            Ok(f) => f,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = self.enforce_cross_user(user, false, false, "get installed packages")? {
            return Ok(Err(e));
        }
        let uninstalled = flags & MATCH_KNOWN_PACKAGES != 0;
        let apex = flags & MATCH_APEX != 0;
        let factory = flags & MATCH_FACTORY_ONLY != 0;
        let archived_only = !uninstalled && flags & MATCH_ARCHIVED_PACKAGES != 0;
        let is_apex = |ps: &PackageState| ps.pkg.as_deref().is_some_and(|p| p.is2(APEX));
        let mut list = Vec::new();
        for current in self.packages_in_order() {
            // Without MATCH_KNOWN_PACKAGES, only packages with code
            // (`mPackages`), and an APEX by the current package.
            if !(uninstalled || archived_only) && current.pkg.is_none() {
                continue;
            }
            if factory && !current.is.system {
                continue;
            }
            let ps = match factory {
                true => self
                    .state
                    .disabled_system_packages
                    .get(&current.name)
                    .unwrap_or(current),
                false => current,
            };
            let apex_of = if uninstalled || archived_only {
                ps
            } else {
                current
            };
            if !apex && is_apex(apex_of) {
                continue;
            }
            let state = user_state(ps, user);
            if archived_only && !state.installed && state.archive_state.is_none() {
                continue;
            }
            if self.filter_shared_lib(ps, user, flags)?
                || self.filtered(Some(ps), self.calling_uid, user)?
            {
                continue;
            }
            if let Some(pi) = self.generate_package_info(ps, flags, user)? {
                list.push(pi);
            }
        }
        Ok(Ok(list))
    }

    /// `getInstalledApplications`.
    fn installed_applications(&self, flags: i64, user: i32) -> Thrown<Vec<ApplicationInfo>> {
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some()
            || self.user(user).is_none()
        {
            return Ok(Ok(Vec::new()));
        }
        let flags = match self.update_flags_for_package(flags, user)? {
            Ok(f) => f,
            Err(e) => return Ok(Err(e)),
        };
        let uninstalled = flags & MATCH_KNOWN_PACKAGES != 0;
        let apex = flags & MATCH_APEX != 0;
        let archived_only = !uninstalled && flags & MATCH_ARCHIVED_PACKAGES != 0;
        if let Err(e) =
            self.enforce_cross_user(user, false, false, "get installed application info")?
        {
            return Ok(Err(e));
        }
        let mut list = Vec::new();
        for ps in self.packages_in_order() {
            let mut effective = flags;
            if uninstalled || archived_only {
                if ps.is.system {
                    effective |= MATCH_ANY_USER;
                }
                let Some(_) = ps.pkg else {
                    return Err(NotModelled("the info of a package without code"));
                };
                let state = user_state(ps, user);
                if archived_only && !state.installed && state.archive_state.is_none() {
                    continue;
                }
            }
            let Some(p) = ps.pkg.as_deref() else { continue };
            if !apex && p.is2(APEX) {
                continue;
            }
            if self.filter_shared_lib(ps, user, flags)?
                || self.filtered(Some(ps), self.calling_uid, user)?
            {
                continue;
            }
            let state = user_state(ps, user);
            if let Some(mut ai) =
                generate_application_info(&self.target(ps, p, &state, user), effective)
            {
                ai.item.package_name = Some(Self::external_name(p));
                list.push(ai);
            }
        }
        Ok(Ok(list))
    }

    /// `checkSignatures`.
    fn check_signatures(&self, pkg1: &str, pkg2: &str, user: i32) -> Thrown<i32> {
        if let Err(e) = self.enforce_cross_user(user, false, false, "checkSignatures")? {
            return Ok(Err(e));
        }
        let (Some((ps1, p1)), Some((ps2, p2))) = (self.package_of(pkg1), self.package_of(pkg2))
        else {
            return Ok(Ok(SIGNATURE_UNKNOWN_PACKAGE));
        };
        if self.filtered_including_uninstalled(Some(ps1), user)?
            || self.filtered_including_uninstalled(Some(ps2), user)?
        {
            return Ok(Ok(SIGNATURE_UNKNOWN_PACKAGE));
        }
        Ok(Ok(check_signatures(p1, p2)))
    }

    /// `getInstallerForPackage` of package_native.
    fn installer_for_package(&self, name: &str, user: i32) -> Thrown<String> {
        let Some(source) = self.install_source(name, user)? else {
            return Ok(Err(Exception::illegal_argument(format!(
                "Unknown package: {name}"
            ))));
        };
        if let Some(installer) = source.and_then(|ps| ps.install_source.installer.as_deref()) {
            let other = self.state.packages.get(installer);
            if other.is_some()
                && !self.filtered_including_uninstalled_not_archived(
                    other,
                    self.calling_uid,
                    user_id(self.calling_uid),
                )?
                && !installer.is_empty()
            {
                return Ok(Ok(installer.to_string()));
            }
        }
        let preload = matches!(
            self.application_info(name, 0, user)?,
            Ok(Some(ai)) if ai.flags & FLAG_SYSTEM != 0
        );
        Ok(Ok(if preload { "preload" } else { "" }.to_string()))
    }

    /// `getTargetSdkVersion`.
    fn target_sdk_version(&self, name: &str) -> Result<i32, NotModelled> {
        let ps = self.state.packages.get(name);
        let Some(p) = ps.and_then(|ps| ps.pkg.as_deref()) else {
            return Ok(-1);
        };
        if self.filtered_including_uninstalled(ps, user_id(self.calling_uid))? {
            return Ok(-1);
        }
        Ok(p.target_sdk_version)
    }

    /// `hasSigningCertificate` with a SHA-256 digest
    /// (`SigningDetails.hasSha256Certificate`).
    fn has_sha256_certificate(&self, name: &str, digest: &[u8]) -> Result<bool, NotModelled> {
        let Some((ps, p)) = self.package_of(name) else {
            return Ok(false);
        };
        if self.filtered_including_uninstalled(Some(ps), user_id(self.calling_uid))? {
            return Ok(false);
        }
        let Some(s) = &p.signing_details else {
            return Ok(false);
        };
        let sha = |der: &[u8]| -> Vec<u8> {
            use sha2::Digest;
            sha2::Sha256::digest(der).to_vec()
        };
        if let Some(past) = &s.past_signing_certificates
            && past.len() > 1
            && past[..past.len() - 1].iter().any(|c| sha(c) == digest)
        {
            return Ok(true);
        }
        Ok(match s.signatures.as_deref() {
            Some([one]) => sha(one) == digest,
            _ => false,
        })
    }
}

fn info_flag(set: bool, flag: i32) -> i32 {
    if set { flag } else { 0 }
}

/// `checkSignaturesInternal` of two packages' signing details.
fn check_signatures(p1: &AndroidPackage, p2: &AndroidPackage) -> i32 {
    let sigs = |p: &AndroidPackage| {
        p.signing_details
            .as_ref()
            .and_then(|s| s.signatures.clone())
    };
    let (s1, s2) = (sigs(p1), sigs(p2));
    let result = compare_signature_arrays(s1.as_deref(), s2.as_deref());
    if result == SIGNATURE_MATCH {
        return result;
    }
    let oldest = |p: &AndroidPackage, current: Option<Vec<Vec<u8>>>| match p
        .signing_details
        .as_ref()
        .and_then(|s| s.past_signing_certificates.as_ref())
    {
        Some(past) if !past.is_empty() => Some(vec![past[0].clone()]),
        _ => current,
    };
    let rotated = |p: &AndroidPackage| {
        p.signing_details
            .as_ref()
            .and_then(|s| s.past_signing_certificates.as_ref())
            .is_some_and(|past| !past.is_empty())
    };
    if rotated(p1) || rotated(p2) {
        let (o1, o2) = (oldest(p1, s1), oldest(p2, s2));
        return compare_signature_arrays(o1.as_deref(), o2.as_deref());
    }
    result
}

/// `PackageManagerServiceUtils.compareSignatureArrays`.
fn compare_signature_arrays(s1: Option<&[Vec<u8>]>, s2: Option<&[Vec<u8>]>) -> i32 {
    match (s1, s2) {
        (None, None) => SIGNATURE_NEITHER_SIGNED,
        (None, Some(_)) => SIGNATURE_FIRST_NOT_SIGNED,
        (Some(_), None) => SIGNATURE_SECOND_NOT_SIGNED,
        (Some(a), Some(b)) if a.len() != b.len() => SIGNATURE_NO_MATCH,
        (Some(a), Some(b)) => {
            let same = a.iter().all(|s| b.contains(s)) && b.iter().all(|s| a.contains(s));
            if same {
                SIGNATURE_MATCH
            } else {
                SIGNATURE_NO_MATCH
            }
        }
    }
}

#[cfg(test)]
mod tests;
