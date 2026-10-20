//! `auth`: AuthService for the app host (`android-16.0.0_r1`), which
//! SystemServer starts with BiometricService whatever biometric features
//! the device declares, and which `BiometricManager` is made from. The
//! Mac's Touch ID is not a sensor of the app host (#1896), so
//! BiometricService has no sensors: `canAuthenticate` answers as
//! `PreAuthInfo` does with none (biometrics: no hardware; a device
//! credential: the Mac's lock, as `trust` answers `isDeviceSecure`),
//! nothing is enrolled and there is no authenticator id. Prompts, sensors'
//! properties and the rest are not implemented (#1896).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::android_hardware_biometrics_iauthservice as auth;

use super::{AppHost, PER_USER_RANGE, reply};

const USE_FINGERPRINT: &str = "android.permission.USE_FINGERPRINT";
const USE_BIOMETRIC: &str = "android.permission.USE_BIOMETRIC";
const USE_BIOMETRIC_INTERNAL: &str = "android.permission.USE_BIOMETRIC_INTERNAL";
const MANDATORY_BIOMETRICS: &str = "android.hardware.biometrics.mandatory_biometrics";

/// `BiometricManager.Authenticators`.
const BIOMETRIC_STRONG: i32 = 0x000F;
const BIOMETRIC_WEAK: i32 = 0x00FF;
const BIOMETRIC_MIN_STRENGTH: i32 = 0x7FFF;
const DEVICE_CREDENTIAL: i32 = 1 << 15;
const IDENTITY_CHECK: i32 = 1 << 16;

/// `BiometricManager`'s results of `canAuthenticate`.
const BIOMETRIC_SUCCESS: i32 = 0;
const BIOMETRIC_ERROR_NONE_ENROLLED: i32 = 11;
const BIOMETRIC_ERROR_NO_HARDWARE: i32 = 12;
const BIOMETRIC_ERROR_IDENTITY_CHECK_NOT_ACTIVE: i32 = 20;

/// `PreAuthInfo`'s `AuthenticatorStatus` with no sensors, as
/// `Utils.authenticatorStatusToBiometricConstant` and
/// `biometricConstantsToBiometricManager` turn it into a result.
#[derive(Debug, PartialEq)]
enum Status {
    Ok,
    BiometricNoHardware,
    CredentialNotEnrolled,
    MandatoryBiometricUnavailable,
}

impl Status {
    fn result(&self) -> i32 {
        match self {
            Status::Ok => BIOMETRIC_SUCCESS,
            Status::BiometricNoHardware => BIOMETRIC_ERROR_NO_HARDWARE,
            Status::CredentialNotEnrolled => BIOMETRIC_ERROR_NONE_ENROLLED,
            Status::MandatoryBiometricUnavailable => BIOMETRIC_ERROR_IDENTITY_CHECK_NOT_ACTIVE,
        }
    }
}

/// `Utils.isValidAuthenticatorConfig`. Mandatory biometrics needs
/// SET_BIOMETRIC_DIALOG_ADVANCED of the caller or the system, and
/// BiometricService asks as the system (AuthService cleared the
/// identity).
fn valid_config(authenticators: i32, mandatory_biometrics: bool) -> bool {
    if authenticators == 0 {
        return true;
    }
    let allowed = DEVICE_CREDENTIAL
        | BIOMETRIC_MIN_STRENGTH
        | if mandatory_biometrics {
            IDENTITY_CHECK
        } else {
            0
        };
    if authenticators & !allowed != 0 {
        return false;
    }
    let biometric = authenticators & BIOMETRIC_MIN_STRENGTH;
    (biometric == 0 && authenticators & DEVICE_CREDENTIAL != 0)
        || biometric == BIOMETRIC_STRONG
        || biometric == BIOMETRIC_WEAK
        || authenticators & IDENTITY_CHECK != 0
}

/// `PreAuthInfo.create(…).getInternalStatus()` with no sensors: nothing is
/// enrolled, so mandatory biometrics never drops the credential fallback
/// (`dropCredentialFallback`).
fn status(authenticators: i32, credential_available: bool, mandatory_biometrics: bool) -> Status {
    let biometric = authenticators & BIOMETRIC_WEAK != 0;
    let credential = authenticators & DEVICE_CREDENTIAL != 0;
    match (biometric, credential) {
        (true, true) | (false, true) if credential_available => Status::Ok,
        (true, true) | (false, true) => Status::CredentialNotEnrolled,
        (true, false) => Status::BiometricNoHardware,
        (false, false) if mandatory_biometrics && authenticators == IDENTITY_CHECK => {
            Status::MandatoryBiometricUnavailable
        }
        (false, false) => Status::BiometricNoHardware,
    }
}

pub struct AuthService {
    host: Arc<AppHost>,
}

impl AuthService {
    pub fn new(host: Arc<AppHost>) -> Self {
        AuthService { host }
    }

    /// `enforceCallingOrSelfPermission(permission, message)`.
    fn enforce(&self, uid: i32, permission: &str, message: &str) -> Result<(), Exception> {
        if self.host.check_permission(Some(permission), uid)? != 0 {
            return Err(Exception::security(format!(
                "{message}: Neither user {uid} nor current process has {permission}."
            )));
        }
        Ok(())
    }

