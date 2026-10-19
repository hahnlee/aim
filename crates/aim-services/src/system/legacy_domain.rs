//! Concrete legacy IPackageManager domain routes and retained verifier lifetime.
use super::*;
use crate::package::{
    domain_verification::{
        collector::{self, Kind},
        enforcer::Operation,
    },
    legacy_domain_mutation::{Current, Owner, Worker},
};
use aim_binder_host::parcel::EX_ILLEGAL_STATE;

impl System {
    pub fn install_legacy_domain_verification(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        persistence: Arc<Mutex<crate::package::owner::Store>>,
        legacy_proxy_enabled: bool,
    ) -> Result<Worker> {
        self.check_package_bootstrap(bridge)?;
        let weak = Arc::downgrade(self);
        let lookup_bridge = bridge.clone();
        let lookup = Arc::new(move |name: &str| {
            let system = weak.upgrade().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "legacy verifier system detached")
            })?;
            let capture = system.capture_package_queries_for(&lookup_bridge)?;
            let domains = capture
                .domains()
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "domain owner unavailable"))?;
            let package = domains
                .owner()
                .persisted()
                .active
                .into_iter()
                .find(|package| package.name == name);
            let Some(package) = package else {
                return Ok(None);
            };
            let loaded = capture.scan().owner().loaded_packages();
            let code = loaded.get(name);
            Ok(Some(Current {
                identifier: package.id,
                domains: package
                    .domains
                    .into_iter()
                    .filter_map(|(name, _)| name)
                    .collect(),
                code_exists: code.is_some(),
            }))
        });
        let weak = Arc::downgrade(self);
        let set_bridge = bridge.clone();
        let commit_store = persistence.clone();
        let set = Arc::new(move |uid: i32, id: &str, names: &[String], status: i32| {
            let system = weak.upgrade().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "legacy verifier system detached")
            })?;
            loop {
                let capture = system.capture_package_queries_for(&set_bridge)?;
                let domains = capture
                    .domains()
                    .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "domain owner unavailable"))?;
                system.authorize_package_domain(
                    &set_bridge,
                    &capture,
                    0,
                    uid,
                    Operation::Verifier,
                )?;
                let Some(package) = domains.owner().package_by_id(id) else {
                    return Ok(1);
                };
                let code = capture
                    .scan()
                    .owner()
                    .loaded_packages()
                    .get(&package.name)
                    .cloned()
                    .ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "legacy verification code owner absent")
                    })?;
                let policy = domains
                    .collector_policy(&code.package.package_name)
                    .map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
                let declared = collector::collect(&code.package, policy, Kind::ValidAutoVerify);
                if names.iter().any(|name| !declared.contains(name)) {
                    return Ok(2);
                }
                let mut owner = domains.owner().clone();
                let mut hosts = names.iter().cloned().collect();
                let status = owner
                    .set_verifier_status(id, Some(&code.package), policy, &mut hosts, status)
                    .map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
                if status != 0 {
                    return Ok(status);
                }
                let update = capture
                    .prepare_domain_update(owner)
                    .map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
                match system.commit_package_domains_if_current(
                    &set_bridge,
                    update,
                    &mut commit_store.lock().unwrap(),
                ) {
                    Ok(Some(_)) => return Ok(0),
                    Ok(None) => continue,
                    Err(error) => {
                        return Err(Exception::new(
                            EX_ILLEGAL_STATE,
                            format!("legacy domain commit: {error:?}"),
                        ));
                    }
                }
            }
        });
        let (owner, worker) = Owner::start(legacy_proxy_enabled, lookup, set);
        let installed = {
            let mut state = self.package_bootstrap.lock().unwrap();
            match state
                .current
                .as_mut()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            {
                Some(current) if current.legacy_domain_verification.is_none() => {
                    current.legacy_domain_verification =
                        Some(Arc::new(crate::package::legacy_domain_mutation::Runtime {
                            owner,
                            store: persistence,
                        }));
                    true
                }
                _ => false,
            }
        };
        if !installed {
            drop(worker);
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "legacy verifier bootstrap changed",
            ));
        }
        Ok(worker)
    }
    pub fn verify_native_intent_filter(
        self: &Arc<Self>,
        pid: i32,
        uid: i32,
        id: i32,
        code: i32,
        failed: Vec<Option<String>>,
    ) -> Result<()> {
        let permitted = self.check_permission(
            "android.permission.INTENT_FILTER_VERIFICATION_AGENT",
            pid,
            uid,
        )?;
        let owner = self
            .package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .and_then(|current| current.legacy_domain_verification.clone())
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "native legacy verifier unavailable")
            })?;
        owner.owner.verify(
            id,
            code,
            failed.into_iter().flatten().collect(),
            uid,
            permitted,
        )
    }
    pub fn register_native_legacy_domain_request(
        &self,
        id: i32,
        package: String,
        identifier: String,
    ) -> Result<()> {
        let owner = self
            .package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .and_then(|current| current.legacy_domain_verification.clone())
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "native legacy verifier unavailable")
            })?;
        owner.owner.register(id, package, identifier);
        Ok(())
    }
    pub fn update_native_intent_verification_status(
        self: &Arc<Self>,
        pid: i32,
        uid: i32,
        package: Option<&str>,
        user: i32,
        status: i32,
    ) -> Result<bool> {
        let persistence = self
            .package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .and_then(|current| {
                current
                    .legacy_domain_verification
                    .as_ref()
                    .map(|runtime| runtime.store.clone())
            })
            .ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "native legacy domain persistence unavailable",
                )
            })?;
        let bridge = self.package_bootstrap()?;
        let Some(name) = package else {
            return Ok(false);
        };
        loop {
            let capture = self.capture_package_queries_for(&bridge)?;
            let allowed = self.authorize_package_domain(
                &bridge,
                &capture,
                pid,
                uid,
                Operation::LegacySelect(name, user),
            )?;
            if !allowed {
                return Ok(false);
            }
            let domains = capture.domains().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "native domain owner unavailable")
            })?;
            let mut owner = domains.owner().clone();
            owner.set_legacy_user_state(package, user, status);
            let update = capture
                .prepare_domain_update(owner)
                .map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
            match self.commit_package_domains_if_current(
                &bridge,
                update,
                &mut persistence.lock().unwrap(),
            ) {
                Ok(Some(_)) => return Ok(true),
                Ok(None) => continue,
                Err(error) => {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("legacy user state commit: {error:?}"),
                    ));
                }
            }
        }
    }
}
