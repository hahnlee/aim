//! Original UriGrantsManagerInternal and UserManagerInternal leaves used by
//! native provider resolution. The provider itself comes from native scan state.
use super::info::ProviderInfo;
use aim_binder_host::parcel::Exception;
pub type AuthorityGrants = Box<dyn Fn(i32, &ProviderInfo, i32, bool) -> Result<bool, Exception> + Send + Sync>;
pub type CloneRedirection = Box<dyn Fn(&str, i32, i32) -> Result<bool, Exception> + Send + Sync>;
pub struct Owner { grants: AuthorityGrants, clone: CloneRedirection }
impl Owner {
    pub fn new(grants: AuthorityGrants, clone: CloneRedirection) -> Self { Self { grants, clone } }
    pub fn check(&self, uid: i32, provider: &ProviderInfo, user: i32) -> Result<bool, Exception> { (self.grants)(uid, provider, user, true) }
    pub fn clone_redirected(&self, authority: &str, uid: i32, user: i32) -> Result<bool, Exception> { (self.clone)(authority, uid, user) }
}
impl std::fmt::Debug for Owner { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("ProviderUriAccess").finish_non_exhaustive() } }
impl PartialEq for Owner { fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) } }
