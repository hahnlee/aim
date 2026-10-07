//! A system-only lease on computed queries of one immutable package capture.
use super::query_state::Capture;
use crate::package::{query::Query, resolve::Resolver};
use aim_binder_host::{
    local::{Call, Reply, Service},
    parcel::{
        BAD_VALUE, EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, Exception, Parcel,
        UNKNOWN_TRANSACTION,
    },
};
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm, dev_aim_server_ipackagecomputer as api,
};
use std::sync::{Arc, Mutex};

pub(crate) struct Computer {
    capture: Mutex<Option<Arc<Capture>>>,
    resolver: Resolver,
}
impl Computer {
    pub(crate) fn new(capture: Arc<Capture>) -> Self {
        Self {
            capture: Mutex::new(Some(capture)),
            resolver: Resolver::default(),
        }
    }
}
impl Service for Computer {
    fn descriptor(&self) -> &str {
        api::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !api::METHODS.iter().any(|(code, _)| *code == call.code) {
            return Err(UNKNOWN_TRANSACTION);
        }
        let position = call.data.position();
        call.data.enforce_interface(api::DESCRIPTOR)?;
        call.data.set_position(position);
        if call.sender_euid != crate::SYSTEM_UID {
            let mut reply = Parcel::new();
            reply.write_exception(&Exception::security(
                "package computer serves system UID only",
            ));
            return Ok(reply);
        }
        enum Action {
            Version,
            Application(api::GetApplicationInfo),
            Package(api::GetPackageInfo),
            Filter(api::FilterAppAccess),
            Close,
            Forward(u32, Parcel, i32),
            Installer(api::GetInstallerPackageName),
            InternalUid(api::GetPackageUidInternal),
            InternalName(api::ResolveInternalPackageName),
        }
        let action = match call.code {
            api::GET_VERSION => {
                api::GetVersion::read(&mut call.data)?;
                Action::Version
            }
            api::GET_APPLICATION_INFO => {
                Action::Application(api::GetApplicationInfo::read(&mut call.data)?)
            }
            api::GET_PACKAGE_INFO => Action::Package(api::GetPackageInfo::read(&mut call.data)?),
            api::FILTER_APP_ACCESS => Action::Filter(api::FilterAppAccess::read(&mut call.data)?),
            api::CLOSE => {
                api::Close::read(&mut call.data)?;
                Action::Close
            }
            api::GET_PACKAGE_UID => {
                let a = api::GetPackageUid::read(&mut call.data)?;
                let mut data = Parcel::new();
                pm::GetPackageUid {
                    package_name: a.package_name,
                    flags: a.flags,
                    user_id: a.user_id,
                }
                .write(&mut data);
                Action::Forward(pm::GET_PACKAGE_UID, data, a.calling_uid)
            }
            api::GET_PACKAGES_FOR_UID => {
                let a = api::GetPackagesForUid::read(&mut call.data)?;
                let mut data = Parcel::new();
                pm::GetPackagesForUid { uid: a.uid }.write(&mut data);
                Action::Forward(pm::GET_PACKAGES_FOR_UID, data, a.calling_uid)
            }
            api::GET_NAME_FOR_UID => {
                let a = api::GetNameForUid::read(&mut call.data)?;
                let mut data = Parcel::new();
                pm::GetNameForUid { uid: a.uid }.write(&mut data);
                Action::Forward(pm::GET_NAME_FOR_UID, data, a.calling_uid)
            }
            api::IS_INSTANT_APP => {
                let a = api::IsInstantApp::read(&mut call.data)?;
                let mut data = Parcel::new();
                pm::IsInstantApp {
                    package_name: a.package_name,
                    user_id: a.user_id,
                }
                .write(&mut data);
                Action::Forward(pm::IS_INSTANT_APP, data, a.calling_uid)
            }
            api::GET_TARGET_SDK_VERSION => {
                let a = api::GetTargetSdkVersion::read(&mut call.data)?;
                let mut data = Parcel::new();
                pm::GetTargetSdkVersion {
                    package_name: a.package_name,
                }
                .write(&mut data);
                Action::Forward(pm::GET_TARGET_SDK_VERSION, data, a.calling_uid)
            }
            api::GET_INSTALLER_PACKAGE_NAME => {
                Action::Installer(api::GetInstallerPackageName::read(&mut call.data)?)
            }
            api::GET_PACKAGE_UID_INTERNAL=>Action::InternalUid(api::GetPackageUidInternal::read(&mut call.data)?),
            api::RESOLVE_INTERNAL_PACKAGE_NAME=>Action::InternalName(api::ResolveInternalPackageName::read(&mut call.data)?),
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        let mut reply = Parcel::new();
        if matches!(action, Action::Close) {
            self.capture.lock().unwrap().take();
            api::write_close_reply(&mut reply);
            return Ok(reply);
        }
        let Some(capture) = self.capture.lock().unwrap().clone() else {
            reply.write_exception(&Exception::new(
                EX_ILLEGAL_STATE,
                "package computer is closed",
            ));
            return Ok(reply);
        };
        if matches!(action, Action::Version) {
            api::write_get_version_reply(&mut reply, capture.scan().version() as i64);
            return Ok(reply);
        }
        let resolution = match self.resolver.resolution(capture.state()) {
            Ok(value) => value,
            Err(error) => return error.reply(),
        };
        let uid = match &action {
            Action::Application(a) => a.calling_uid,
            Action::Package(a) => a.calling_uid,
            Action::Filter(a) => a.calling_uid,
            Action::Forward(_, _, uid) => *uid,
            Action::Installer(a) => a.calling_uid,
            Action::InternalUid(_)=>1000,
            Action::InternalName(a)=>a.calling_uid,
            _ => unreachable!(),
        };
        if uid < 0 {
            reply.write_exception(&Exception::illegal_argument("negative caller UID"));
            return Ok(reply);
        }
        let query = Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let unsupported = |reply: &mut Parcel, error: crate::package::apps_filter::NotModelled| {
            reply.write_exception(&Exception::new(EX_UNSUPPORTED_OPERATION, error.0))
        };
        match action {
            Action::Application(a) => match query.application_info_internal(
                a.package_name.as_deref().unwrap_or_default(),
                a.flags,
                a.user_id,
                a.filter_calling_uid,
            ) {
                Ok(Ok(value)) => api::write_get_application_info_reply(&mut reply, value.as_ref()),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Package(a) => match query.package_info_internal(
                a.package_name.as_deref().unwrap_or_default(),
                -1,
                a.flags,
                a.user_id,
                a.filter_calling_uid,
            ) {
                Ok(Ok(value)) => api::write_get_package_info_reply(&mut reply, value.as_ref()),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Filter(a) => match crate::package::apps_filter::should_filter_application(
                capture.state(),
                &resolution.apps_filter,
                capture
                    .state()
                    .packages
                    .get(a.package_name.as_deref().unwrap_or_default()),
                a.calling_uid,
                a.user_id,
                a.filter_uninstalled,
                true,
            ) {
                Ok(value) => api::write_filter_app_access_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Forward(code, data, _) => match query.answer(
                pm::DESCRIPTOR,
                code,
                &mut aim_binder_host::parcel::Reader::new(data.data(), data.objects()),
            ) {
                Ok(value) => return Ok(value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Installer(a) => match query.installer_package_internal(
                a.package_name.as_deref().unwrap_or_default(),
                a.user_id,
            ) {
                Ok(Ok(value)) => api::write_get_installer_package_name_reply(&mut reply, &value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::InternalUid(a)=>match query.package_uid_internal(a.package_name.as_deref().unwrap_or_default(),a.flags,a.user_id,1000) {
                Ok(value)=>api::write_get_package_uid_internal_reply(&mut reply,value),Err(error)=>unsupported(&mut reply,error),
            },
            Action::InternalName(a)=>api::write_resolve_internal_package_name_reply(&mut reply,&a.package_name.map(|name|query.resolve_internal_package_name(&name,a.version_code))),
            _ => unreachable!(),
        }
        Ok(reply)
    }
}
