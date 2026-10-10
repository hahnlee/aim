//! Concrete native operations behind the original UserManager redirects.
use super::{effects, lifecycle, owner::{Store, runtime_metadata}, preferred, scan::SigningScan,
    scan_snapshot::query_state::Capture, users::{Creation, CreationPolicy}, intent_filter::IntentFilter};
use aim_binder_host::parcel::{Exception, Parcel, EX_ILLEGAL_STATE};
use std::{collections::{BTreeMap, BTreeSet}, sync::{Arc, Mutex}};

/// Root binds these operations to its existing generation gate and concrete
/// native owners. All Settings/app-data choices and write bodies live below.
#[derive(Clone)]
pub struct UserRecord { pub user: super::model::User, pub scan: super::scan::User }
impl UserRecord {
    fn read(bytes: &[u8], expected: i32) -> Result<Self, Exception> {
        let mut reader = aim_binder_host::parcel::Reader::new(bytes, &[]);
        let malformed = |status| Exception::new(EX_ILLEGAL_STATE, format!("original user record: {status}"));
        if reader.read_i32().map_err(malformed)? != 1 { return Err(failure("original user record version")); }
        let id = reader.read_i32().map_err(malformed)?;
        if id != expected { return Err(failure("original user record identity differs")); }
        let flags = reader.read_i32().map_err(malformed)?;
        let profile_group_id = reader.read_i32().map_err(malformed)?;
        let unlocking_or_unlocked = reader.read_bool().map_err(malformed)?;
        let pre_created = reader.read_bool().map_err(malformed)?;
        let adb_install_disallowed = reader.read_bool().map_err(malformed)?;
        if reader.remaining() != 0 { return Err(failure("original user record trailing bytes")); }
        Ok(Self {
            user: super::model::User { id, flags, profile_group_id, unlocking_or_unlocked,
                preferred_activities: None, restrictions: None, default_browser: None },
            scan: super::scan::User { id, pre_created, adb_install_disallowed },
        })
    }
}
pub struct Dependencies {
    pub capture: Box<dyn Fn() -> Result<Arc<Capture>, Exception> + Send + Sync>,
    pub publish_user: Box<dyn Fn(&Arc<Capture>, SigningScan, i32, bool, Option<UserRecord>) -> Result<Arc<Capture>, Exception> + Send + Sync>,
    pub launcher: Box<dyn Fn(&str, i32, i64) -> Result<bool, String> + Send + Sync>,
    pub kernel_exclusion: Box<dyn Fn(&str, i32) -> Result<(), Exception> + Send + Sync>,
    pub kernel_remove_user: Box<dyn Fn(i32) -> Result<(), Exception> + Send + Sync>,
    pub default_preferred: Box<dyn Fn(i32) -> Result<aim_android_xml::Element, Exception> + Send + Sync>,
    pub write_package_list: Box<dyn Fn(&Arc<Capture>, Option<i32>) -> Result<(), Exception> + Send + Sync>,
    pub finish_permission_creation:Box<dyn Fn(i32)->Result<(),Exception>+Send+Sync>,
    pub read_permission_state: Box<dyn Fn(i32) -> Result<(), Exception> + Send + Sync>,
    pub clear_domain_user: Box<dyn Fn(i32) -> Result<(), Exception> + Send + Sync>,
    pub remove_unused_packages: Box<dyn Fn(&Arc<Capture>, i32) -> Result<(), Exception> + Send + Sync>,
    pub instant_user_removed: Box<dyn Fn(i32) -> Result<(), Exception> + Send + Sync>,
    pub commit_cross_profile_stage: Box<dyn Fn(preferred::registry::Stage) -> Result<(), Exception> + Send + Sync>,
    pub add_cross_profile: Box<dyn Fn(IntentFilter, Option<String>, i32, i32, i32) -> Result<(), Exception> + Send + Sync>,
}
pub struct Owner {
    disk: Arc<Mutex<Store>>, pending: Mutex<BTreeMap<i32, Creation>>,
    preferred: Arc<preferred::registry::Registry>, runtime: Arc<Mutex<runtime_metadata::State>>,
    effects: Arc<effects::Owner>, lifecycle: Arc<lifecycle::Owner>,
    dependencies: Dependencies, fix_first_install_time: bool,
    non_stopped: BTreeSet<String>,
}
impl Owner {
    pub fn new(disk: Arc<Mutex<Store>>, preferred: Arc<preferred::registry::Registry>,
        runtime: Arc<Mutex<runtime_metadata::State>>, effects: Arc<effects::Owner>,
        lifecycle: Arc<lifecycle::Owner>, dependencies: Dependencies,
        fix_first_install_time: bool, non_stopped: BTreeSet<String>) -> Self {
        Self { disk, pending: Mutex::new(BTreeMap::new()), preferred, runtime, effects,
            lifecycle, dependencies, fix_first_install_time, non_stopped }
    }
    pub fn create_user_state(&self, user: i32, installable: Option<Vec<Option<String>>>,
        disallowed: Option<Vec<Option<String>>>, current_time_millis: i64, stop_system_packages: bool, user_record: Vec<u8>) -> Result<Vec<u8>, Exception> {
        if user < 0 { return Err(Exception::illegal_argument("negative new user")); }
        let user_record = UserRecord::read(&user_record, user)?;
        if self.pending.lock().unwrap().contains_key(&user) { return Err(Exception::new(EX_ILLEGAL_STATE, "user creation already pending")); }
        let capture = (self.dependencies.capture)()?;
        let installable = installable.map(|names| names.into_iter().flatten().collect::<BTreeSet<_>>());
        let disallowed: Vec<_> = disallowed.unwrap_or_default().into_iter().flatten().collect();
        let policy = CreationPolicy { current_time_millis, fix_system_apps_first_install_time: self.fix_first_install_time,
            stop_system_packages_by_default: stop_system_packages, initial_non_stopped_system_packages: self.non_stopped.clone() };
        let creation = Creation::prepare(&capture, user, installable.as_ref(), &disallowed, &policy,
            |name, user, flags| (self.dependencies.launcher)(name, user, flags)).map_err(failure)?;
        let mut scan = capture.scan().owner().clone(); creation.apply_scan(&mut scan).map_err(failure)?;
        // Original Settings changes live user state before installd runs. The
        // original Java adapter owns the actual install lock during this call.
        self.disk.lock().unwrap().register_package_user(user as u32).map_err(|error| failure(error.to_string()))?;
        (self.dependencies.publish_user)(&capture, scan, user, true, Some(user_record))?;
        for name in &creation.kernel_exclusions { (self.dependencies.kernel_exclusion)(name, user)?; }
        let mut response = Parcel::new();
        response.write_i32(i32::try_from(creation.app_data.len()).map_err(|error| failure(error.to_string()))?);
        for data in &creation.app_data {
            response.write_string16(data.volume_uuid.as_deref()); response.write_string16(Some(&data.package));
            response.write_i32(data.flags); response.write_i32(data.app_id);
            response.write_string16(data.seinfo.as_deref()); response.write_i32(data.target_sdk_version);
        }
        self.pending.lock().unwrap().insert(user, creation);
        Ok(response.data().to_vec())
    }
    pub fn finish_user_creation(&self, user: i32) -> Result<(), Exception> {
        if !self.pending.lock().unwrap().contains_key(&user) { return Err(failure("native user app-data phase was not prepared")); }
        (self.dependencies.finish_permission_creation)(user)?;
        let sections = (self.dependencies.default_preferred)(user)?;
        let capture = (self.dependencies.capture)()?;
        self.disk.lock().unwrap().commit_initial_scan_restrictions(capture.scan().owner(), user as u32, false, sections)
            .map_err(|error| failure(error.to_string()))?;
        (self.dependencies.write_package_list)(&capture, Some(user))?;
        // publish_user rebuilds the complete native AppsFilter for this user;
        // no shadow-only onUserCreated notification is treated as publication.
        self.pending.lock().unwrap().remove(&user);
        Ok(())
    }
    pub fn clean_up_user_settings(&self, user: i32) -> Result<(), Exception> {
        if user < 0 { return Err(Exception::illegal_argument("negative removed user")); }
        let base = (self.dependencies.capture)()?;
        (self.dependencies.remove_unused_packages)(&base, user)?;
        let capture = (self.dependencies.capture)()?;
        let mut scan = capture.scan().owner().clone(); scan.remove_package_user_state(user).map_err(failure)?;
        self.disk.lock().unwrap().remove_package_user(user as u32).map_err(|error| failure(error.to_string()))?;
        // Source-side resolver disappears; other real registries retain their
        // identities while target-user filters are removed in SparseArray order.
        let users: Vec<_> = self.preferred.user_states().into_iter().map(|(source, _)| source).collect();
        for source in users {
            if source == user { continue; }
            if let Some(stage) = self.preferred.prepare_cross_profile_target_removal(source, user).map_err(failure)? {
                (self.dependencies.commit_cross_profile_stage)(stage)?;
            }
        }
        self.preferred.remove_user(user).map_err(failure)?;
        self.runtime.lock().unwrap().remove_user(user);
        (self.dependencies.clear_domain_user)(user)?;
        let current = (self.dependencies.publish_user)(&capture, scan, user, false, None)?;
        (self.dependencies.write_package_list)(&current, None)?;
        (self.dependencies.kernel_remove_user)(user)?;
        self.pending.lock().unwrap().remove(&user);
        Ok(())
    }
    pub fn finish_user_removal(&self, user: i32) -> Result<(), Exception> {
        (self.dependencies.instant_user_removed)(user)?;
        self.effects.user_removed(user)
    }
    pub fn read_permission_state(&self, user: i32) -> Result<(), Exception> { (self.dependencies.read_permission_state)(user) }
    pub fn clear_domain_user(&self, user: i32) -> Result<(), Exception> { (self.dependencies.clear_domain_user)(user) }
    pub fn has_system_feature(&self, name: Option<&str>, version: i32) -> Result<bool, Exception> {
        let state = (self.dependencies.capture)()?;
        Ok(name.and_then(|name| state.state().system.features.iter().find(|(feature, _)| feature == name)).is_some_and(|(_, supported)| *supported >= version))
    }
    pub fn is_device_upgrading(&self) -> bool { self.lifecycle.device_upgrading() }
    pub fn add_cross_profile(&self, filter: IntentFilter, owner: Option<String>, source: i32, target: i32, flags: i32) -> Result<(), Exception> {
        (self.dependencies.add_cross_profile)(filter, owner, source, target, flags)
    }
}
fn failure(message: impl Into<String>) -> Exception { Exception::new(EX_ILLEGAL_STATE, message.into()) }

