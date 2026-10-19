//! `usagestats`: UsageStatsService for the app host (`android-16.0.0_r1`):
//! whether app standby is on and an app's own standby bucket, as
//! AppStandbyController answers them (`app_standby`). Another app's
//! bucket and the usage events and stats are not implemented (#1723).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::android_app_usage_iusagestatsmanager as us;

use super::{AppHost, reply};
use crate::package::query::Query;

const SYSTEM_UID: i32 = 1000;

pub struct UsageStats {
    host: Arc<AppHost>,
}

impl UsageStats {
    pub fn new(host: Arc<AppHost>) -> Self {
        UsageStats { host }
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host.unimplemented_named("usagestats", &format!("{what} (#1723)"))
    }

    /// `getAppStandbyBucket`: of the caller's own package, or any for a
    /// caller that may query usage (`hasQueryPermission`).
    fn app_standby_bucket(&self, uid: i32, package: &str, user: i32) -> Result<i32, Exception> {
        let user = self.host.incoming_user(uid, user, false)?;
        let (state, resolution) = self.host.packages()?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
        let package_uid = query
            .package_uid(package, 0, user)
            .map_err(|e| self.unimplemented(&format!("getAppStandbyBucket: {}", e.0)))??;
        if package_uid != uid && uid != SYSTEM_UID {
            return Err(self.unimplemented("getAppStandbyBucket of another app"));
        }
        if package_uid < 0 {
            return Err(Exception::illegal_argument(format!(
                "Cannot get standby bucket for non existent package ({package})"
            )));
        }
        self.host.app_standby_bucket(package, user)
    }
}

impl Service for UsageStats {
    fn descriptor(&self) -> &str {
        us::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        match call.code {
            us::IS_APP_STANDBY_ENABLED => {
                us::IsAppStandbyEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.host.app_idle_enabled() {
                    Ok(enabled) => us::write_is_app_standby_enabled_reply(&mut p, enabled),
                    Err(e) => return reply(Err(e)),
                }
            }
            us::GET_APP_STANDBY_BUCKET => {
                let a = us::GetAppStandbyBucket::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.app_standby_bucket(uid, a.package_name.as_deref().unwrap_or_default(), a.user_id) {
                    Ok(bucket) => us::write_get_app_standby_bucket_reply(&mut p, bucket),
                    Err(e) => return reply(Err(e)),
                }
            }
            code => {
                let name = us::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                return reply(Err(self.unimplemented(name)));
            }
        }
        Ok(p)
    }
}
