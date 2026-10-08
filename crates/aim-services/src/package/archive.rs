//! Native archive queries over Settings ArchiveState and original rendering leaves.
//! android-16.0.0_r1, Copyright AOSP, Apache License 2.0.
use super::{info::{self, SigningInfo}, instant::Bitmap, intent::ComponentName, model::PackageState};
use aim_binder_host::parcel::{Exception, Parcel, Reader};
use aim_service_aidl::{ReadParcelable, WriteParcelable};
use std::{fs, path::{Path, PathBuf}};
pub struct UserHandle(pub i32);
impl ReadParcelable for UserHandle { fn read_from(reader: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> { Ok(Self(reader.read_i32()?)) } }
#[derive(Clone)]
pub struct Activity { pub title: String, pub component: ComponentName, pub icon: Option<Vec<u8>>, pub monochrome: Option<Vec<u8>> }
impl WriteParcelable for Activity {
    fn write_to(&self, parcel: &mut Parcel) {
        let start = parcel.position(); parcel.write_i32(0);
        parcel.write_string16(Some(&self.title)); parcel.write_i32(1);
        parcel.write_string16(Some(&self.component.package)); parcel.write_string16(Some(&self.component.class));
        bytes(parcel, self.icon.as_deref()); bytes(parcel, self.monochrome.as_deref());
        parcel.set_i32_at(start, (parcel.position() - start) as i32);
    }
}
pub struct Package { pub name: String, pub signing: Option<SigningInfo>, pub version: i64,
    pub target_sdk: i32, pub device_storage: bool, pub legacy_storage: bool, pub fragile: bool, pub activities: Vec<Activity> }
impl WriteParcelable for Package {
    fn write_to(&self, parcel: &mut Parcel) {
        let start = parcel.position(); parcel.write_i32(0);
        parcel.write_string16(Some(&self.name)); parcel.write_i32(1);
        info::write_signing_details(parcel, self.signing.as_ref());
        parcel.write_i32(self.version as i32); parcel.write_i32((self.version >> 32) as i32); parcel.write_i32(self.target_sdk);
        for flag in [self.device_storage, self.legacy_storage, self.fragile] { parcel.write_string16(Some(if flag { "true" } else { "false" })); }
        parcel.write_i32(self.activities.len() as i32);
        for activity in &self.activities { parcel.write_i32(1); activity.write_to(parcel); }
        parcel.set_i32_at(start, (parcel.position() - start) as i32);
    }
}
fn bytes(parcel: &mut Parcel, bytes: Option<&[u8]>) {
    if let Some(bytes) = bytes { parcel.write_i32(bytes.len() as i32); parcel.write_raw(bytes, &[]); }
    else { parcel.write_i32(-1); }
}
pub type Launcher = Box<dyn Fn(&str, i32) -> Result<Vec<Activity>, Exception> + Send + Sync>;
pub type Op = Box<dyn Fn(i32, &str) -> Result<i32, Exception> + Send + Sync>;
pub type Overlay = Box<dyn Fn(Bitmap) -> Result<Bitmap, Exception> + Send + Sync>;
pub struct Owner { data: PathBuf, density: i32, launcher: Launcher, overlay_op: Op, opt_out_op: Op, overlay: Overlay }
impl Owner {
    pub fn clear_icons(&self, package: &str, user: i32) -> Result<(), Exception> {
        if user < 0 || package.is_empty() || package.contains(['/', '\\', '\0'])
            || matches!(package, "." | "..") { return Err(Exception::illegal_argument("invalid archive icon owner")); }
        let path = self.data.join("system_ce").join(user.to_string()).join("package_archiver").join(package);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => { eprintln!("Archive icon directory lookup failed: {error}"); Ok(()) },
            Ok(metadata) if metadata.file_type().is_symlink() => Err(Exception::illegal_argument("archive icon directory is a symlink")),
            Ok(_) => {
                if let Err(error) = fs::remove_dir_all(path) { eprintln!("Archive icon removal failed: {error}"); }
                Ok(())
            }
        }
    }
    pub fn new(data: &Path, density: i32, launcher: Launcher, overlay_op: Op, opt_out_op: Op, overlay: Overlay) -> Self {
        Self { data: data.to_owned(), density, launcher, overlay_op, opt_out_op, overlay }
    }
    fn mapped(&self, guest: &str) -> Result<PathBuf, Exception> {
        let path = Path::new(guest); let suffix = path.strip_prefix("/data").map_err(|_| Exception::illegal_argument("archive icon is outside Android data"))?;
        if suffix.components().any(|component| matches!(component, std::path::Component::ParentDir)) { return Err(Exception::illegal_argument("archive icon contains parent traversal")); }
        Ok(self.data.join(suffix))
    }
    fn icon_bytes(&self, path: Option<&str>) -> Result<Option<Vec<u8>>, Exception> {
        path.map(|path| fs::read(self.mapped(path)?).map_err(|error| Exception::illegal_argument(error.to_string()))).transpose()
    }
    pub fn activities(&self, package: &PackageState, user: i32) -> Result<Vec<Activity>, Exception> {
        if let Some(archive) = super::info::user_state(package, user).archive_state {
            if archive.activities.is_empty() { return Err(Exception::illegal_argument("Package does not have a main activity")); }
            let mut activities = Vec::new();
            for activity in archive.activities {
                let (name, class) = activity.original_component_name.split_once('/').ok_or_else(|| Exception::illegal_argument("Package does not have a main activity"))?;
                activities.push(Activity { title: activity.title, component: ComponentName { package: name.into(), class: class.into() },
                    icon: self.icon_bytes(activity.icon_path.as_deref())?, monochrome: self.icon_bytes(activity.monochrome_icon_path.as_deref())? });
            }
            Ok(activities)
        } else { (self.launcher)(&package.name, user) }
    }
    pub fn launcher(&self, package: &str, user: i32) -> Result<Vec<Activity>, Exception> { (self.launcher)(package, user) }
    pub fn opted_out(&self, uid: i32, package: &str) -> Result<bool, Exception> { (self.opt_out_op)(uid, package).map(|mode| mode == 1) }
    pub fn overlay_enabled(&self, caller: i32, calling_package: &str) -> Result<bool, Exception> {
        (self.overlay_op)(caller, calling_package).map(|mode| mode == 0)
    }
    pub fn icon(&self, package: &PackageState, user: i32, overlay: bool) -> Result<Option<Bitmap>, Exception> {
        let state = package.users.get(&user).filter(|state| !state.installed && state.archive_state.is_some())
            .or_else(|| package.users.values().find(|state| !state.installed && state.archive_state.is_some()));
        let Some(activity) = state.and_then(|state| state.archive_state.as_ref()).and_then(|archive| archive.activities.first()) else { return Ok(None); };
        let Some(bytes) = self.icon_bytes(activity.icon_path.as_deref())? else { return Ok(None); };
        let icon = Bitmap::decode(&bytes, self.density).map_err(Exception::illegal_argument)?;
        match icon { Some(icon) if overlay => (self.overlay)(icon).map(Some), other => Ok(other) }
    }
}
impl std::fmt::Debug for Owner { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("PackageArchiveOwner").finish_non_exhaustive() } }
impl PartialEq for Owner { fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) } }
