//! IPackageManager.requestPackageChecksums: native visibility/path selection,
//! then original APK checksum algorithms with one retained Binder callback.
use super::{Query, Thrown};
use crate::package::installer::{checksums::TrustedInstallers, hardlink::Files};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader,
};
use aim_service_aidl::android_content_pm_ipackagemanager as aidl;

pub struct Request {
    package: Option<String>,
    include_splits: bool,
    optional: i32,
    required: i32,
    trusted: TrustedInstallers,
    listener: Option<Binder>,
    user: i32,
}
pub struct Prepared {
    files: Vec<(Option<String>, String)>,
    installer: Option<String>,
    optional: i32,
    required: i32,
    trusted: Option<Vec<u8>>,
    listener: Option<Binder>,
}
impl Prepared {
    pub fn send(self, owner: &Files) -> Result<(), Exception> {
        owner.request_package(
            &self.files,
            self.installer,
            self.optional,
            self.required,
            self.trusted,
            self.listener,
        )
    }
}
impl Request {
    pub fn read(r: &mut Reader<'_>) -> Result<Self, i32> {
        r.enforce_interface(aidl::DESCRIPTOR)?;
        let request = Self {
            package: r.read_string16()?,
            include_splits: r.read_bool()?,
            optional: r.read_i32()?,
            required: r.read_i32()?,
            trusted: TrustedInstallers::read(r)?,
            listener: r.read_binder()?,
            user: r.read_i32()?,
        };
        if r.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        Ok(request)
    }
    pub fn prepare(&self, query: &Query<'_>) -> Thrown<Prepared> {
        let Some(package) = self.package.as_deref() else {
            return Ok(Err(Exception::new(EX_NULL_POINTER, "packageName")));
        };
        if self.listener.is_none() {
            return Ok(Err(Exception::new(
                EX_NULL_POINTER,
                "onChecksumsReadyListener",
            )));
        }
        let info = match query.application_info(package, 0, self.user)? {
            Err(error) => return Ok(Err(error)),
            Ok(Some(info)) => info,
            Ok(None) => {
                let mut payload = Parcel::new();
                payload.write_string16(Some("android.os.ParcelableException"));
                payload.write_string16(Some(
                    "android.content.pm.PackageManager$NameNotFoundException",
                ));
                payload.write_string16(Some(package));
                let message =
                    format!("android.content.pm.PackageManager$NameNotFoundException: {package}");
                return Ok(Err(Exception::parcelable(Some(&message), &payload)
                    .map_err(|_| {
                        super::NotModelled("checksum package exception envelope")
                    })?));
            }
        };
        let source = match query.install_source_info(package, self.user)? {
            Err(error) => return Ok(Err(error)),
            Ok(source) => source,
        };
        let installer = source.and_then(|source| {
            if matches!(
                source.initiating_package_name.as_deref(),
                Some("com.android.shell") | None
            ) {
                source.installing_package_name
            } else {
                source.initiating_package_name
            }
        });
        let Some(base) = info.source_dir else {
            return Ok(Err(Exception::new(
                EX_NULL_POINTER,
                "application sourceDir",
            )));
        };
        let mut files = vec![(None, base)];
        if self.include_splits {
            if let Some(names) = info.split_names {
                let Some(paths) = info.split_source_dirs else {
                    return Ok(Err(Exception::new(EX_NULL_POINTER, "splitSourceDirs")));
                };
                if paths.len() < names.len() {
                    return Ok(Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        "split source path inventory incomplete",
                    )));
                }
                for (name, path) in names.into_iter().zip(paths) {
                    let Some(path) = path else {
                        return Ok(Err(Exception::new(EX_NULL_POINTER, "split source path")));
                    };
                    files.push((name, path));
                }
            }
        }
        Ok(Ok(Prepared {
            files,
            installer,
            optional: self.optional,
            required: self.required,
            trusted: self.trusted.certificates(),
            listener: self.listener,
        }))
    }
}
