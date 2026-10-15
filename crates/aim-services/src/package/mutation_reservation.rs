//! Captured native package write reservation; callbacks execute outside locks.
use super::{bootstrap::Bridge, scan_snapshot::{query_state::Capture, endpoint::Endpoint}};
use crate::system::System;
use aim_binder_host::{local::{Call, Reply, Service}, parcel::{Binder, Exception, Parcel, EX_ILLEGAL_STATE, BAD_VALUE, UNKNOWN_TRANSACTION}};
use aim_service_aidl::dev_aim_server_ipackagemutationreservation as api;
use std::sync::{Arc, Mutex, Weak};

pub struct Reservation {
    system: Weak<System>, bridge: Arc<Bridge>, sequence: i32,
    state: Mutex<Option<State>>,
}
struct State { capture: Arc<Capture>, endpoint: Arc<Endpoint>, binder: Binder }
fn error(message: impl Into<String>) -> Exception { Exception::new(EX_ILLEGAL_STATE, message) }
impl Reservation {
    pub(crate) fn new(system: &Arc<System>, bridge: Arc<Bridge>, capture: Arc<Capture>, sequence: i32) -> Self {
        let endpoint = Arc::new(Endpoint::with_computer(capture.scan().clone(), capture.clone(), &system.binder_process()));
        let binder = system.binder_process().add_service(endpoint.clone());
        Self { system: Arc::downgrade(system), bridge, sequence, state: Mutex::new(Some(State { capture, endpoint, binder })) }
    }
    fn captured(&self) -> Result<(Arc<System>, Arc<Capture>, Binder), Exception> {
        let system = self.system.upgrade().ok_or_else(|| error("mutation system stopped"))?;
        system.check_package_bootstrap(&self.bridge)?;
        let state = self.state.lock().unwrap();
        let state = state.as_ref().ok_or_else(|| error("mutation reservation closed"))?;
        Ok((system, state.capture.clone(), state.binder))
    }
    pub(crate) fn close(&self) {
        if let Some(state) = self.state.lock().unwrap().take() { state.endpoint.revoke_install_scope(); }
    }
}
impl Drop for Reservation { fn drop(&mut self) { self.close(); } }
impl Service for Reservation {
    fn descriptor(&self) -> &str { api::DESCRIPTOR }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !api::METHODS.iter().any(|(code, _)| *code == call.code) { return Err(UNKNOWN_TRANSACTION); }
        if call.sender_euid != crate::SYSTEM_UID {
            let mut reply = Parcel::new(); reply.write_exception(&Exception::security("package mutation reservation serves system UID only")); return Ok(reply);
        }
        let mut reply = Parcel::new();
        let result = (|| -> Result<(), Exception> {
            match call.code {
                api::GET_SNAPSHOT => {
                    api::GetSnapshot::read(&mut call.data).map_err(|status| error(format!("mutation snapshot request: {status}")))?;
                    if call.data.remaining() != 0 { return Err(error("mutation trailing bytes")); }
                    let (_, _, binder) = self.captured()?;
                    api::write_get_snapshot_reply(&mut reply, Some(binder));
                }
                api::GET_CHANGED_PACKAGES_SEQUENCE => {
                    api::GetChangedPackagesSequence::read(&mut call.data).map_err(|status| error(format!("mutation sequence request: {status}")))?;
                    if call.data.remaining() != 0 { return Err(error("mutation trailing bytes")); }
                    self.captured()?;
                    api::write_get_changed_packages_sequence_reply(&mut reply, self.sequence);
                }
                api::GET_DISABLED_USER_ALIASES => {
                    let args = api::GetDisabledUserAliases::read(&mut call.data).map_err(|status| error(format!("mutation alias request: {status}")))?;
                    if call.data.remaining() != 0 { return Err(error("mutation trailing bytes")); }
                    let (_, capture, _) = self.captured()?;
                    let package = args.package_name.ok_or_else(|| Exception::illegal_argument("mutation alias package absent"))?;
                    let aliases = capture.scan().owner().disabled_user_aliases(&package).map_err(error)?;
                    api::write_get_disabled_user_aliases_reply(&mut reply, &Some(aliases));
                }
                api::PUBLISH => {
                    let args = api::Publish::read(&mut call.data).map_err(|status| error(format!("mutation publish request: {status}")))?;
                    if call.data.remaining() != 0 { return Err(error("mutation trailing bytes")); }
                    let (system, capture, _) = self.captured()?;
                    let bytes = args.mutation_record.ok_or_else(|| Exception::illegal_argument("mutation record absent"))?;
                    system.publish_internal_package_mutation(&self.bridge, &capture, args.expected_version, &bytes)?;
                    self.close();
                    api::write_publish_reply(&mut reply);
                }
                api::CLOSE => {
                    api::Close::read(&mut call.data).map_err(|status| error(format!("mutation close request: {status}")))?;
                    if call.data.remaining() != 0 { return Err(error("mutation trailing bytes")); }
                    self.close(); api::write_close_reply(&mut reply);
                }
                _ => return Err(error(format!("mutation transaction {BAD_VALUE}"))),
            }
            Ok(())
        })();
        if let Err(error) = result { reply = Parcel::new(); reply.write_exception(&error); }
        Ok(reply)
    }
}
