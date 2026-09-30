//! Icons given as a resource or a URI, loaded for the Mac as SystemUI
//! loads them (`Icon.loadDrawable`): a resource from its package's APK,
//! drawn as aim-apps draws launcher icons; a `content:` URI through its
//! provider as the system uid (`IActivityManager.openContentUri`); a
//! `file:` URI from the guest's file, if an app may read it. What cannot be
//! loaded is logged, and the Mac shows the app's icon only.

use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use aim_apps::apk::{Apk, Resources};
use aim_apps::res::Value;
use aim_binder_host::parcel::{Parcel, Reader};
use aim_host_display::notify::Image;
use aim_service_aidl::{
    ReadParcelable, android_app_iactivitymanager as am, android_content_pm_ipackagemanager as pm,
};

use super::parcels::{ApplicationInfo, Icon};
use super::{Bridge, MAX_BLOB};

/// A guest path's host file, if an app other than its owner may read it.
pub type GuestFiles = dyn Fn(&str) -> Option<PathBuf> + Send + Sync;

/// The framework's resources, which an app's refer to.
const FRAMEWORK_RES: &str = "/system/framework/framework-res.apk";
/// The side of a drawn picture, in pixels.
const SIZE: usize = 256;
/// How long a provider's pipe may stay silent.
const READ_TIMEOUT_MS: i32 = 5000;

impl Bridge {
    /// `icon` as the Mac shows it; `user` is the posting app's.
    pub(super) fn image(&self, icon: &Icon, user: i32) -> Option<Image> {
        let image = match icon {
            Icon::Image(i) => return Some(i.clone()),
            Icon::Unreadable => return None,
            Icon::Resource { package, id } => self.resource(package, *id, user),
            Icon::Uri(uri) => self.uri(uri, user),
        };
        if image.is_none() {
            eprintln!("guest-init: notifications: {icon:?}: not loaded");
        }
        image
    }

    /// A `file:` or `content:` URI's image, or an `android.resource:` one
    /// given by id.
    fn uri(&self, uri: &str, user: i32) -> Option<Image> {
        let data = if let Some(path) = uri.strip_prefix("file://") {
            std::fs::read((self.files)(&percent_decode(path)?)?).ok()?
        } else if uri.starts_with("content://") {
            self.content(uri)?
        } else {
            let (package, id) = uri.strip_prefix("android.resource://")?.split_once('/')?;
            return self.resource(package, id.parse().ok()?, user);
        };
        Some(Image::Encoded(data))
    }

    /// Drawable `id` of `package`, from the APK its `ApplicationInfo`
    /// names, with the application's theme.
    fn resource(&self, package: &str, id: i32, user: i32) -> Option<Image> {
        let info = self
            .call(
                "package",
                pm::GET_APPLICATION_INFO,
                |p| {
                    pm::GetApplicationInfo {
                        package_name: Some(package.into()),
                        flags: 0,
                        user_id: user,
                    }
                    .write(p)
                },
                pm::read_get_application_info_reply::<ApplicationInfo>,
            )
            .ok()??;
        let app = Apk::open(&(self.files)(info.source_dir.as_deref()?)?).ok()?;
        let framework = self
            .framework
            .get_or_init(|| Apk::open(&(self.files)(FRAMEWORK_RES)?).ok());
        let res = Resources {
            app: &app,
            framework: framework.as_ref(),
            theme: info.theme as u32,
            night: false,
        };
        aim_apps::icon::picture(&res, &Value::Ref(id as u32), SIZE).map(Image::Encoded)
    }

    /// The content of a `content:` URI, opened by its provider for the
    /// system uid.
    fn content(&self, uri: &str) -> Option<Vec<u8>> {
        let service = self.find("activity")?;
        let mut data = Parcel::new();
        am::OpenContentUri {
            uri_string: Some(uri.into()),
        }
        .write(&mut data);
        let reply = service.transact(am::OPEN_CONTENT_URI, &data, false).ok()?;
        let Ok(Ok(Some(Fd(fd)))) = am::read_open_content_uri_reply::<Fd>(&mut reply.reader())
        else {
            return None;
        };
        let file = self.process.file(fd)?;
        read_bounded(std::fs::File::from(aim_binder_host::server::file_fd(
            &file,
        )?))
    }
}

/// A `ParcelFileDescriptor`: its fd.
struct Fd(u32);

impl ReadParcelable for Fd {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        let comm = r.read_i32()?;
        let fd = r.read_fd()?;
        if comm != 0 {
            r.read_fd()?;
        }
        Ok(Fd(fd))
    }
}

/// A file or pipe to its end, up to [`MAX_BLOB`] bytes, each read within
/// [`READ_TIMEOUT_MS`].
fn read_bounded(mut f: std::fs::File) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 64 << 10];
    loop {
        let mut pfd = libc::pollfd {
            fd: f.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd of a file we hold.
        if unsafe { libc::poll(&mut pfd, 1, READ_TIMEOUT_MS) } <= 0 {
            return None;
        }
        match f.read(&mut chunk) {
            Ok(0) => return Some(out),
            Ok(n) if out.len() + n <= MAX_BLOB => out.extend_from_slice(&chunk[..n]),
            _ => return None,
        }
    }
}

/// A URI path with its `%XX` escapes decoded.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_file_uri_path() {
        assert_eq!(
            percent_decode("/data/local/tmp/a%20b.png").as_deref(),
            Some("/data/local/tmp/a b.png")
        );
        assert_eq!(percent_decode("/x%2"), None);
    }
}
