//! `package`: PackageManager's queries for the app host, answered by the
//! native model (`package::query`, `package::resolve`) from the packages
//! the app host joined (`super::packages`). What the model does not
//! answer fails with the reason, logged once.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

use super::AppHost;
use crate::package::query::Query;
use crate::shadow::{Answer, ShadowCall};

pub struct PackageService {
    host: Arc<AppHost>,
    reported: Mutex<HashSet<(u32, &'static str)>>,
    monitors: Mutex<Vec<aim_binder_host::parcel::Binder>>,
}

impl PackageService {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageService { host, reported: Mutex::new(HashSet::new()), monitors: Mutex::new(Vec::new()) }
    }

    fn unmodelled(&self, code: u32, reason: &'static str) -> Reply {
        if self.reported.lock().unwrap().insert((code, reason)) {
            eprintln!("app host: package#{code} not modelled: {reason}");
        }
        let mut reply = Parcel::new();
        reply.write_exception(&Exception::new(
            EX_UNSUPPORTED_OPERATION,
            format!("package#{code}: {reason}"),
        ));
        Ok(reply)
    }
}

impl Service for PackageService {
    fn descriptor(&self) -> &str {
        pm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let (state, resolution) = match self.host.packages() {
            Ok(packages) => packages,
            Err(e) => {
                let mut reply = Parcel::new();
                reply.write_exception(&e);
                return Ok(reply);
            }
        };
        // The packages do not change while the app host runs, so a
        // monitor is kept and never called.
        if call.code == pm::REGISTER_PACKAGE_MONITOR_CALLBACK {
            let args = pm::RegisterPackageMonitorCallback::read(&mut call.data).map_err(|_| aim_binder_host::parcel::BAD_VALUE)?;
            if let Some(callback) = args.callback {
                self.monitors.lock().unwrap().push(callback);
            }
            let mut p = Parcel::new();
            pm::write_register_package_monitor_callback_reply(&mut p);
            return Ok(p);
        }
        let mut shadow = ShadowCall {
            service: "package",
            descriptor: pm::DESCRIPTOR,
            code: call.code,
            flags: call.flags,
            sender_pid: call.sender_pid,
            sender_euid: call.sender_euid,
            seq: 0,
            sent: Instant::now(),
            dropped: 0,
            data: call.data.clone(),
        };
        if let Some(answer) = self.host.resolver.answer(&state, &mut shadow) {
            return match answer {
                Answer::Reply(reply) => Ok(reply),
                Answer::Status(status) => Err(status),
                Answer::NotModelled => self.unmodelled(call.code, "intent query"),
            };
        }
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: call.sender_euid as i32,
        };
        // The call's data from its interface token, which the query reads.
        let mut data = call.data.clone();
        match query.answer(pm::DESCRIPTOR, call.code, &mut data) {
            Ok(reply) => Ok(reply),
            Err(crate::package::apps_filter::NotModelled(reason)) => {
                self.unmodelled(call.code, reason)
            }
        }
    }
}
