//! The live UserManagerInternal leaf used by PMS cross-user shell checks.
use aim_binder_host::parcel::Exception;
pub struct Owner {
    profile_parent: Option<Box<dyn Fn(i32) -> Result<Option<i32>, Exception> + Send + Sync>>,
    parent_app_linking: Option<Box<dyn Fn(i32) -> Result<bool, Exception> + Send + Sync>>,
    role_holders: Option<Box<dyn Fn(&str, i32) -> Result<Vec<String>, Exception> + Send + Sync>>,
    shell_debugging_restricted: Box<dyn Fn(i32) -> Result<bool, Exception> + Send + Sync>,
}
impl Owner {
    pub fn new(shell_debugging_restricted: Box<dyn Fn(i32) -> Result<bool, Exception> + Send + Sync>) -> Self { Self { shell_debugging_restricted, role_holders: None, profile_parent: None, parent_app_linking: None } }
    pub fn with_cross_profile(mut self, parent: Box<dyn Fn(i32) -> Result<Option<i32>, Exception> + Send + Sync>,
        app_linking: Box<dyn Fn(i32) -> Result<bool, Exception> + Send + Sync>) -> Self {
        self.profile_parent = Some(parent); self.parent_app_linking = Some(app_linking); self
    }
    pub fn profile_parent(&self, user: i32) -> Result<Option<i32>, Exception> {
        self.profile_parent.as_ref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION, "actual UserManager profile parent owner unavailable"))?(user)
    }
    pub fn parent_app_linking(&self, user: i32) -> Result<bool, Exception> {
        self.parent_app_linking.as_ref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION, "actual UserManager parent app-linking restriction owner unavailable"))?(user)
    }
    pub fn with_role_holders(mut self, roles: Box<dyn Fn(&str, i32) -> Result<Vec<String>, Exception> + Send + Sync>) -> Self {
        self.role_holders = Some(roles); self
    }
    pub fn role_holders(&self, role: &str, user: i32) -> Result<Vec<String>, Exception> {
        self.role_holders.as_ref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION, "original live RoleManager owner unavailable"))?(role, user)
    }
    pub fn shell_debugging_restricted(&self, user: i32) -> Result<bool, Exception> { (self.shell_debugging_restricted)(user) }
}
impl std::fmt::Debug for Owner { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("PackageUserPolicy").finish_non_exhaustive() } }
impl PartialEq for Owner { fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) } }
