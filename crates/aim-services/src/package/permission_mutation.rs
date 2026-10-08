//! Deprecated PMS permission methods delegate to the original PermissionManager
//! with the authenticated inbound Binder identity, as IPackageManagerBase does.
use aim_binder_host::{
    local::{Call, Reply},
    parcel::{BAD_VALUE, Parcel},
};
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm, android_permission_ipermissionmanager as permission,
};
use std::sync::Arc;

pub fn transact(system: &Arc<crate::system::System>, call: &mut Call<'_>) -> Option<Reply> {
    let mut request = Parcel::new();
    let code = match call.code {
        pm::ADD_PERMISSION | pm::ADD_PERMISSION_ASYNC => {
            // PermissionInfo is the only IPM argument. Preserve its complete
            // original Parcel representation; the original permission Stub
            // decodes it and enforces no trailing data before any mutation.
            if let Err(error) = call.data.enforce_interface(pm::DESCRIPTOR) {
                return Some(Err(error));
            }
            request.write_interface_token(permission::DESCRIPTOR);
            let start = call.data.position();
            let end = start + call.data.remaining();
            call.data.set_position(end);
            let (bytes, objects) = call.data.since(start);
            request.write_raw(bytes, &objects);
            request.write_bool(call.code == pm::ADD_PERMISSION_ASYNC);
            permission::ADD_PERMISSION
        }
        pm::REMOVE_PERMISSION => {
            let args = match pm::RemovePermission::read(&mut call.data) {
                Ok(args) => args,
                Err(error) => return Some(Err(error)),
            };
            if call.data.remaining() != 0 {
                return Some(Err(BAD_VALUE));
            }
            request.write_interface_token(permission::DESCRIPTOR);
            request.write_string16(args.name.as_deref());
            permission::REMOVE_PERMISSION
        }
        pm::GRANT_RUNTIME_PERMISSION => {
            let args = match pm::GrantRuntimePermission::read(&mut call.data) {
                Ok(args) => args,
                Err(error) => return Some(Err(error)),
            };
            if call.data.remaining() != 0 {
                return Some(Err(BAD_VALUE));
            }
            permission::GrantRuntimePermission {
                package_name: args.package_name,
                permission_name: args.permission_name,
                persistent_device_id: Some("default:0".into()),
                user_id: args.user_id,
            }
            .write(&mut request);
            permission::GRANT_RUNTIME_PERMISSION
        }
        _ => return None,
    };
    Some((|| {
        let mut reply = Parcel::new();
        let original = match system.package_permission_call(code, &request) {
            Ok(reply) => reply,
            Err(error) => {
                reply.write_exception(&error);
                return Ok(reply);
            }
        };
        let mut reader = original.reader();
        let result = reader.read_exception()?;
        match result {
            Err(error) => reply.write_exception(&error),
            Ok(()) => {
                let added = if code == permission::ADD_PERMISSION {
                    Some(reader.read_bool()?)
                } else {
                    None
                };
                if reader.remaining() != 0 {
                    return Err(BAD_VALUE);
                }
                // The mutation already belongs to PermissionManager. Publish
                // its live grants/GIDs before reporting success to native clients.
                let refresh = system.package_bootstrap().and_then(|bridge| {
                    system.capture_package_queries().and_then(|capture| {
                        system.refresh_package_permission_queries(&bridge, &capture)
                    })
                });
                match refresh {
                    Err(error) => reply.write_exception(&error),
                    Ok(()) => {
                        if let Some(added) = added {
                            if call.code == pm::ADD_PERMISSION_ASYNC {
                                pm::write_add_permission_async_reply(&mut reply, added);
                            } else {
                                pm::write_add_permission_reply(&mut reply, added);
                            }
                        } else if code == permission::REMOVE_PERMISSION {
                            pm::write_remove_permission_reply(&mut reply);
                        } else {
                            pm::write_grant_runtime_permission_reply(&mut reply);
                        }
                    }
                }
            }
        }
        Ok(reply)
    })())
}

pub fn group_info(system: &Arc<crate::system::System>, call: &mut Call<'_>) -> Reply {
    let args = pm::GetPermissionGroupInfo::read(&mut call.data)?;
    if call.data.remaining() != 0 {
        return Err(BAD_VALUE);
    }
    let mut request = Parcel::new();
    request.write_interface_token(permission::DESCRIPTOR);
    request.write_string16(args.name.as_deref());
    request.write_i32(args.flags);
    let original =
        match system.package_permission_call(permission::GET_PERMISSION_GROUP_INFO, &request) {
            Ok(reply) => reply,
            Err(error) => {
                let mut reply = Parcel::new();
                reply.write_exception(&error);
                return Ok(reply);
            }
        };
    // Both interfaces return the identical original PermissionGroupInfo typed
    // Parcelable, including metadata; preserve Binder capability offsets.
    let mut reader = original.reader();
    reader.set_position(reader.position() + reader.remaining());
    let (bytes, objects) = reader.since(0);
    let mut reply = Parcel::new();
    reply.write_raw(bytes, &objects);
    Ok(reply)
}
