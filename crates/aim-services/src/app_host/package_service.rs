//! Process-lifetime compatibility fronts for the app host's package services.
//! Every call enters the native implementation with its original identity,
//! parcel cursor and flags, including handles retained before construction.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm, android_content_pm_ipackagemanagernative as native,
};

use super::AppHost;

pub struct PackageService {
    host: Arc<AppHost>,
    native: bool,
}

impl PackageService {
    pub fn new(host: Arc<AppHost>) -> Self {
        Self {
            host,
            native: false,
        }
    }

    pub fn native(host: Arc<AppHost>) -> Self {
        Self { host, native: true }
    }
}

impl Service for PackageService {
    fn descriptor(&self) -> &str {
        if self.native {
            native::DESCRIPTOR
        } else {
            pm::DESCRIPTOR
        }
    }

    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let system = match self.host.package_system() {
            Ok(system) => system,
            Err(error) => return super::reply(Err(error)),
        };
        let (public, native) = crate::package::service::PackageQueries::from_system(system);
        if self.native {
            native.transact(call)
        } else {
            public.transact(call)
        }
    }
}
