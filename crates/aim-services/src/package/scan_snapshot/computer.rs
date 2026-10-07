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
use aim_service_aidl::dev_aim_server_ipackagecomputer as api;
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
        let position=call.data.position();
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
            Action::Filter(a) => match query.filtered(
                capture
                    .state()
                    .packages
                    .get(a.package_name.as_deref().unwrap_or_default()),
                a.calling_uid,
                a.user_id,
            ) {
                Ok(value) => api::write_filter_app_access_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            _ => unreachable!(),
        }
        Ok(reply)
    }
}
