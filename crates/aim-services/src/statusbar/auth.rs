//! `BiometricPrompt`'s device credential (decision D2): the request
//! BiometricService's AuthSession hands the status bar
//! (`showAuthenticationDialog`) is a sheet of the app's window on the Mac,
//! and what the user enters is verified as SystemUI's CredentialInteractor
//! verifies it: `ILockSettings.verifyCredential` for a Gatekeeper password
//! handle, then a hardware auth token for the request's operation
//! (`verifyGatekeeperPasswordHandle`), which the receiver gets with
//! `DISMISSED_REASON_CREDENTIAL_CONFIRMED`. A wrong credential lets the
//! user try again, after the lockout LockSettingsService imposes. There
//! are no biometric sensors: a request that does not allow the credential
//! cannot be shown.

use aim_binder_host::local::Strong;
use aim_host_display::shell::{Authenticate, Credential, Message};
use aim_service_aidl::{
    android_hardware_biometrics_ibiometricsysuireceiver as receiver, android_os_iusermanager as um,
    com_android_internal_statusbar_istatusbar as bar,
    com_android_internal_widget_ilocksettings as lock,
};

use super::StatusBar;
use super::parcels::{LockscreenCredential, PromptInfo, VerifyCredentialResponse};

/// `BiometricPrompt.DISMISSED_REASON_*`.
const DISMISSED_USER_CANCEL: i32 = 3;
const DISMISSED_ERROR: i32 = 5;
const DISMISSED_CREDENTIAL_CONFIRMED: i32 = 7;
const DISMISSED_ERROR_NO_WM: i32 = 9;
/// `LockPatternUtils.CREDENTIAL_TYPE_*`.
const CREDENTIAL_PATTERN: i32 = 1;
const CREDENTIAL_PIN: i32 = 3;
const CREDENTIAL_PASSWORD: i32 = 4;
/// `LockPatternUtils.VERIFY_FLAG_REQUEST_GK_PW_HANDLE`.
const REQUEST_GK_PW_HANDLE: i32 = 1;
/// `VerifyCredentialResponse.RESPONSE_OK`.
const RESPONSE_OK: i32 = 0;

/// The request shown.
pub struct Request {
    id: u64,
    package: String,
    receiver: Strong,
    /// The requesting user, and the one whose credential it takes.
    user: i32,
    owner: i32,
    /// The keystore operation the token is for (the Gatekeeper challenge).
    operation: i64,
    /// Its `CREDENTIAL_TYPE_*`.
    kind: i32,
}

impl Request {
    /// The message that closes its sheet.
    pub fn dismiss(&self) -> Message {
        Message::Dismiss {
            id: self.id,
            package: self.package.clone(),
        }
    }
}

