//! PackageInstallerSession user-action admission, android-16.0.0_r1 (AOSP Apache-2.0).
use super::{Record, policy, preapproval::BridgeOwner, silent};
use crate::package::{info, pkg::booleans2, query::Query};
use aim_binder_host::parcel::{Exception, Parcel};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    None,
    Required,
    PendingApkParsing,
    UpdateOwnerReminder,
}
pub struct Input<'a> {
    pub record: &'a Record,
    pub package_name: Option<&'a str>,
    pub manually_accepted: bool,
    pub multi_package: bool,
    pub has_device_admin_receiver: bool,
    pub is_sdk_or_static_library: bool,
    pub validated_target_sdk: i32,
}
fn permission(query: &Query<'_>, uid: i32, name: &str) -> Result<bool, Exception> {
    query.uid_has_permission(uid, name).map_err(policy::unknown)
}
fn target_uid(query: &Query<'_>, package: Option<&str>, user: i32) -> Result<i32, Exception> {
    let mut request = Parcel::new();
    pm::GetPackageUid {
        package_name: package,
        flags: 1i64 << 32,
        user_id: user,
    }
    .write(&mut request);
    let reply = query
        .answer(
            pm::DESCRIPTOR,
            pm::GET_PACKAGE_UID,
            &mut aim_binder_host::parcel::Reader::new(request.data(), request.objects()),
        )
        .map_err(policy::unknown)?;
    pm::read_get_package_uid_reply(&mut aim_binder_host::parcel::Reader::new(
        reply.data(),
        reply.objects(),
    ))
    .map_err(|code| {
        Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,
            format!("user-action package UID reply: {code}"),
        )
    })?
}
fn emergency(
    query: &Query<'_>,
    package: Option<&str>,
    user: i32,
    installer_uid: i32,
) -> Result<bool, Exception> {
    let Some(name) = package else {
        return Ok(false);
    };
    let name = query.resolve_internal_package_name(name, -1);
    let Some(setting) = query.state.packages.get(&name) else {
        return Ok(false);
    };
    let Some(code) = setting.pkg.as_deref().filter(|_| setting.is.system) else {
        return Ok(false);
    };
    let Some(emergency) = code.emergency_installer.as_deref() else {
        return Ok(false);
    };
    if !query
        .packages_for_uid(installer_uid)
        .map_err(policy::unknown)?
        .is_some_and(|names| names.iter().any(|name| name.as_deref() == Some(emergency)))
    {
        return Ok(false);
    }
    let target = info::uid(user, setting.app_id);
    if !permission(query, target, "android.permission.INSTALL_PACKAGES")?
        && !permission(query, target, "android.permission.INSTALL_PACKAGE_UPDATES")?
        && !permission(query, target, "android.permission.INSTALL_SELF_UPDATES")?
    {
        return Ok(false);
    }
    permission(
        query,
        installer_uid,
        "android.permission.EMERGENCY_INSTALL_PACKAGES",
    )
}
/// Query uses the original worker identity (SYSTEM_UID), not the installer UID.
/// External DPM/UM/source-policy calls happen outside session/install locks.
pub fn compute(
    query: &Query<'_>,
    bridge: &BridgeOwner,
    input: &Input<'_>,
) -> Result<Requirement, Exception> {
    if input.multi_package || input.manually_accepted {
        return Ok(Requirement::None);
    }
    let record = input.record;
    let uid = record.installer_uid as i32;
    let user = record.user as i32;
    let flags = record.params.install_flags;
    let forced = flags & 0x400 != 0 || record.params.require_user_action == 1;
    let ordinary = if forced {
        Requirement::Required
    } else {
        Requirement::None
    };
    let install = permission(query, uid, "android.permission.INSTALL_PACKAGES")?;
    let self_updates = permission(query, uid, "android.permission.INSTALL_SELF_UPDATES")?;
    let updates = permission(query, uid, "android.permission.INSTALL_PACKAGE_UPDATES")?;
    let silent = permission(
        query,
        uid,
        "android.permission.UPDATE_PACKAGES_WITHOUT_USER_ACTION",
    )?;
    let dpc = permission(query, uid, "android.permission.INSTALL_DPC_PACKAGES")?;
    let external = bridge.user_action_policy(record.installer_package.as_deref(), uid, user)?;
    let dependencies = external & 4 != 0
        && permission(
            query,
            uid,
            "android.permission.INSTALL_DEPENDENCY_SHARED_LIBRARIES",
        )?;
    let target_uid = target_uid(query, input.package_name, user)?;
    let apex = flags & 0x20000 != 0;
    let update = target_uid != -1 || apex;
    let existing = if update {
        input
            .package_name
            .map(|name| query.resolve_internal_package_name(name, -1))
            .and_then(|name| query.state.packages.get(&name))
            .filter(|setting| {
                setting
                    .pkg
                    .as_ref()
                    .is_none_or(|code| code.booleans2 & booleans2::APEX == 0)
            })
            .map(|setting| &setting.install_source)
    } else {
        None
    };
    let existing_installer = existing.and_then(|source| source.installer.as_deref());
    let existing_owner = existing.and_then(|source| source.update_owner.as_deref());
    let installer = record.installer_package.as_deref();
    let installer_of_record = update && existing_installer == installer;
    let update_owner = existing_owner == installer;
    let self_update = target_uid == uid;
    let emergency = emergency(query, input.package_name, user, uid)?;
    let permitted = install
        || updates && update
        || self_updates && self_update
        || dpc && input.has_device_admin_receiver
        || dependencies && input.is_sdk_or_static_library;
    let ownership = external & 2 != 0 && existing_owner.is_some();
    if uid == 0 || uid == 1000 || external & 1 != 0 || emergency || flags & (1 << 30) != 0 {
        return Ok(ordinary);
    }
    if ownership && !apex && !update_owner && uid != 2000 && flags & (1 << 26) == 0 {
        return Ok(Requirement::UpdateOwnerReminder);
    }
    if permitted {
        return Ok(ordinary);
    }
    if external & 8 != 0 {
        return Ok(Requirement::Required);
    }
    if record.params.require_user_action == 2
        && silent
        && (if ownership {
            update_owner
        } else {
            installer_of_record
        } || self_update)
    {
        return Ok(Requirement::PendingApkParsing);
    }
    Ok(Requirement::Required)
}
/// The source checkUserActionRequirement's parsing/compatibility/throttle phase.
/// Returns the requirement retained in SessionInfo, and whether a prompt is sent.
pub fn check(
    query: &Query<'_>,
    bridge: &BridgeOwner,
    input: &Input<'_>,
    silent_policy: &mut silent::Policy,
    uptime_millis: i64,
) -> Result<(Requirement, bool), Exception> {
    let requirement = compute(query, bridge, input)?;
    if matches!(
        requirement,
        Requirement::Required | Requirement::UpdateOwnerReminder
    ) {
        return Ok((requirement, true));
    }
    if requirement == Requirement::PendingApkParsing
        && input.record.params.install_flags & 0x20000 == 0
    {
        if !bridge.silent_install_target_allowed(input.package_name, input.validated_target_sdk)? {
            return Ok((requirement, true));
        }
        if input.record.params.require_user_action == 2 {
            let package = input.package_name.ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "silent install parsed package unavailable",
                )
            })?;
            let installer = input.record.installer_package.as_deref();
            if !silent_policy.allowed(installer, package, uptime_millis) {
                return Ok((requirement, true));
            }
            silent_policy.track(installer, package, uptime_millis);
        }
    }
    Ok((requirement, false))
}
