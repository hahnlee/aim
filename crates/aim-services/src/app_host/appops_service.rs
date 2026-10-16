//! `appops`: AppOps for the app host, for the ops its services decide by
//! (the clipboard's): an op's mode is the uid's, else the package's, as
//! the permission service last persisted them (`access.abx`), else the
//! op's default (`AppOpsManager.sAppOpInfos` at `android-16.0.0_r1`). The
//! modes do not change while the app host runs, so watchers are kept and
//! never called.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::com_android_internal_app_iappopsservice as appops;

use super::{AppHost, reply};

/// `AppOpsManager.MODE_ALLOWED`, `MODE_ERRORED`.
const MODE_ALLOWED: i32 = 0;
const MODE_ERRORED: i32 = 2;
/// The ops it decides: their code, their name and default mode. Both are
/// their own switch, and neither has a permission, so their notes are not
/// collected (`AppOpsService.shouldCollectNotes`).
const OPS: &[(i32, &str, i32)] = &[
    (29, "android:read_clipboard", MODE_ALLOWED),
    (30, "android:write_clipboard", MODE_ALLOWED),
];

pub struct AppOps {
    host: Arc<AppHost>,
    watchers: Mutex<Vec<(i32, Binder)>>,
}

impl AppOps {
    pub fn new(host: Arc<AppHost>) -> Self {
        AppOps { host, watchers: Mutex::new(Vec::new()) }
    }

    fn op(&self, code: i32) -> Result<&'static (i32, &'static str, i32), Exception> {
        OPS.iter().find(|(c, _, _)| *c == code).ok_or_else(|| {
            Exception::new(EX_UNSUPPORTED_OPERATION, format!("app op {code} is not decided by the app host"))
        })
    }

    /// `checkOperation`: the uid's mode, else the package's, else the
    /// op's default.
    fn mode(&self, code: i32, uid: i32, package: Option<&str>) -> Result<i32, Exception> {
        let (_, name, default) = *self.op(code)?;
        let access = self.host.inputs()?.packages.access();
        let named = |ops: &[(String, i32)]| ops.iter().find(|(n, _)| n == name).map(|(_, m)| *m);
        let app_id = uid.rem_euclid(100_000);
        let uid_mode = access.and_then(|a| a.app_id_app_ops.iter().find(|(id, _)| *id == app_id)).and_then(|(_, ops)| named(ops));
        let package_mode = package
            .and_then(|p| access.and_then(|a| a.package_app_ops.iter().find(|(name, _)| name == p)))
            .and_then(|(_, ops)| named(ops));
        Ok(uid_mode.or(package_mode).unwrap_or(default))
    }

    /// `checkPackage`: whether `package` is `uid`'s.
    fn package_mode(&self, uid: i32, package: Option<&str>) -> Result<i32, Exception> {
        let (state, _) = self.host.packages()?;
        let owns = package.and_then(|p| state.packages.get(p)).is_some_and(|ps| ps.app_id == uid.rem_euclid(100_000));
        Ok(if owns { MODE_ALLOWED } else { MODE_ERRORED })
    }
}

impl Service for AppOps {
    fn descriptor(&self) -> &str {
        appops::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            appops::CHECK_PACKAGE => {
                let args = appops::CheckPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.package_mode(args.uid, args.package_name.as_deref()) {
                    Ok(mode) => appops::write_check_package_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::CHECK_OPERATION_FOR_DEVICE => {
                let args = appops::CheckOperationForDevice::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.mode(args.code, args.uid, args.package_name.as_deref()) {
                    Ok(mode) => appops::write_check_operation_for_device_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::NOTE_OPERATION => {
                let args = appops::NoteOperation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mode = match self.package_mode(args.uid, args.package_name.as_deref()) {
                    Ok(MODE_ALLOWED) => match self.mode(args.code, args.uid, args.package_name.as_deref()) {
                        Ok(mode) => mode,
                        Err(e) => return reply(Err(e)),
                    },
                    Ok(_) => MODE_ERRORED,
                    Err(e) => return reply(Err(e)),
                };
                // `SyncNotedAppOp`: which nullable fields follow, the mode,
                // the op, the attribution tag, the package.
                p.write_no_exception();
                p.write_i32(1);
                let tag = args.attribution_tag.as_deref();
                let package = args.package_name.as_deref();
                p.write_i32(if tag.is_some() { 0x4 } else { 0 } | if package.is_some() { 0x8 } else { 0 });
                p.write_i32(mode);
                p.write_i32(args.code);
                if let Some(tag) = tag {
                    p.write_string16(Some(tag));
                }
                if let Some(package) = package {
                    p.write_string16(Some(package));
                }
            }
            appops::SHOULD_COLLECT_NOTES => {
                let args = appops::ShouldCollectNotes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.op(args.op_code) {
                    return reply(Err(e));
                }
                appops::write_should_collect_notes_reply(&mut p, false);
            }
            appops::START_WATCHING_MODE_WITH_FLAGS => {
                let args = appops::StartWatchingModeWithFlags::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(callback) = args.callback {
                    self.watchers.lock().unwrap().push((args.op, callback));
                }
                appops::write_start_watching_mode_with_flags_reply(&mut p);
            }
            code => return reply(Err(self.host.unimplemented("appops", code))),
        }
        Ok(p)
    }
}
