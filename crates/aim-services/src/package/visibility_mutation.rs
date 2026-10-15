//! PMS makeUidVisible/grantImplicitAccess, android-16.0.0_r1.
//! Copyright AOSP, Apache-2.0. Interaction grants are live AppsFilter state.
use super::{
    apps_filter::{self, ImplicitAccess, NotModelled},
    info,
    query::Query,
};
use aim_binder_host::parcel::Exception;

pub struct MakeUidVisible {
    pub recipient: i32,
    pub visible: i32,
}
impl MakeUidVisible {
    pub fn decide(
        &self,
        query: &Query<'_>,
    ) -> Result<Result<Option<ImplicitAccess>, Exception>, NotModelled> {
        let uid = query.calling_uid;
        if apps_filter::app_id(uid) != 0
            && apps_filter::app_id(uid) != 1000
            && !query.uid_has_permission(uid, "android.permission.MAKE_UID_VISIBLE")?
        {
            return Ok(Err(Exception::security(
                "makeUidVisible requires android.permission.MAKE_UID_VISIBLE",
            )));
        }
        // These are three separate checks in PMS, including the recipient's
        // own authority to see the visible user's packages.
        for (caller, user) in [
            (uid, apps_filter::user_id(self.recipient)),
            (uid, apps_filter::user_id(self.visible)),
            (self.recipient, apps_filter::user_id(self.visible)),
        ] {
            if let Err(error) =
                query.internal_enforce_cross_user(caller, user, false, false, "makeUidVisible")?
            {
                return Ok(Err(error));
            }
        }
        self.grant(query)
    }
    pub(crate) fn grant(
        &self,
        query: &Query<'_>,
    ) -> Result<Result<Option<ImplicitAccess>, Exception>, NotModelled> {
        let system = Query {
            state: query.state,
            filter: query.filter,
            calling_uid: 1000,
        };
        let package = |uid| -> Result<Option<&super::model::PackageState>, NotModelled> {
            Ok(system
                .packages_for_uid(uid)?
                .unwrap_or_default()
                .iter()
                .filter_map(|name| name.as_deref())
                .filter_map(|name| query.state.packages.get(name))
                .find(|state| state.pkg.is_some()))
        };
        let Some(visible) = package(self.visible)? else {
            return Ok(Ok(None));
        };
        if package(self.recipient)?.is_none() {
            return Ok(Ok(None));
        }
        // The original asks whether visiblePackage is instant in recipient's
        // user; this interaction is indirect and cannot grant instant access.
        if info::user_state(visible, apps_filter::user_id(self.recipient)).instant_app {
            return Ok(Ok(None));
        }
        let mut grants = query.state.system.implicit_access.clone();
        Ok(Ok(grants
            .grant(self.recipient, self.visible, false)
            .then_some(grants)))
    }
}

pub struct MakeProviderVisible {
    pub recipient: i32,
    pub authority: Option<String>,
}
impl MakeProviderVisible {
    pub fn decide(
        &self,
        resolution: &super::resolve::Resolution,
        query: &Query<'_>,
    ) -> Result<Result<Option<ImplicitAccess>, Exception>, super::resolve::ResolutionError> {
        let uid = query.calling_uid;
        let contacts = resolution.resolve_content_provider(
            "com.android.contacts",
            0,
            apps_filter::user_id(uid),
            uid,
        )?;
        if contacts.is_none_or(|provider| {
            apps_filter::app_id(provider.info.application_info.uid) != apps_filter::app_id(uid)
        }) {
            return Ok(Err(Exception::security(format!(
                "{uid} is not allow to call grantImplicitAccess"
            ))));
        }
        let Some(authority) = self.authority.as_deref() else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "visible authority is null",
            )));
        };
        let Some(provider) = resolution.resolve_content_provider(
            authority,
            0,
            apps_filter::user_id(self.recipient),
            uid,
        )?
        else {
            return Ok(Ok(None));
        };
        MakeUidVisible {
            recipient: self.recipient,
            visible: provider.info.application_info.uid,
        }
        .grant(query)
        .map_err(super::resolve::ResolutionError::NotModelled)
    }
}