impl StatusBar {
    pub(super) fn show_authentication(&self, args: bar::ShowAuthenticationDialog<PromptInfo>) {
        let Some(receiver) = self.strong(args.sysui_receiver) else {
            return;
        };
        let request = Request {
            id: args.request_id as u64,
            package: args.op_package_name.unwrap_or_default(),
            receiver,
            user: args.user_id,
            owner: args.user_id,
            operation: args.operation_id,
            kind: 0,
        };
        let credential = if args.credential_allowed {
            self.credential(args.user_id)
        } else {
            Err("only biometrics allowed".into())
        };
        let (owner, kind, credential) = match credential {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "guest-init: statusbar: {}: cannot ask: {e}",
                    request.package
                );
                return self.dismissed(&request, DISMISSED_ERROR_NO_WM, None);
            }
        };
        // The credential's own texts first, as SystemUI's credential view
        // takes them.
        let info = args.prompt_info.unwrap_or_default();
        let ask = Message::Authenticate(Authenticate {
            id: request.id,
            package: request.package.clone(),
            title: info.credential_title.or(info.title).unwrap_or_default(),
            subtitle: info
                .credential_subtitle
                .or(info.subtitle)
                .unwrap_or_default(),
            description: info
                .credential_description
                .or(info.description)
                .unwrap_or_default(),
            credential,
        });
        // A new request replaces the one shown, whose receiver hears
        // nothing more (AuthController.showDialog).
        let previous = self.state.lock().unwrap().auth.replace(Request {
            owner,
            kind,
            ..request
        });
        if let Some(p) = previous {
            self.send(&p.dismiss());
        }
        self.send(&ask);
    }

    /// The user whose credential `user` confirms, its kind, and how the
    /// Mac asks for it.
    fn credential(&self, user: i32) -> Result<(i32, i32, Credential), String> {
        let owner = self.call(
            "user",
            um::GET_CREDENTIAL_OWNER_PROFILE,
            |p| um::GetCredentialOwnerProfile { user_id: user }.write(p),
            um::read_get_credential_owner_profile_reply,
        )?;
        let kind = self.call(
            "lock_settings",
            lock::GET_CREDENTIAL_TYPE,
            |p| lock::GetCredentialType { user_id: owner }.write(p),
            lock::read_get_credential_type_reply,
        )?;
        let credential = match kind {
            CREDENTIAL_PIN => Credential::Pin,
            CREDENTIAL_PASSWORD => Credential::Password,
            CREDENTIAL_PATTERN => Credential::Pattern,
            _ => return Err(format!("user {owner} has no credential ({kind})")),
        };
        Ok((owner, kind, credential))
    }

    /// `hideAuthenticationDialog`: BiometricService ended the request.
    pub(super) fn hide_authentication(&self, request_id: i64) {
        if let Some(r) = self.take(request_id as u64) {
            self.send(&r.dismiss());
        }
    }

    /// The user entered `secret` in sheet `id`.
    pub(super) fn verify(&self, id: u64, secret: &str) {
        let (owner, kind, package) = {
            let state = self.state.lock().unwrap();
            let Some(r) = state.auth.as_ref().filter(|r| r.id == id) else {
                return;
            };
            (r.owner, r.kind, r.package.clone())
        };
        let response = self.call(
            "lock_settings",
            lock::VERIFY_CREDENTIAL,
            |p| {
                lock::VerifyCredential {
                    credential: Some(LockscreenCredential::new(kind, secret)),
                    user_id: owner,
                    flags: REQUEST_GK_PW_HANDLE,
                }
                .write(p)
            },
            lock::read_verify_credential_reply::<VerifyCredentialResponse>,
        );
        let response = match response {
            Ok(Some(r)) => r,
            Ok(None) => return self.retry(id, package, 0),
            Err(e) => {
                eprintln!("guest-init: statusbar: verifyCredential: {e}");
                return self.retry(id, package, 0);
            }
        };
        if response.code != RESPONSE_OK {
            return self.retry(id, package, response.timeout_ms.max(0) as u32);
        }
        let Some(request) = self.take(id) else {
            return;
        };
        if let Err(e) = self.call(
            "lock_settings",
            lock::USER_PRESENT,
            |p| lock::UserPresent { user_id: owner }.write(p),
            lock::read_user_present_reply,
        ) {
            eprintln!("guest-init: statusbar: userPresent: {e}");
        }
        let handle = response.password_handle;
        let token = self.call(
            "lock_settings",
            lock::VERIFY_GATEKEEPER_PASSWORD_HANDLE,
            |p| {
                lock::VerifyGatekeeperPasswordHandle {
                    gatekeeper_password_handle: handle,
                    challenge: request.operation,
                    user_id: request.user,
                }
                .write(p)
            },
            lock::read_verify_gatekeeper_password_handle_reply::<VerifyCredentialResponse>,
        );
        if let Err(e) = self.call(
            "lock_settings",
            lock::REMOVE_GATEKEEPER_PASSWORD_HANDLE,
            |p| {
                lock::RemoveGatekeeperPasswordHandle {
                    gatekeeper_password_handle: handle,
                }
                .write(p)
            },
            lock::read_remove_gatekeeper_password_handle_reply,
        ) {
            eprintln!("guest-init: statusbar: removeGatekeeperPasswordHandle: {e}");
        }
        self.send(&request.dismiss());
        match token {
            Ok(Some(VerifyCredentialResponse { hat: Some(hat), .. })) => {
                self.dismissed(&request, DISMISSED_CREDENTIAL_CONFIRMED, Some(hat))
            }
            other => {
                let why = other.err().unwrap_or_else(|| "no token".into());
                eprintln!("guest-init: statusbar: {package}: auth token: {why}");
                self.dismissed(&request, DISMISSED_ERROR, None)
            }
        }
    }

    fn retry(&self, id: u64, package: String, lockout_ms: u32) {
        self.send(&Message::Retry {
            id,
            package,
            lockout_ms,
        });
    }

    /// The user cancelled sheet `id`, or (`shown` false) it could not be
    /// shown.
    pub(super) fn cancelled(&self, id: u64, shown: bool) {
        if let Some(r) = self.take(id) {
            let reason = if shown {
                DISMISSED_USER_CANCEL
            } else {
                DISMISSED_ERROR_NO_WM
            };
            self.dismissed(&r, reason, None);
        }
    }

    fn take(&self, id: u64) -> Option<Request> {
        let mut state = self.state.lock().unwrap();
        if state.auth.as_ref().is_some_and(|r| r.id == id) {
            state.auth.take()
        } else {
            None
        }
    }

    fn dismissed(&self, r: &Request, reason: i32, attestation: Option<Vec<u8>>) {
        self.tell(&r.receiver, receiver::ON_DIALOG_DISMISSED, |p| {
            receiver::OnDialogDismissed {
                reason,
                credential_attestation: attestation,
            }
            .write(p)
        });
    }
}
