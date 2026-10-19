//! `netpolicy`: NetworkPolicyManagerService for the app host
//! (`android-16.0.0_r1`): Data Saver (restrict background) and the uids'
//! policies, as `readPolicyAL` read them from `netpolicy.xml` at the start
//! (else from `default_restrict_background_data`), and the network
//! policies, of which a device without carriers has none unless the file
//! holds some (reading those is #1711). Nothing changes them
//! while the app host runs, so the listeners apps register are never told
//! of a change. Battery saver's Data Saver policy is #1489.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::android_net_inetworkpolicymanager as np;

use super::settings::Table;
use super::{AppHost, PER_USER_RANGE, reply};

const MANAGE_NETWORK_POLICY: &str = "android.permission.MANAGE_NETWORK_POLICY";
const ACCESS_NETWORK_STATE: &str = "android.permission.ACCESS_NETWORK_STATE";
const CONNECTIVITY_INTERNAL: &str = "android.permission.CONNECTIVITY_INTERNAL";
const OBSERVE_NETWORK_POLICY: &str = "android.permission.OBSERVE_NETWORK_POLICY";
const NETWORK_STACK: &str = "android.permission.NETWORK_STACK";
const READ_PHONE_STATE: &str = "android.permission.READ_PHONE_STATE";
const READ_PRIVILEGED_PHONE_STATE: &str = "android.permission.READ_PRIVILEGED_PHONE_STATE";
/// `AppOpsManager.OP_READ_PHONE_STATE`, `MODE_ALLOWED`.
const OP_READ_PHONE_STATE: i32 = 51;
const MODE_ALLOWED: i32 = 0;
const MAINLINE_NETWORK_STACK: &str = "android.permission.MAINLINE_NETWORK_STACK";
/// `NetworkPolicyManager.POLICY_*`.
const POLICY_NONE: i32 = 0;
const POLICY_REJECT_METERED_BACKGROUND: i32 = 1;
const POLICY_ALLOW_METERED_BACKGROUND: i32 = 4;
/// `ConnectivityManager.RESTRICT_BACKGROUND_STATUS_*`.
const RESTRICT_BACKGROUND_STATUS_DISABLED: i32 = 1;
const RESTRICT_BACKGROUND_STATUS_WHITELISTED: i32 = 2;
const RESTRICT_BACKGROUND_STATUS_ENABLED: i32 = 3;
/// `VERSION_ADDED_RESTRICT_BACKGROUND`.
const VERSION_ADDED_RESTRICT_BACKGROUND: i32 = 3;
const POLICY_FILE: &str = "system/netpolicy.xml";

/// `UserHandle.isApp`.
fn is_app(uid: i32) -> bool {
    (10_000..=19_999).contains(&uid.rem_euclid(PER_USER_RANGE as i32))
}

/// What `readPolicyAL` reads: whether Data Saver was on, and each app
/// uid's policy (allowlisted uids allowed metered background, unless
/// denied it). `writePolicyAL` writes the allowlist after the policy list,
/// as a second top-level element, and the pull parser reads on past it.
#[derive(Debug, Default, PartialEq)]
struct Policy {
    restrict_background: bool,
    uid_policy: BTreeMap<i32, i32>,
    /// How many network policies (`<network-policy>`) it holds.
    network_policies: usize,
}

impl Policy {
    fn read(elements: &[aim_android_xml::Element]) -> Result<Policy, String> {
        let root = elements
            .first()
            .filter(|e| e.name == "policy-list")
            .ok_or("no <policy-list>")?;
        let version = root.int("version")?.unwrap_or(0);
        let mut p = Policy {
            restrict_background: version >= VERSION_ADDED_RESTRICT_BACKGROUND
                && root.bool("restrictBackground")?.unwrap_or(false),
            uid_policy: BTreeMap::new(),
            network_policies: 0,
        };
        let mut allowed = Vec::new();
        for e in root.children().chain(elements[1..].iter()) {
            match e.name.as_str() {
                "network-policy" => p.network_policies += 1,
                "uid-policy" => p.set(e.int("uid")?.unwrap_or(0), e.int("policy")?.unwrap_or(0)),
                // Of user 0.
                "app-policy" => p.set(e.int("appId")?.unwrap_or(0), e.int("policy")?.unwrap_or(0)),
                "whitelist" => {
                    for r in e.children().filter(|r| r.name == "restrict-background") {
                        allowed.push(r.int("uid")?.unwrap_or(0));
                    }
                }
                _ => {}
            }
        }
        for uid in allowed {
            let policy = p.uid_policy.get(&uid).copied().unwrap_or(POLICY_NONE);
            if policy & POLICY_REJECT_METERED_BACKGROUND == 0 {
                p.set(uid, policy | POLICY_ALLOW_METERED_BACKGROUND);
            }
        }
        Ok(p)
    }

