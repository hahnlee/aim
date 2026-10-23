//! `IPackageInstallerFiles` for the app host: the native installer's file
//! owner, which system_server's `PackageInstallerFileBridge` serves, to the
//! system uid only. Checksums of guest files are computed as the app host
//! computes `requestPackageChecksums` (`package_checksums`); a checksum
//! signature's verification and a hard-linked stage are not implemented
//! (#2159).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_BAD_PARCELABLE, EX_NULL_POINTER, Exception, Parcel, Reader, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::dev_aim_server_ipackageinstallerfiles as files;

use super::package_checksums::{self, Refused};
use super::{AppHost, reply};
use crate::package::installer::checksums::TrustedInstallers;
use crate::package::query::checksums::Prepared;

const SYSTEM_UID: u32 = 1000;

pub struct InstallerFiles {
    host: Arc<AppHost>,
}

impl InstallerFiles {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerFiles { host }
    }

    /// `dispatch`: the trusted installers decoded and the listener taken,
    /// then the checksums computed off the caller's thread.
    fn dispatch(
        &self,
        files: Vec<(Option<String>, String)>,
        installer: Option<String>,
        optional: i32,
        required: i32,
        trusted: Option<Vec<u8>>,
        listener: Option<aim_binder_host::parcel::Binder>,
    ) -> Result<(), Exception> {
        let trusted = match trusted {
            None => TrustedInstallers(None),
            Some(bytes) => {
                let mut r = Reader::new(&bytes, &[]);
                let values = aim_service_aidl::read_array_list(&mut r)
                    .map_err(|_| bad_parcelable("invalid certificate list record"))?
                    .ok_or_else(|| Exception::illegal_argument("null certificate list record"))?;
                enforce_no_data_avail(&r)?;
                TrustedInstallers(Some(values))
            }
        };
        if listener.is_none() {
            return Err(Exception::new(EX_NULL_POINTER, "onChecksumsReadyListener"));
        }
        let prepared = Prepared {
            files,
            installer,
            optional,
            required,
            trusted,
            listener,
        };
        match package_checksums::request(&self.host, prepared) {
            Ok(()) => Ok(()),
            Err(Refused::NotModelled(what)) => {
                Err(self.host.unimplemented_named("installer files", what))
            }
            Err(Refused::Exception(e)) => Err(e),
        }
    }
}

fn bad_parcelable(message: &str) -> Exception {
    Exception::new(EX_BAD_PARCELABLE, message)
}

/// `Parcel.enforceNoDataAvail`.
fn enforce_no_data_avail(r: &Reader<'_>) -> Result<(), Exception> {
    match r.remaining() {
        0 => Ok(()),
        n => Err(bad_parcelable(&format!(
            "Parcel data not fully consumed, unread size: {n}"
        ))),
    }
}

/// `new ParcelableException(new FileNotFoundException(path))`.
fn file_not_found(path: &str) -> Exception {
    let class = "java.io.FileNotFoundException";
    let mut p = Parcel::new();
    p.write_string16(Some("android.os.ParcelableException"));
    p.write_string16(Some(class));
    p.write_string16(Some(path));
    Exception::parcelable(Some(&format!("{class}: {path}")), &p).expect("a message has no objects")
}