pub struct Endpoint(pub Arc<Owner>);
struct Filter(IntentFilter);
impl aim_service_aidl::ReadParcelable for Filter {
    fn read_from(reader: &mut aim_binder_host::parcel::Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        IntentFilter::read(reader, &mut super::intent_filter::Plain).map(Self)
    }
}
impl aim_binder_host::local::Service for Endpoint {
    fn descriptor(&self) -> &str { aim_service_aidl::dev_aim_server_ipackageuseroperations::DESCRIPTOR }
    fn transact(&self, call: &mut aim_binder_host::local::Call<'_>) -> aim_binder_host::local::Reply {
        use aim_service_aidl::dev_aim_server_ipackageuseroperations as api;
        use aim_binder_host::parcel::{BAD_VALUE, UNKNOWN_TRANSACTION};
        call.data.enforce_interface(api::DESCRIPTOR)?;
        call.data.set_position(0);
        let mut reply = Parcel::new();
        if call.sender_euid != 1000 { reply.write_exception(&Exception::security("package user operations require original system UID")); return Ok(reply); }
        let result = match call.code {
            api::CREATE_USER_STATE => {
                let args = api::CreateUserState::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                args.user_record.ok_or_else(|| Exception::illegal_argument("original user record absent"))
                    .and_then(|user_record| self.0.create_user_state(args.user_id, args.installable_packages, args.disallowed_packages,
                        args.current_time_millis, args.stop_system_packages, user_record))
                    .map(|bytes| api::write_create_user_state_reply(&mut reply, &Some(bytes)))
            }
            api::FINISH_USER_CREATION => {
                let args = api::FinishUserCreation::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                self.0.finish_user_creation(args.user_id).map(|()| api::write_finish_user_creation_reply(&mut reply))
            }
            api::CLEAN_UP_USER_SETTINGS => {
                let args = api::CleanUpUserSettings::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                self.0.clean_up_user_settings(args.user_id).map(|()| api::write_clean_up_user_settings_reply(&mut reply))
            }
            api::FINISH_USER_REMOVAL => {
                let args = api::FinishUserRemoval::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                self.0.finish_user_removal(args.user_id).map(|()| api::write_finish_user_removal_reply(&mut reply))
            }
            api::READ_PERMISSION_STATE_FOR_USER => {
                let args = api::ReadPermissionStateForUser::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                self.0.read_permission_state(args.user_id).map(|()| api::write_read_permission_state_for_user_reply(&mut reply))
            }
            api::CLEAR_DOMAIN_USER => {
                let args = api::ClearDomainUser::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                self.0.clear_domain_user(args.user_id).map(|()| api::write_clear_domain_user_reply(&mut reply))
            }
            api::HAS_SYSTEM_FEATURE => {
                let args = api::HasSystemFeature::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                self.0.has_system_feature(args.name.as_deref(), args.version).map(|value| api::write_has_system_feature_reply(&mut reply, value))
            }
            api::IS_DEVICE_UPGRADING => {
                api::IsDeviceUpgrading::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                api::write_is_device_upgrading_reply(&mut reply, self.0.is_device_upgrading()); Ok(())
            }
            api::ADD_CROSS_PROFILE_INTENT_FILTER => {
                let args = api::AddCrossProfileIntentFilter::<Filter>::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                match args.filter {
                    Some(filter) => self.0.add_cross_profile(filter.0, args.owner_package, args.source_user_id, args.target_user_id, args.flags)
                        .map(|()| api::write_add_cross_profile_intent_filter_reply(&mut reply)),
                    None => Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null cross-profile filter")),
                }
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        if let Err(error) = result { reply = Parcel::new(); reply.write_exception(&error); }
        Ok(reply)
    }
}