    /// `setUidPolicyUncheckedUL` of an app uid.
    fn set(&mut self, uid: i32, policy: i32) {
        if !is_app(uid) {
            return;
        }
        if policy == POLICY_NONE {
            self.uid_policy.remove(&uid);
        } else {
            self.uid_policy.insert(uid, policy);
        }
    }

    /// `getRestrictBackgroundStatusInternal`.
    fn status(&self, uid: i32) -> i32 {
        let policy = self.uid_policy.get(&uid).copied().unwrap_or(POLICY_NONE);
        if policy == POLICY_REJECT_METERED_BACKGROUND {
            RESTRICT_BACKGROUND_STATUS_ENABLED
        } else if !self.restrict_background {
            RESTRICT_BACKGROUND_STATUS_DISABLED
        } else if policy & POLICY_ALLOW_METERED_BACKGROUND != 0 {
            RESTRICT_BACKGROUND_STATUS_WHITELISTED
        } else {
            RESTRICT_BACKGROUND_STATUS_ENABLED
        }
    }
}

pub struct NetworkPolicy {
    host: Arc<AppHost>,
    policy: OnceLock<Policy>,
}

impl NetworkPolicy {
    pub fn new(host: Arc<AppHost>) -> Self {
        NetworkPolicy {
            host,
            policy: OnceLock::new(),
        }
    }

    /// The policy read at the start; Data Saver as `initService` sets it.
    fn policy(&self) -> Result<&Policy, Exception> {
        if let Some(p) = self.policy.get() {
            return Ok(p);
        }
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("netpolicy: {e}"));
        let policy = match std::fs::read(self.host.data()?.join(POLICY_FILE)) {
            Ok(bytes) => Policy::read(&aim_android_xml::read_sequence(&bytes).map_err(state)?)
                .map_err(state)?,
            // `upgradeDefaultBackgroundDataUL`.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Policy {
                restrict_background: self
                    .host
                    .inputs()?
                    .settings
                    .get(Table::Global, "default_restrict_background_data")
                    .as_deref()
                    == Some("1"),
                uid_policy: BTreeMap::new(),
                network_policies: 0,
            },
            Err(e) => return Err(state(e.to_string())),
        };
        if self
            .host
            .power_service
            .get()
            .is_some_and(|p| matches!(p.power_save_mode(), Ok(true)))
        {
            return Err(self
                .host
                .unimplemented_named("netpolicy", "battery saver's Data Saver (#1489)"));
        }
        Ok(self.policy.get_or_init(|| policy))
    }

    /// `getNetworkPolicies`: MANAGE_NETWORK_POLICY, then
    /// READ_PRIVILEGED_PHONE_STATE, else READ_PHONE_STATE with its app op
    /// (none if denied); no carrier adds a policy without telephony.
    fn network_policies(&self, uid: i32, package: Option<&str>) -> Result<usize, Exception> {
        self.enforce(uid, MANAGE_NETWORK_POLICY)?;
        if self
            .host
            .check_permission(Some(READ_PRIVILEGED_PHONE_STATE), uid)?
            != 0
        {
            if self.host.check_permission(Some(READ_PHONE_STATE), uid)? != 0 {
                return Err(Exception::security(format!(
                    "NetworkPolicy: Neither user {uid} nor current process has {READ_PHONE_STATE}."
                )));
            }
            if super::appops_service::note(&self.host, OP_READ_PHONE_STATE, uid, package)?
                != MODE_ALLOWED
            {
                return Ok(0);
            }
        }
        let n = self.policy()?.network_policies;
        if n > 0 {
            return Err(self
                .host
                .unimplemented_named("netpolicy", "network policies of netpolicy.xml (#1711)"));
        }
        Ok(0)
    }

    /// `@EnforcePermission`.
    fn enforce(&self, uid: i32, permission: &str) -> Result<(), Exception> {
        if self.host.check_permission(Some(permission), uid)? != 0 {
            return Err(Exception::security(format!(
                "Access denied, requires: {permission}"
            )));
        }
        Ok(())
    }

    /// `enforceAnyPermissionOf`.
    fn enforce_any(&self, uid: i32, permissions: &[&str]) -> Result<(), Exception> {
        for p in permissions {
            if self.host.check_permission(Some(p), uid)? == 0 {
                return Ok(());
            }
        }
        Err(Exception::security(format!(
            "Requires one of the following permissions: {}.",
            permissions.join(", ")
        )))
    }
}

