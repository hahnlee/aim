//! AppIdPermissionPolicy signature and static protection grants, Android 16 r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;
use crate::package::{
    pkg::Permission as Declaration, sign::History, system_config::PermissionAllowlist,
};
use sha2::{Digest, Sha256};

#[derive(Default)]
pub(super) struct Policy {
    pub allowlist: PermissionAllowlist,
    pub debuggable: bool,
    pub control_privapp_permissions: String,
    pub splits: Vec<SplitPermission>,
    pub leanback: bool,
}
impl Policy {
    fn namespace(ps: &PackageState) -> &str {
        if ps.is.vendor || ps.is.odm {
            "vendor"
        } else if ps.is.product {
            "product"
        } else if ps.is.system_ext {
            "system_ext"
        } else {
            "system"
        }
    }
    fn allowlisted(&self, ps: &PackageState, permission: &str, signature: bool) -> Option<bool> {
        let kind = if signature {
            "signature-permissions"
        } else {
            "privapp-permissions"
        };
        let namespace = Self::namespace(ps);
        if namespace != "system" {
            return self.allowlist.get(namespace, kind, &ps.name, permission);
        }
        if signature {
            for ns in ["apex", "product", "vendor", "system_ext", "system"] {
                if let Some(value) = self.allowlist.get(ns, kind, &ps.name, permission) {
                    return Some(value);
                }
            }
            None
        } else {
            ps.apex_module_name
                .as_ref()
                .and_then(|module| {
                    self.allowlist
                        .get(&format!("apex/{module}"), kind, &ps.name, permission)
                })
                .or_else(|| self.allowlist.get("system", kind, &ps.name, permission))
        }
    }
    fn privileged_allowed(
        &self,
        ps: &PackageState,
        permission: &Permission,
    ) -> Result<bool, Exception> {
        if self.control_privapp_permissions == "disable"
            || ps.name == "android"
            || !(ps.is.system && ps.is.privileged)
            || permission.package != "android"
        {
            return Ok(true);
        }
        if let Some(allowed) = self.allowlisted(ps, &permission.name, false) {
            return Ok(allowed);
        }
        if ps.is.updated_system_app {
            return Ok(true);
        }
        if self.control_privapp_permissions == "enforce" {
            if !ps.is.apk_in_updated_apex {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    format!(
                        "Signature|privileged permission {} for {} is not in the privileged permission allowlist",
                        permission.name, ps.name
                    ),
                ));
            }
            return Ok(false);
        }
        Ok(true)
    }
    fn signature(&self, state: &State, ps: &PackageState, permission: &Permission) -> bool {
        let Some(signing) = ps.signatures.as_ref() else {
            return false;
        };
        let signing = History::saved(signing);
        let source = state
            .packages
            .get(&permission.package)
            .and_then(|p| p.signatures.as_ref());
        let platform = state
            .packages
            .get("android")
            .and_then(|p| p.signatures.as_ref());
        let common = source.is_some_and(|source| {
            History::saved(source).has_common_signer_with_capability(&signing, 4)
        }) || platform.is_some_and(|platform| {
            signing.has_ancestor_or_self(&History::saved(platform))
                || History::saved(platform).check_capability(&signing, 4)
        });
        if !common {
            return false;
        }
        let enforce = state.system.flags.iter().any(|(name, on)| {
            name == "android.permission.flags.signature_permission_allowlist_enabled" && *on
        });
        if !enforce || permission.package != "android" {
            return true;
        }
        let factory_requested = ps.is.system
            && (!ps.is.updated_system_app
                || state
                    .disabled_system_packages
                    .get(&ps.name)
                    .and_then(|p| p.pkg.as_ref())
                    .is_some_and(|p| p.requested_permissions.contains(&permission.name)));
        factory_requested
            || self.allowlisted(ps, &permission.name, true) == Some(true)
            || self.debuggable
    }
    fn privileged_or_oem(
        &self,
        ps: &PackageState,
        permission: &Permission,
    ) -> Result<bool, Exception> {
        if permission.protection_level & 0x10 != 0 {
            return Ok(ps.is.privileged
                && (!(ps.is.vendor || ps.is.odm) || permission.protection_level & 0x8000 != 0));
        }
        if permission.protection_level & 0x4000 != 0 && ps.is.oem {
            return self.allowlist.get("oem", "oem-permissions", &ps.name, &permission.name)
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("OEM permission {} requested by {} must be explicitly declared granted or not", permission.name, ps.name)));
        }
        Ok(false)
    }
    fn protection(
        &self,
        state: &State,
        ps: &PackageState,
        permission: &Permission,
        declaration: Option<&Declaration>,
    ) -> Result<bool, Exception> {
        let flags = permission.protection_level;
        if flags & (0x10 | 0x4000) != 0 && ps.is.system {
            let factory = if ps.is.updated_system_app {
                state.disabled_system_packages.get(&ps.name).filter(|p| {
                    p.pkg
                        .as_ref()
                        .is_some_and(|p| p.requested_permissions.contains(&permission.name))
                })
            } else {
                Some(ps)
            };
            if let Some(factory) = factory {
                if self.privileged_or_oem(factory, permission)? {
                    return Ok(true);
                }
            }
        }
        if flags & 0x80 != 0 && ps.target_sdk_version < M {
            return Ok(true);
        }
        if flags & 0x400 != 0 && ps.is.system {
            return Ok(true);
        }
        if flags & 0x8000000 != 0 {
            if let (Some(signing), Some(certs)) = (
                &ps.signatures,
                declaration.and_then(|d| d.known_certs.as_ref()),
            ) {
                let matches = |cert: &Vec<u8>| {
                    let digest = Sha256::digest(cert)
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<String>();
                    certs.iter().flatten().any(|known| *known == digest)
                };
                let trusted = if signing.signatures.len() > 1 {
                    signing.signatures.iter().all(matches)
                } else {
                    signing.signatures.iter().any(matches)
                        || signing
                            .past_signatures
                            .iter()
                            .flatten()
                            .any(|(cert, _)| matches(cert))
                };
                if trusted {
                    return Ok(true);
                }
            }
        }
        for (flag, kinds) in [
            (0x100, &[2, 7][..]),
            (0x200, &[4][..]),
            (0x800, &[1][..]),
            (0x10000, &[6][..]),
            (0x80000, &[10][..]),
            (0x100000, &[11][..]),
            (0x200000, &[12][..]),
            (0x800000, &[15][..]),
            (0x2000000, &[17][..]),
        ] {
            if flags & flag == 0 {
                continue;
            }
            for kind in kinds {
                let selected = state
                    .system
                    .roles
                    .as_ref()
                    .and_then(|owner| owner.permission_known_package(*kind, &ps.name))
                    .ok_or_else(|| {
                        unsupported("known-package protection grants before role selection")
                    })?;
                // AppIdPermissionPolicy.kt:1641-1725: KnownPackages already
                // owns selection; there is no additional isSystem condition.
                if selected {
                    return Ok(true);
                }
            }
        }
        // PermissionInfo.java:259; AppIdPermissionPolicy.kt:1727-1730.
        if flags & 0x400000 != 0 && ps.apex_module_name.is_some() {
            return Ok(true);
        }
        Ok(false)
    }
    pub(super) fn grant(
        &self,
        state: &State,
        requesters: &[&PackageState],
        permission: &Permission,
        declaration: Option<&Declaration>,
    ) -> Result<bool, Exception> {
        let mut may_grant_privileged = permission.protection_level & 0x10 == 0;
        let mut granted = false;
        for ps in requesters {
            if permission.protection_level & 0x10 != 0 {
                may_grant_privileged |= self.privileged_allowed(ps, permission)?;
            }
            // AppIdPermissionPolicy.kt:1036-1040 (android-16.0.0_r1):
            // internal|role is not eligible for a signature grant.
            let by_signature = permission.protection_level & PROTECTION_MASK_BASE
                == PROTECTION_SIGNATURE
                && self.signature(state, ps, permission);
            let by_flags = self.protection(state, ps, permission, declaration)?;
            granted |= by_signature || by_flags;
        }
        Ok(may_grant_privileged && granted)
    }
}