impl Service for InstallerFiles {
    fn descriptor(&self) -> &str {
        files::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `enforceNativeCaller`.
        let system = call.sender_euid == SYSTEM_UID;
        let caller = || {
            if system {
                Ok(())
            } else {
                Err(Exception::security(
                    "native installer file owner requires system UID",
                ))
            }
        };
        match call.code {
            files::VERIFY_CHECKSUMS => {
                files::VerifyChecksums::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| {
                    Err(self
                        .host
                        .unimplemented_named("installer files", "verifyChecksums (#2159)"))
                }))
            }
            files::REQUEST_CHECKSUMS => {
                let a = files::RequestChecksums::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| {
                    let path = a
                        .guest_path
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "guestPath"))?;
                    // `File.getAbsolutePath` against system_server's `/`.
                    let path = if path.starts_with('/') {
                        path
                    } else {
                        format!("/{path}")
                    };
                    let (root, data) = (self.host.root()?, self.host.data()?);
                    let exists = super::packages::host_path(root, data, &path)
                        .is_ok_and(|host| host.exists());
                    if !exists {
                        return Err(file_not_found(&path));
                    }
                    self.dispatch(
                        vec![(None, path)],
                        a.installer_package,
                        a.optional,
                        a.required,
                        a.trusted_certificate_parcel,
                        a.listener,
                    )?;
                    let mut p = Parcel::new();
                    files::write_request_checksums_reply(&mut p);
                    Ok(p)
                }))
            }
            files::REQUEST_PACKAGE_CHECKSUMS => {
                let a =
                    files::RequestPackageChecksums::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| {
                    let bytes = a
                        .file_parcel
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "fileParcel"))?;
                    let mut r = Reader::new(&bytes, &[]);
                    let invalid = || Exception::illegal_argument("invalid checksum file list");
                    let count = r.read_i32().map_err(|_| invalid())?;
                    if count < 1 || count as usize > bytes.len() / 8 {
                        return Err(invalid());
                    }
                    let mut list = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        let split = r.read_string16().map_err(|_| invalid())?;
                        let path = r
                            .read_string16()
                            .map_err(|_| invalid())?
                            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "checksum file"))?;
                        list.push((split, path));
                    }
                    enforce_no_data_avail(&r)?;
                    self.dispatch(
                        list,
                        a.installer_package,
                        a.optional,
                        a.required,
                        a.trusted_certificate_parcel,
                        a.listener,
                    )?;
                    let mut p = Parcel::new();
                    files::write_request_package_checksums_reply(&mut p);
                    Ok(p)
                }))
            }
            files::STAGE_VIA_HARD_LINK => {
                files::StageViaHardLink::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| {
                    Err(self
                        .host
                        .unimplemented_named("installer files", "stageViaHardLink (#2159)"))
                }))
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{
        EX_ILLEGAL_ARGUMENT, EX_SECURITY, EX_UNSUPPORTED_OPERATION, Parcel,
    };

    fn node(euid: u32) -> aim_binder_host::local::LocalService {
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid,
                security_context: None,
            },
        );
        let host = AppHost::new(process.clone());
        let aim_binder_host::parcel::Binder::Local(ptr) =
            process.add_service(Arc::new(InstallerFiles::new(host)))
        else {
            unreachable!()
        };
        process.local_service(ptr).unwrap()
    }

    fn code(node: &aim_binder_host::local::LocalService, code: u32, p: &Parcel) -> i32 {
        let reply = node.transact(code, p, false).unwrap();
        reply.reader().read_exception().unwrap().unwrap_err().code
    }

    fn package_checksums(file_parcel: Vec<u8>) -> Parcel {
        let mut p = Parcel::new();
        files::RequestPackageChecksums {
            file_parcel: Some(file_parcel),
            installer_package: None,
            optional: 0,
            required: 0,
            trusted_certificate_parcel: None,
            listener: None,
        }
        .write(&mut p);
        p
    }

    /// Only the system uid is served.
    #[test]
    fn other_callers_are_refused() {
        let mut p = Parcel::new();
        files::StageViaHardLink {
            source: Some("/data/local/tmp/a.apk".into()),
            stage_directory: Some("/data/app/vmdl1.tmp".into()),
        }
        .write(&mut p);
        assert_eq!(
            code(&node(10000), files::STAGE_VIA_HARD_LINK, &p),
            EX_SECURITY
        );
        assert_eq!(
            code(&node(SYSTEM_UID), files::STAGE_VIA_HARD_LINK, &p),
            EX_UNSUPPORTED_OPERATION
        );
    }

    /// A file list is decoded as `requestPackageChecksums` decodes it: a
    /// count of at least one, each file's path present, nothing left over.
    #[test]
    fn package_checksum_file_lists_are_checked() {
        let node = node(SYSTEM_UID);
        let list = |count: i32, files: &[(Option<&str>, Option<&str>)], extra: bool| {
            let mut body = Parcel::new();
            body.write_i32(count);
            for (split, path) in files {
                body.write_string16(*split);
                body.write_string16(*path);
            }
            if extra {
                body.write_i32(0);
            }
            package_checksums(body.data().to_vec())
        };
        let request = |p: Parcel| code(&node, files::REQUEST_PACKAGE_CHECKSUMS, &p);
        assert_eq!(request(list(0, &[], false)), EX_ILLEGAL_ARGUMENT);
        assert_eq!(request(list(1, &[(None, None)], false)), EX_NULL_POINTER);
        let base = [(None, Some("/data/app/a/base.apk"))];
        assert_eq!(request(list(1, &base, true)), EX_BAD_PARCELABLE);
        // A valid list without a listener.
        assert_eq!(request(list(1, &base, false)), EX_NULL_POINTER);
    }
}
