//! Production user lifecycle dependency factory. Inputs are retained concrete
//! native owners; caller-supplied behavior callbacks are not accepted.
use crate::{system::System, package::{bootstrap::Bridge, owner::{Store, runtime_metadata},
    effects, instant, kernel_mappings, lifecycle, preferred, user_operations}};
use aim_binder_host::parcel::{Exception, EX_ILLEGAL_STATE};
use std::{collections::BTreeSet, sync::{Arc, Mutex}};
fn error(value: impl ToString) -> Exception { Exception::new(EX_ILLEGAL_STATE, value.to_string()) }

pub fn create(system: &Arc<System>, bridge: &Arc<Bridge>, disk: Arc<Mutex<Store>>,
    preferred: Arc<preferred::registry::Registry>, runtime: Arc<Mutex<runtime_metadata::State>>,
    instant: Arc<instant::Owner>, effects: Arc<effects::Owner>, lifecycle: Arc<lifecycle::Owner>,
    kernel: Arc<kernel_mappings::Owner>, fix_first_install_time: bool,
    non_stopped: BTreeSet<String>) -> Result<Arc<user_operations::Owner>, Exception> {
    system.check_package_bootstrap(bridge)?;
    let guarded = || (Arc::downgrade(system), bridge.clone());
    let (capture_system, capture_bridge) = guarded();
    let (publish_system, publish_bridge) = guarded();
    let (launcher_system, launcher_bridge) = guarded();
    let (defaults_system, defaults_bridge) = guarded();
    let (list_system, list_bridge) = guarded();
    let (permission_system, permission_bridge) = guarded();
    let (creation_system,creation_bridge)=guarded();
    let (domain_system, domain_bridge) = guarded();
    let (remove_system, remove_bridge) = guarded();
    let (preferred_system, preferred_bridge) = guarded();
    let (add_system, add_bridge) = guarded();
    let exclusion_system = Arc::downgrade(system); let exclusion_bridge = bridge.clone();
    let kernel_exclusion = kernel.clone(); let kernel_remove = kernel;
    let list_disk = disk.clone(); let domain_disk = disk.clone(); let permission_disk = disk.clone();
    let dependencies = user_operations::Dependencies {
        capture: Box::new(move || {
            let system = capture_system.upgrade().ok_or_else(|| error("native user system stopped"))?;
            system.check_package_bootstrap(&capture_bridge)?; system.capture_package_queries()
        }),
        publish_user: Box::new(move |base, scan, user, created, record| {
            let system = publish_system.upgrade().ok_or_else(|| error("native user publication system stopped"))?;
            system.check_package_bootstrap(&publish_bridge)?;
            system.publish_package_user_transition(&publish_bridge, base, scan, user, created, record)
        }),
        launcher: Box::new(move |name, user, flags| {
            let system = launcher_system.upgrade().ok_or("native launcher system stopped")?;
            system.check_package_bootstrap(&launcher_bridge).map_err(|error| format!("{error:?}"))?;
            system.native_package_has_launcher(name, user, flags).map_err(|error| format!("{error:?}"))
        }),
        kernel_exclusion: Box::new(move |name, _user| {
            let system = exclusion_system.upgrade().ok_or_else(|| error("native kernel mapping system stopped"))?;
            system.check_package_bootstrap(&exclusion_bridge)?;
            let capture = system.capture_package_queries()?;
            let package = capture.scan().owner().settings.packages.iter().find(|package| package.name == name).ok_or_else(|| error("kernel mapping package absent"))?;
            let users = capture.scan().owner().scanned_user_states(name).ok_or_else(|| error("kernel mapping user owner absent"))?;
            let excluded: Vec<_> = users.iter().filter_map(|(user, state)| (!state.installed).then_some(*user)).collect();
            kernel_exclusion.update(name, package.app_id, &excluded).map_err(error)
        }),
        kernel_remove_user: Box::new(move |user| kernel_remove.remove_user(user).map_err(error)),
        default_preferred: Box::new(move |user| {
            let system = defaults_system.upgrade().ok_or_else(|| error("default preferred system stopped"))?;
            system.check_package_bootstrap(&defaults_bridge)?;
            system.apply_native_default_preferred_apps(&defaults_bridge, user)
        }),
        write_package_list: Box::new(move |capture, creating_user| {
            let system = list_system.upgrade().ok_or_else(|| error("package list system stopped"))?;
            system.check_package_bootstrap(&list_bridge)?;
            let mut users: Vec<_> = capture.state().users.keys().copied().collect();
            if let Some(user) = creating_user { if !users.contains(&user) { users.push(user); users.sort(); } }
            system.commit_package_list_from_scan(&list_bridge, &mut list_disk.lock().unwrap(), capture, &users).map_err(|error| self::error(error.to_string()))
        }),
        finish_permission_creation:Box::new(move|user|{
            let system=creation_system.upgrade().ok_or_else(||error("runtime user creation System stopped"))?;
            system.publish_native_runtime_permission_creation(&creation_bridge,user)
        }),
        read_permission_state: Box::new(move |user| {
            let system = permission_system.upgrade().ok_or_else(|| error("native permission read system stopped"))?;
            system.check_package_bootstrap(&permission_bridge)?;
            system.read_native_package_permission_user(&permission_bridge, &permission_disk, user)
        }),
        clear_domain_user: Box::new(move |user| {
            let system = domain_system.upgrade().ok_or_else(|| error("native domain user system stopped"))?;
            system.check_package_bootstrap(&domain_bridge)?;
            let capture = system.capture_package_queries()?;
            let mut owner = capture.domains().ok_or_else(|| error("native domain owner absent"))?.owner().clone();
            owner.clear_user(user);
            let update = capture.prepare_domain_update(owner).map_err(error)?;
            system.commit_package_domains(&domain_bridge, update, &mut domain_disk.lock().unwrap()).map(|_| ()).map_err(|cause|error(format!("Native domain user commit committed={}: {}",cause.committed,cause.message)))
        }),
        remove_unused_packages: Box::new(move |capture, user| {
            let system = remove_system.upgrade().ok_or_else(|| error("native unused package removal system stopped"))?;
            system.check_package_bootstrap(&remove_bridge)?;
            system.remove_native_unused_packages_for_user(&remove_bridge, capture, user)
        }),
        instant_user_removed: Box::new(move |user| { instant.remove_user(user); Ok(()) }),
        commit_cross_profile_stage: Box::new(move |stage| {
            let system = preferred_system.upgrade().ok_or_else(|| error("native cross-profile system stopped"))?;
            system.check_package_bootstrap(&preferred_bridge)?;
            system.commit_native_package_preferred_stage(&preferred_bridge, stage)
        }),
        add_cross_profile: Box::new(move |filter, owner, source, target, flags| {
            let system = add_system.upgrade().ok_or_else(|| error("native cross-profile add system stopped"))?;
            system.check_package_bootstrap(&add_bridge)?;
            system.add_native_default_cross_profile_filter(&add_bridge, filter, owner, source, target, flags)
        }),
    };
    Ok(Arc::new(user_operations::Owner::new(disk, preferred, runtime, effects, lifecycle,
        dependencies, fix_first_install_time, non_stopped)))
}