    /// `checkPermission`: USE_FINGERPRINT, else USE_BIOMETRIC.
    fn check_permission(&self, uid: i32) -> Result<(), Exception> {
        if self.host.check_permission(Some(USE_FINGERPRINT), uid)? == 0 {
            return Ok(());
        }
        self.enforce(uid, USE_BIOMETRIC, "Must have USE_BIOMETRIC permission")
    }

    fn check_internal_permission(&self, uid: i32) -> Result<(), Exception> {
        self.enforce(
            uid,
            USE_BIOMETRIC_INTERNAL,
            "Must have USE_BIOMETRIC_INTERNAL permission",
        )
    }

    fn mandatory_biometrics(&self) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state
            .system
            .flags
            .iter()
            .any(|(n, on)| n == MANDATORY_BIOMETRICS && *on))
    }

    /// `canAuthenticate`, then BiometricService's.
    fn can_authenticate(&self, uid: i32, user: i32, authenticators: i32) -> Result<i32, Exception> {
        if user != uid / PER_USER_RANGE as i32 {
            self.check_internal_permission(uid)?;
        } else {
            self.check_permission(uid)?;
        }
        let mandatory_biometrics = self.mandatory_biometrics()?;
        if !valid_config(authenticators, mandatory_biometrics) {
            return Err(Exception::security("Invalid authenticator configuration"));
        }
        // `ITrustManager.isDeviceSecure`, as `trust` answers it.
        let credential_available = aim_host_lock::secure().map_err(|code| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("auth: the Mac's lock: LocalAuthentication error {code}"),
            )
        })?;
        Ok(status(authenticators, credential_available, mandatory_biometrics).result())
    }
}

impl Service for AuthService {
    fn descriptor(&self) -> &str {
        auth::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            auth::CAN_AUTHENTICATE => {
                let a = auth::CanAuthenticate::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.can_authenticate(uid, a.user_id, a.authenticators)
                    .map(|r| auth::write_can_authenticate_reply(&mut p, r))
            }
            // `getAuthenticatorIds`: another user's with
            // USE_BIOMETRIC_INTERNAL; no strong sensor has an enrollment,
            // so there are none.
            auth::GET_AUTHENTICATOR_IDS => {
                let a = auth::GetAuthenticatorIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let checked = if a.user_id != uid / PER_USER_RANGE as i32 {
                    self.enforce(
                        uid,
                        USE_BIOMETRIC_INTERNAL,
                        &format!("Must have {USE_BIOMETRIC_INTERNAL} permission."),
                    )
                } else {
                    Ok(())
                };
                checked.map(|()| auth::write_get_authenticator_ids_reply(&mut p, &Some(Vec::new())))
            }
            // `hasEnrolledBiometrics`: no sensor has an enrollment.
            auth::HAS_ENROLLED_BIOMETRICS => {
                auth::HasEnrolledBiometrics::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check_internal_permission(uid)
                    .map(|()| auth::write_has_enrolled_biometrics_reply(&mut p, false))
            }
            code => {
                let name = auth::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("auth", &format!("{name} (#1896)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no sensors, biometrics have no hardware and a device credential
    /// is the Mac's lock.
    #[test]
    fn no_sensors_answer_as_pre_auth_info() {
        let r = |a, secure| status(a, secure, true).result();
        assert_eq!(r(BIOMETRIC_WEAK, true), BIOMETRIC_ERROR_NO_HARDWARE);
        assert_eq!(r(BIOMETRIC_STRONG, false), BIOMETRIC_ERROR_NO_HARDWARE);
        assert_eq!(
            r(BIOMETRIC_WEAK | DEVICE_CREDENTIAL, true),
            BIOMETRIC_SUCCESS
        );
        assert_eq!(
            r(BIOMETRIC_WEAK | DEVICE_CREDENTIAL, false),
            BIOMETRIC_ERROR_NONE_ENROLLED
        );
        assert_eq!(r(DEVICE_CREDENTIAL, true), BIOMETRIC_SUCCESS);
        assert_eq!(
            r(IDENTITY_CHECK, true),
            BIOMETRIC_ERROR_IDENTITY_CHECK_NOT_ACTIVE
        );
        assert_eq!(
            status(IDENTITY_CHECK, true, false).result(),
            BIOMETRIC_ERROR_NO_HARDWARE
        );
        assert_eq!(r(0, true), BIOMETRIC_ERROR_NO_HARDWARE);
    }

    #[test]
    fn authenticator_configurations_are_checked_as_utils_checks_them() {
        assert!(valid_config(0, false));
        assert!(valid_config(BIOMETRIC_WEAK, false));
        assert!(valid_config(BIOMETRIC_STRONG | DEVICE_CREDENTIAL, false));
        assert!(valid_config(DEVICE_CREDENTIAL, false));
        assert!(!valid_config(0x0001, false));
        assert!(!valid_config(IDENTITY_CHECK, false));
        assert!(valid_config(IDENTITY_CHECK, true));
        assert!(!valid_config(1 << 20, true));
    }
}
