//! Prepared sync handover (#1460 6c). Public content routing is not connected.
use super::{AppHost, reply};
use aim_binder_host::{
    local::{Call, LocalProcess, Reply, Strong},
    parcel::{BAD_VALUE, PERMISSION_DENIED, Parcel, UNKNOWN_TRANSACTION},
};
use aim_service_aidl::android_content_icontentservice as cs;

const SYNC_CODES: &[u32] = &[
    cs::REQUEST_SYNC,
    cs::SYNC,
    cs::SYNC_AS_USER,
    cs::CANCEL_SYNC,
    cs::CANCEL_SYNC_AS_USER,
    cs::CANCEL_REQUEST,
    cs::GET_SYNC_AUTOMATICALLY,
    cs::GET_SYNC_AUTOMATICALLY_AS_USER,
    cs::SET_SYNC_AUTOMATICALLY,
    cs::SET_SYNC_AUTOMATICALLY_AS_USER,
    cs::GET_PERIODIC_SYNCS,
    cs::ADD_PERIODIC_SYNC,
    cs::REMOVE_PERIODIC_SYNC,
    cs::GET_IS_SYNCABLE,
    cs::GET_IS_SYNCABLE_AS_USER,
    cs::SET_IS_SYNCABLE,
    cs::SET_IS_SYNCABLE_AS_USER,
    cs::SET_MASTER_SYNC_AUTOMATICALLY,
    cs::SET_MASTER_SYNC_AUTOMATICALLY_AS_USER,
    cs::GET_MASTER_SYNC_AUTOMATICALLY,
    cs::GET_MASTER_SYNC_AUTOMATICALLY_AS_USER,
    cs::GET_CURRENT_SYNCS,
    cs::GET_CURRENT_SYNCS_AS_USER,
    cs::GET_SYNC_ADAPTER_TYPES,
    cs::GET_SYNC_ADAPTER_TYPES_AS_USER,
    cs::GET_SYNC_ADAPTER_PACKAGES_FOR_AUTHORITY_AS_USER,
    cs::GET_SYNC_ADAPTER_PACKAGE_AS_USER,
    cs::IS_SYNC_ACTIVE,
    cs::GET_SYNC_STATUS,
    cs::GET_SYNC_STATUS_AS_USER,
    cs::IS_SYNC_PENDING,
    cs::IS_SYNC_PENDING_AS_USER,
    cs::ADD_STATUS_CHANGE_LISTENER,
    cs::REMOVE_STATUS_CHANGE_LISTENER,
    cs::RESET_TODAY_STATS,
];

fn check(process: &LocalProcess, call: &Call<'_>) -> Result<(), i32> {
    if !SYNC_CODES.contains(&call.code) {
        return Err(UNKNOWN_TRANSACTION);
    }
    if call.is_oneway() || call.data.position() != 0 {
        return Err(BAD_VALUE);
    }
    if process.authenticated_inbound_identity() != Some((call.sender_pid, call.sender_euid)) {
        return Err(PERMISSION_DENIED);
    }
    Ok(())
}

fn forward(process: &LocalProcess, owner: &Strong, call: &Call<'_>) -> Reply {
    check(process, call)?;
    // Leave decoding, null/AsUser validation, permissions and redaction to the original.
    // Copy the entire wire parcel, including its token and relative object offsets.
    let mut data = call.data.clone();
    data.set_position(data.remaining());
    let (bytes, objects) = data.since(0);
    let mut request = Parcel::new();
    request.write_raw(bytes, &objects);
    process
        .transact_preserving_inbound(owner.handle, call.code, &request)
        .map(|answer| answer.into_parcel())
}

impl AppHost {
    // Intentionally unconnected until the atomic 6f handover.
    #[allow(dead_code)]
    pub(super) fn forward_sync(&self, call: &Call<'_>) -> Reply {
        check(&self.process, call)?;
        let owner = match self.sync_forwarding_owner() {
            Ok(owner) => owner,
            Err(error) => return reply(Err(error)),
        };
        // No owner/process lock spans the original or any nested callback.
        forward(&self.process, &owner, call)
    }
}

#[cfg(test)]
mod tests;