impl Service for NetworkPolicy {
    fn descriptor(&self) -> &str {
        np::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            np::GET_RESTRICT_BACKGROUND => {
                np::GetRestrictBackground::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce(uid, MANAGE_NETWORK_POLICY)
                    .and_then(|()| self.policy())
                    .map(|policy| {
                        np::write_get_restrict_background_reply(&mut p, policy.restrict_background)
                    })
            }
            np::GET_RESTRICT_BACKGROUND_BY_CALLER => {
                np::GetRestrictBackgroundByCaller::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce(uid, ACCESS_NETWORK_STATE)
                    .and_then(|()| self.policy())
                    .map(|policy| {
                        np::write_get_restrict_background_by_caller_reply(
                            &mut p,
                            policy.status(uid),
                        )
                    })
            }
            np::GET_RESTRICT_BACKGROUND_STATUS => {
                let a =
                    np::GetRestrictBackgroundStatus::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `PermissionUtils.enforceNetworkStackPermission`.
                self.enforce_any(uid, &[NETWORK_STACK, MAINLINE_NETWORK_STACK])
                    .and_then(|()| self.policy())
                    .map(|policy| {
                        np::write_get_restrict_background_status_reply(&mut p, policy.status(a.uid))
                    })
            }
            np::GET_NETWORK_POLICIES => {
                let a = np::GetNetworkPolicies::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.network_policies(uid, a.calling_package.as_deref())
                    .map(|_| {
                        p.write_no_exception();
                        p.write_i32(0);
                    })
            }
            np::GET_UID_POLICY => {
                let a = np::GetUidPolicy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce(uid, MANAGE_NETWORK_POLICY)
                    .and_then(|()| self.policy())
                    .map(|policy| {
                        np::write_get_uid_policy_reply(
                            &mut p,
                            policy
                                .uid_policy
                                .get(&a.uid)
                                .copied()
                                .unwrap_or(POLICY_NONE),
                        )
                    })
            }
            np::GET_UIDS_WITH_POLICY => {
                let a = np::GetUidsWithPolicy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce(uid, MANAGE_NETWORK_POLICY)
                    .and_then(|()| self.policy())
                    .map(|policy| {
                        let uids = policy
                            .uid_policy
                            .iter()
                            .filter(|&(_, &v)| {
                                (a.policy == POLICY_NONE && v == POLICY_NONE) || v & a.policy != 0
                            })
                            .map(|(&u, _)| u)
                            .collect();
                        np::write_get_uids_with_policy_reply(&mut p, &Some(uids))
                    })
            }
            np::REGISTER_LISTENER | np::UNREGISTER_LISTENER => {
                let listener = if call.code == np::REGISTER_LISTENER {
                    np::RegisterListener::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .listener
                } else {
                    np::UnregisterListener::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .listener
                };
                if listener.is_none() {
                    Err(Exception::new(EX_NULL_POINTER, "listener"))
                } else {
                    self.enforce_any(uid, &[CONNECTIVITY_INTERNAL, OBSERVE_NETWORK_POLICY])
                        .map(|()| p.write_no_exception())
                }
            }
            code => {
                let name = np::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("netpolicy", &format!("{name} (#1711)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The image's `netpolicy.xml`: Data Saver off, a few uids allowed
    /// metered background; an allowlisted uid is allowed unless denied.
    #[test]
    fn policy_is_read_as_read_policy_al_reads_it() {
        let list = br#"<policy-list version="14" restrictBackground="true">
<uid-policy uid="10103" policy="4" />
<uid-policy uid="10150" policy="1" />
<uid-policy uid="1000" policy="4" />
</policy-list>"#;
        let allowlist = br#"<whitelist>
<restrict-background uid="10150" />
<restrict-background uid="10200" />
</whitelist>"#;
        let elements = [
            aim_android_xml::read(list).unwrap(),
            aim_android_xml::read(allowlist).unwrap(),
        ];
        let p = Policy::read(&elements).unwrap();
        assert!(p.restrict_background);
        assert_eq!(
            p.uid_policy,
            BTreeMap::from([(10103, 4), (10150, 1), (10200, 4)])
        );
        assert_eq!(
            (p.status(10103), p.status(10150), p.status(10300)),
            (
                RESTRICT_BACKGROUND_STATUS_WHITELISTED,
                RESTRICT_BACKGROUND_STATUS_ENABLED,
                RESTRICT_BACKGROUND_STATUS_ENABLED
            )
        );
        // The image's file (ABX): Data Saver off, four uids allowed metered
        // background, an empty allowlist after the list.
        let image: &[u8] = &[
            0x41, 0x42, 0x58, 0x00, 0x10, 0x32, 0xff, 0xff, 0x00, 0x0b, 0x70, 0x6f, 0x6c, 0x69,
            0x63, 0x79, 0x2d, 0x6c, 0x69, 0x73, 0x74, 0x6f, 0xff, 0xff, 0x00, 0x07, 0x76, 0x65,
            0x72, 0x73, 0x69, 0x6f, 0x6e, 0x00, 0x00, 0x00, 0x0e, 0xdf, 0xff, 0xff, 0x00, 0x12,
            0x72, 0x65, 0x73, 0x74, 0x72, 0x69, 0x63, 0x74, 0x42, 0x61, 0x63, 0x6b, 0x67, 0x72,
            0x6f, 0x75, 0x6e, 0x64, 0x32, 0xff, 0xff, 0x00, 0x0a, 0x75, 0x69, 0x64, 0x2d, 0x70,
            0x6f, 0x6c, 0x69, 0x63, 0x79, 0x6f, 0xff, 0xff, 0x00, 0x03, 0x75, 0x69, 0x64, 0x00,
            0x00, 0x27, 0x77, 0x6f, 0xff, 0xff, 0x00, 0x06, 0x70, 0x6f, 0x6c, 0x69, 0x63, 0x79,
            0x00, 0x00, 0x00, 0x04, 0x33, 0x00, 0x03, 0x32, 0x00, 0x03, 0x6f, 0x00, 0x04, 0x00,
            0x00, 0x27, 0xa6, 0x6f, 0x00, 0x05, 0x00, 0x00, 0x00, 0x04, 0x33, 0x00, 0x03, 0x32,
            0x00, 0x03, 0x6f, 0x00, 0x04, 0x00, 0x00, 0x27, 0xcf, 0x6f, 0x00, 0x05, 0x00, 0x00,
            0x00, 0x04, 0x33, 0x00, 0x03, 0x32, 0x00, 0x03, 0x6f, 0x00, 0x04, 0x00, 0x00, 0x27,
            0xe4, 0x6f, 0x00, 0x05, 0x00, 0x00, 0x00, 0x04, 0x33, 0x00, 0x03, 0x33, 0x00, 0x00,
            0x32, 0xff, 0xff, 0x00, 0x09, 0x77, 0x68, 0x69, 0x74, 0x65, 0x6c, 0x69, 0x73, 0x74,
            0x33, 0x00, 0x06, 0x11,
        ];
        let p = Policy::read(&aim_android_xml::read_sequence(image).unwrap()).unwrap();
        assert!(!p.restrict_background);
        assert_eq!(
            p.uid_policy,
            BTreeMap::from([(10103, 4), (10150, 4), (10191, 4), (10212, 4)])
        );
        assert_eq!(p.network_policies, 0);
        let off = Policy {
            restrict_background: false,
            uid_policy: BTreeMap::new(),
            network_policies: 0,
        };
        assert_eq!(off.status(10300), RESTRICT_BACKGROUND_STATUS_DISABLED);
    }
}
