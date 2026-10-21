//! `file_integrity`: FileIntegrityService for the app host
//! (`android-16.0.0_r1`): fs-verity set up on an app's file through
//! installd, as StorageManagerService's `Installer` asks it.
//! `createAuthToken` hands installd the caller's writable fd of the file
//! for a token; `setupFsverity` (SETUP_FSVERITY, the caller's own
//! package) has installd enable fs-verity with it and answers its errno.
//! A failure of installd's call is the RemoteException the original
//! throws, which leaves its binder uncaught.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Received, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_NULL_POINTER, Exception, Parcel, Reader, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::ReadParcelable;
use aim_service_aidl::android_os_iinstalld as installd;
use aim_service_aidl::android_security_ifileintegrityservice as fi;

use super::{AppHost, reply};
use crate::package::app_metadata::Descriptor;

const SETUP_FSVERITY: &str = "android.permission.SETUP_FSVERITY";
const PER_USER_RANGE: i32 = 100_000;
const SYSTEM_UID: i32 = 1000;

/// A `ParcelFileDescriptor` as it is parcelled: its fd (the call's).
struct ReceivedFd(u32);

impl ReadParcelable for ReceivedFd {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        let comm = r.read_i32()? != 0;
        let fd = r.read_fd()?;
        if comm {
            r.read_fd()?;
        }
        Ok(ReceivedFd(fd))
    }
}

/// What a call answers: its reply, or the transaction failed as an
/// uncaught exception fails it.
enum Answer {
    Reply(Parcel),
    Uncaught(String),
}

pub struct FileIntegrity {
    host: Arc<AppHost>,
}

impl FileIntegrity {
    pub fn new(host: Arc<AppHost>) -> Self {
        FileIntegrity { host }
    }

    fn installd(&self) -> Result<Strong, String> {
        self.host.installd()
    }

    /// `checkCallerPackageName`: the package is the caller's
    /// (`PackageManagerInternal.getPackageUid` in its user).
    fn check_caller_package(&self, uid: i32, package: &str) -> Result<(), Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let package_uid = query
            .package_uid_internal(package, 0, uid / PER_USER_RANGE, SYSTEM_UID)
            .map_err(|e| {
                Exception::new(
                    aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                    format!("file_integrity: {}", e.0),
                )
            })?;
        if package_uid != uid {
            return Err(Exception::security(format!(
                "Calling uid {uid} does not own package {package}"
            )));
        }
        Ok(())
    }

    /// installd's reply to `code`, or the `InstallerException` it makes.
    fn call_installd(&self, code: u32, data: &Parcel, name: &str) -> Result<Received, String> {
        let installd = self.installd()?;
        let reply = installd
            .transact(code, data, false)
            .map_err(|s| format!("{name}: status {s}"))?;
        match reply.reader().read_exception() {
            Ok(Ok(())) => Ok(reply),
            Ok(Err(e)) => Err(format!("{name}: {}", e.message)),
            Err(s) => Err(format!("{name}: status {s}")),
        }
    }

    /// `createAuthToken`: installd's token for the caller's fd.
    fn create_auth_token(&self, uid: i32, fd: Option<ReceivedFd>) -> Result<Answer, Exception> {
        let file = fd
            .and_then(|fd| self.host.process.file(fd.0))
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        let mut data = Parcel::new();
        installd::CreateFsveritySetupAuthToken {
            auth_fd: Some(Descriptor(file)),
            uid,
        }
        .write(&mut data);
        const NAME: &str = "createFsveritySetupAuthToken";
        let answer = (|| {
            let received =
                self.call_installd(installd::CREATE_FSVERITY_SETUP_AUTH_TOKEN, &data, NAME)?;
            let token =
                installd::read_create_fsverity_setup_auth_token_reply(&mut received.reader())
                    .map_err(|s| format!("{NAME}: status {s}"))?
                    .map_err(|e| format!("{NAME}: {}", e.message))?;
            let mut p = Parcel::new();
            fi::write_create_auth_token_reply(&mut p, token);
            // The token is installd's reply's until this one is sent.
            p.keep_alive(Arc::new(received));
            Ok(p)
        })();
        Ok(answer.map_or_else(Answer::Uncaught, Answer::Reply))
    }

    /// `setupFsverity`: installd's `enableFsverity`, its errno answered.
    fn setup_fsverity(&self, uid: i32, a: fi::SetupFsverity) -> Result<Answer, Exception> {
        if self.host.check_permission(Some(SETUP_FSVERITY), uid)? != 0 {
            return Err(Exception::security(format!(
                "Access denied, requires: {SETUP_FSVERITY}"
            )));
        }
        let (Some(token), Some(path), Some(package)) = (a.auth_token, a.file_path, a.package_name)
        else {
            return Err(Exception::new(EX_NULL_POINTER, ""));
        };
        self.check_caller_package(uid, &package)?;
        let mut data = Parcel::new();
        installd::EnableFsverity {
            auth_token: Some(token),
            file_path: Some(path),
            package_name: Some(package),
        }
        .write(&mut data);
        const NAME: &str = "enableFsverity";
        let answer = (|| {
            let received = self.call_installd(installd::ENABLE_FSVERITY, &data, NAME)?;
            let errno = installd::read_enable_fsverity_reply(&mut received.reader())
                .map_err(|s| format!("{NAME}: status {s}"))?
                .map_err(|e| format!("{NAME}: {}", e.message))?;
            let mut p = Parcel::new();
            fi::write_setup_fsverity_reply(&mut p, errno);
            Ok(p)
        })();
        Ok(answer.map_or_else(Answer::Uncaught, Answer::Reply))
    }
}

impl Service for FileIntegrity {
    fn descriptor(&self) -> &str {
        fi::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let answer = match call.code {
            fi::CREATE_AUTH_TOKEN => {
                let a = fi::CreateAuthToken::<ReceivedFd>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.create_auth_token(uid, a.auth_fd)
            }
            fi::SETUP_FSVERITY => {
                let a = fi::SetupFsverity::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.setup_fsverity(uid, a)
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        match answer {
            Ok(Answer::Reply(p)) => Ok(p),
            // `RemoteException(IOException(InstallerException))`, which
            // `writeException` cannot parcel: JavaBBinder reports it and
            // fails the transaction.
            Ok(Answer::Uncaught(e)) => {
                eprintln!(
                    "app host: file_integrity: *** Uncaught remote exception! \
                     android.os.RemoteException: java.io.IOException: {e}"
                );
                Err(UNKNOWN_TRANSACTION)
            }
            Err(e) => reply(Err(e)),
        }
    }
}
