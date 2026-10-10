//! Permission group definitions remain PermissionManagerService-owned. The
//! native PM endpoint consumes its typed original Parcelable result.
use aim_binder_host::parcel::{Exception, Parcel};
use aim_service_aidl::WriteParcelable;
pub struct Group { body: Parcel }
impl Group {
    pub fn from_owned_parcel(body: Parcel) -> Self { Self { body } }
}
impl WriteParcelable for Group {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_raw_files(self.body.data(), self.body.objects(), self.body.files());
    }
}
pub struct Owner { query: Box<dyn Fn(Option<&str>, i32) -> Result<Option<Group>, Exception> + Send + Sync> }
impl Owner {
    pub fn new(query: Box<dyn Fn(Option<&str>, i32) -> Result<Option<Group>, Exception> + Send + Sync>) -> Self { Self { query } }
    pub fn group(&self, name: Option<&str>, flags: i32) -> Result<Option<Group>, Exception> { (self.query)(name, flags) }
}
impl std::fmt::Debug for Owner { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("PermissionGroups").finish_non_exhaustive() } }
impl PartialEq for Owner { fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) } }
