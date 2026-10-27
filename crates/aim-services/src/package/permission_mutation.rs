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
                    system
                        .capture_package_queries_for(&bridge)
                        .and_then(|capture| {
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

/// IPackageManagerBase's deprecated permission reads use the live access
/// owner too, with the original caller and the default virtual device.
pub fn check(system: &Arc<crate::system::System>, call: &mut Call<'_>) -> Reply {
    let (code, request) = check_request(call)?;
    let original = match system.package_permission_call(code, &request) {
        Ok(reply) => reply,
        Err(error) => {
            let mut reply = Parcel::new();
            reply.write_exception(&error);
            return Ok(reply);
        }
    };
    let mut reader = original.reader();
    let mut reply = Parcel::new();
    match reader.read_exception()? {
        Err(error) => reply.write_exception(&error),
        Ok(()) => {
            let value = reader.read_i32()?;
            if code == permission::CHECK_PERMISSION {
                pm::write_check_permission_reply(&mut reply, value);
            } else {
                pm::write_check_uid_permission_reply(&mut reply, value);
            }
        }
    }
    if reader.remaining() != 0 {
        return Err(BAD_VALUE);
    }
    Ok(reply)
}
fn check_request(call: &mut Call<'_>) -> aim_binder_host::parcel::Result<(u32, Parcel)> {
    let mut request = Parcel::new();
    let code = match call.code {
        pm::CHECK_PERMISSION => {
            let args = pm::CheckPermission::read(&mut call.data)?;
            permission::CheckPermission {
                package_name: args.pkg_name,
                permission_name: args.perm_name,
                persistent_device_id: Some("default:0".into()),
                user_id: args.user_id,
            }
            .write(&mut request);
            permission::CHECK_PERMISSION
        }
        pm::CHECK_UID_PERMISSION => {
            let args = pm::CheckUidPermission::read(&mut call.data)?;
            permission::CheckUidPermission {
                uid: args.uid,
                permission_name: args.perm_name,
                device_id: 0,
            }
            .write(&mut request);
            permission::CHECK_UID_PERMISSION
        }
        _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
    };
    if call.data.remaining() != 0 {
        return Err(BAD_VALUE);
    }
    Ok((code, request))
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_reads_translate_device_arguments_without_changing_the_caller() {
        for uid in [1000, 2000, 10123] {
            let mut request = Parcel::new();
            pm::CheckPermission {
                perm_name: Some("p.camera".into()),
                pkg_name: Some("a.b".into()),
                user_id: 10,
            }
            .write(&mut request);
            let mut call = Call {
                code: pm::CHECK_PERMISSION,
                flags: 0,
                sender_pid: 431,
                sender_euid: uid,
                data: request.reader(),
            };
            let (code, translated) = check_request(&mut call).unwrap();
            assert_eq!(code, permission::CHECK_PERMISSION);
            let args = permission::CheckPermission::read(&mut translated.reader()).unwrap();
            assert_eq!(
                (
                    args.package_name.as_deref(),
                    args.permission_name.as_deref(),
                    args.persistent_device_id.as_deref(),
                    args.user_id
                ),
                (Some("a.b"), Some("p.camera"), Some("default:0"), 10)
            );
            assert_eq!((call.sender_pid, call.sender_euid), (431, uid));
            let mut request = Parcel::new();
            pm::CheckUidPermission {
                perm_name: None,
                uid: 99001,
            }
            .write(&mut request);
            call.code = pm::CHECK_UID_PERMISSION;
            call.data = request.reader();
            let (code, translated) = check_request(&mut call).unwrap();
            assert_eq!(code, permission::CHECK_UID_PERMISSION);
            let args = permission::CheckUidPermission::read(&mut translated.reader()).unwrap();
            assert_eq!(
                (args.uid, args.permission_name, args.device_id),
                (99001, None, 0)
            );
            let mut malformed = request.clone();
            malformed.write_i32(42);
            call.data = malformed.reader();
            assert_eq!(check_request(&mut call).unwrap_err(), BAD_VALUE);
        }
    }
}
