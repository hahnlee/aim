//! Pre-scan original UserManager bridge, with an explicit one-way owner handoff.
//! SystemConfig is available before original UM construction. Restored lifecycle
//! is bound once from the real Settings owner after that constructor returns.
use super::{lifecycle, system_config::SystemConfig, user_operations};
use aim_binder_host::{
    local::{Call, Reply, Service},
    parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, UNKNOWN_TRANSACTION},
};
use aim_service_aidl::dev_aim_server_ipackageuseroperations as api;
use std::sync::{Arc, Mutex};

pub struct Endpoint {
    features: Vec<(String, i32)>,
    lifecycle: Mutex<Option<Arc<lifecycle::Owner>>>,
    late: Mutex<Option<Arc<user_operations::Owner>>>,
    closed: std::sync::atomic::AtomicBool,
}
impl Endpoint {
    pub fn new(config: &SystemConfig) -> Arc<Self> {
        Arc::new(Self {
            features: config.features.clone(),
            lifecycle: Mutex::new(None),
            late: Mutex::new(None),
            closed: std::sync::atomic::AtomicBool::new(false),
        })
    }
    pub fn bind_lifecycle(&self, owner: Arc<lifecycle::Owner>) -> Result<(), Exception> {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "original UM package epoch closed",
            ));
        }
        if lifecycle.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "original UM lifecycle already bound",
            ));
        }
        *lifecycle = Some(owner);
        Ok(())
    }
    pub fn attach_after_scan(&self, owner: Arc<user_operations::Owner>) -> Result<(), Exception> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "original UM package epoch closed",
            ));
        }
        let mut late = self.late.lock().unwrap();
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "original UM package epoch closed",
            ));
        }
        if late.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "original UM production user owner already attached",
            ));
        }
        *late = Some(owner);
        Ok(())
    }
    pub fn is_attached(&self) -> bool {
        self.late.lock().unwrap().is_some()
    }
    pub fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        let late = { self.late.lock().unwrap().take() };
        let lifecycle = { self.lifecycle.lock().unwrap().take() };
        drop(late);
        drop(lifecycle);
    }
}
impl Service for Endpoint {
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
        let mut reply = Parcel::new();
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            reply.write_exception(&Exception::new(
                EX_ILLEGAL_STATE,
                "original UM package epoch closed",
            ));
            return Ok(reply);
        }
        if call.sender_euid != 1000 {
            reply.write_exception(&Exception::security(
                "original UM native host requires system UID",
            ));
            return Ok(reply);
        }
        // Once attached, every operation (including read facts) uses the same
        // complete native production owner and its current capture.
        let late = { self.late.lock().unwrap().clone() };
        if let Some(owner) = late {
            return user_operations::Endpoint(owner).transact(call);
        }
        match call.code {
            api::HAS_SYSTEM_FEATURE => {
                let args = api::HasSystemFeature::read(&mut call.data)?;
                if call.data.remaining() != 0 {
                    return Err(BAD_VALUE);
                }
                let feature = args
                    .name
                    .as_ref()
                    .and_then(|name| self.features.iter().find(|(feature, _)| feature == name));
                api::write_has_system_feature_reply(
                    &mut reply,
                    feature.is_some_and(|(_, version)| *version >= args.version),
                );
            }
            api::IS_DEVICE_UPGRADING => {
                api::IsDeviceUpgrading::read(&mut call.data)?;
                if call.data.remaining() != 0 {
                    return Err(BAD_VALUE);
                }
                let lifecycle = { self.lifecycle.lock().unwrap().clone() };
                match lifecycle {
                    Some(owner) => {
                        api::write_is_device_upgrading_reply(&mut reply, owner.device_upgrading())
                    }
                    None => reply.write_exception(&Exception::new(
                        EX_ILLEGAL_STATE,
                        "original UM restored lifecycle is not yet bound",
                    )),
                }
            }
            _ => {
                // Native scan/settings/permission/domain owners are not yet
                // available. Unexpected constructor writes fail explicitly,
                // rather than returning invented empty package state/success.
                reply.write_exception(&Exception::new(
                    EX_ILLEGAL_STATE,
                    "package user operation requires completed native scan owner",
                ));
            }
        }
        Ok(reply)
    }
}
