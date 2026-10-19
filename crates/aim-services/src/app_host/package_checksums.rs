//! `requestPackageChecksums` for the app host (`requestChecksumsInternal`
//! at `android-16.0.0_r1`): once the package's checks pass
//! (`package::query::checksums`), the trusted installers are decoded and
//! the files' checksums computed on a thread of their own
//! (`package::apk_checksums`), then handed to the caller's
//! `IOnChecksumsReadyListener`. A file's fs-verity digest is the one the
//! guest's kernel measures: a proof in the data volume's store, which
//! guest-init keeps beside the mapped `/data`. Installer-provided digests
//! (`.digests` and `.signature` beside an APK, bound to the installer's
//! certificates) are not read (#1626).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;

use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::{ParcelValue, WriteParcelable};

use super::AppHost;
use crate::package::apk_checksums::{self, ApkFile};
use crate::package::query::checksums::Prepared;
use crate::package::sign;

const LISTENER_DESCRIPTOR: &str = "android.content.pm.IOnChecksumsReadyListener";
/// `IOnChecksumsReadyListener.onChecksumsReady`.
const ON_CHECKSUMS_READY: u32 = 1;
/// `PackageManagerServiceUtils.SHELL_PACKAGE_NAME`.
const SHELL_PACKAGE: &str = "com.android.shell";

/// Why a request is not taken: not modelled, or the original's exception.
pub(super) enum Refused {
    NotModelled(&'static str),
    Exception(Exception),
}

impl From<Exception> for Refused {
    fn from(e: Exception) -> Refused {
        Refused::Exception(e)
    }
}

/// `requestChecksumsInternal` from its trusted installers on.
pub(super) fn request(host: &Arc<AppHost>, prepared: Prepared) -> Result<(), Refused> {
    // `decodeCertificates`.
    let trusted: Option<usize> = match &prepared.trusted.0 {
        None => None,
        Some(values) => {
            let certificates = values
                .iter()
                .all(|v| matches!(v, ParcelValue::Bytes(Some(b)) if sign::is_certificate(b)));
            if !certificates {
                return Err(Refused::NotModelled(
                    "trusted installers that are not X.509 certificates (#1626)",
                ));
            }
            Some(values.len())
        }
    };
    let Some(Binder::Handle(listener)) = prepared.listener else {
        return Err(Refused::NotModelled(
            "a checksums listener of the app host's (#1626)",
        ));
    };
    let (root, data) = (host.root()?, host.data()?);
    let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("checksums: {e}"));
    // `getInstallerChecksums` reads the digests unless the installer is
    // adb (`isInstalledByAdb`) or nobody is trusted.
    let adb = prepared
        .installer
        .as_deref()
        .is_none_or(|i| i == SHELL_PACKAGE);
    let store = data.parent().map(|volume| volume.join("fs-verity"));
    let mut files = Vec::new();
    for (split, path) in &prepared.files {
        let host_file = super::packages::host_path(root, data, path).map_err(state)?;
        if !adb
            && trusted != Some(0)
            && let Some(base) = path.strip_suffix(".apk")
            && super::packages::host_path(root, data, &format!("{base}.digests"))
                .map_err(state)?
                .exists()
        {
            return Err(Refused::NotModelled("installer-provided checksums (#1626)"));
        }
        let fs_verity_digest = store
            .as_deref()
            .and_then(|store| fs_verity_digest(store, &host_file));
        files.push(ApkFile {
            split: split.clone(),
            path: host_file,
            fs_verity_digest,
        });
    }
    let build = host.inputs()?.packages.signing_build();
    let listener = host.process.strong(listener);
    let (optional, required) = (prepared.optional, prepared.required);
    std::thread::Builder::new()
        .name("checksums".into())
        .spawn(move || {
            let found = apk_checksums::checksums(&files, optional, required, &build);
            let mut p = Parcel::new();
            p.write_interface_token(LISTENER_DESCRIPTOR);
            p.write_i32(found.len() as i32);
            for checksum in &found {
                p.write_i32(1);
                checksum.write_to(&mut p);
            }
            if let Err(e) = listener.transact(ON_CHECKSUMS_READY, &p, true) {
                eprintln!("app host: ApkChecksums: onChecksumsReady: {e}");
            }
        })
        .map_err(|e| state(e.to_string()))?;
    Ok(())
}

/// `VerityUtils.hasFsverity` and `getFsverityDigest` as the guest's kernel
/// answers them: a proof in the `store` for a file on its volume, measured
/// as SHA-256 (`FS_IOC_MEASURE_VERITY` with a 32-byte digest).
fn fs_verity_digest(store: &Path, file: &Path) -> Option<Vec<u8>> {
    let log = |e: String| {
        eprintln!(
            "app host: ApkChecksums: fs-verity of {}: {e}",
            file.display()
        )
    };
    let open = std::fs::File::open(file).ok()?;
    let same_volume = match (open.metadata(), std::fs::metadata(store)) {
        (Ok(f), Ok(s)) => f.dev() == s.dev(),
        _ => false,
    };
    if !same_volume {
        return None;
    }
    let identity = match aim_storage::fsverity::Identity::from_fd(open.as_fd()) {
        Ok(identity) => identity,
        Err(e) => {
            log(e.to_string());
            return None;
        }
    };
    let proof = match aim_storage::fsverity::lookup_in(store, identity) {
        Ok(proof) => proof?,
        Err(e) => {
            log(format!("{e:?}"));
            return None;
        }
    };
    match proof.measure(32) {
        Ok((1, digest)) => Some(digest),
        Ok((algorithm, _)) => {
            log(format!("unsupported algorithm {algorithm}"));
            None
        }
        Err(e) => {
            log(format!("{e:?}"));
            None
        }
    }
}
