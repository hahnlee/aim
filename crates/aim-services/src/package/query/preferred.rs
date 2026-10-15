//! Preferred activity endpoints consume the native owner's immutable registry
//! snapshot. No endpoint reconstructs a live owner from a displaced PMS feed.
mod home;
mod mutations;
pub use home::{activities as home_activities, default_home as default_home_for_instant};
pub use mutations::{METHODS as MUTATION_METHODS, answer as mutation};

use super::*;
use crate::package::intent_filter::IntentFilter;
use crate::package::preferred::Preferred;
use aim_service_aidl::WriteParcelable;

/// A registry generation and its actual filterIterator order. The native disk
/// owner constructs this together, retaining registration object identity.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub generation: u64,
    pub state: Arc<Preferred>,
    pub preferred_order: Vec<usize>,
}

pub trait Owner: Send + Sync {
    /// None means that the native owner has no preferred resolver for this user;
    /// an unconfigured or revoked owner must return an error.
    fn snapshot(&self, user: i32) -> Result<Option<Snapshot>, NotModelled>;
}

struct Filter<'a>(&'a IntentFilter);
impl WriteParcelable for Filter<'_> {
    fn write_to(&self, parcel: &mut Parcel) {
        self.0.write(parcel);
    }
}
struct Component<'a>(&'a ComponentName);
impl WriteParcelable for Component<'_> {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_string16(Some(&self.0.package));
        parcel.write_string16(Some(&self.0.class));
    }
}

/// getPreferredActivities returns zero even when it appends output entries in
/// the pinned helper. A named package excludes last-chosen (!always) entries.
pub fn list(
    query: &Query<'_>,
    data: &mut Reader<'_>,
    owner: &dyn Owner,
) -> Result<Parcel, NotModelled> {
    let args = args(pm::GetPreferredActivities::read(data))?;
    if data.remaining() != 0 {
        return Err(NotModelled("trailing preferred activities arguments"));
    }
    let user = user_id(query.calling_uid);
    let mut filters = Vec::new();
    let mut components = Vec::new();
    let instant = apps_filter::instant_app_package_name(query.state, query.calling_uid)?.is_some();
    let snapshot = if instant { None } else { owner.snapshot(user)? };
    if let Some(snapshot) = &snapshot {
        let entries = snapshot.state.preferred_activities(
            &snapshot.preferred_order,
            args.package_name.as_deref(),
            |_| true,
        );
        let entries =
            entries.map_err(|_| NotModelled("preferred identity iteration owner unavailable"))?;
        for activity in entries {
            if !query.filtered(
                query.state.packages.get(&activity.component.package),
                query.calling_uid,
                user,
            )? {
                filters.push(Some(Filter(&activity.filter)));
                components.push(Some(Component(&activity.component)));
            }
        }
    }
    Ok(reply(|parcel| {
        pm::write_get_preferred_activities_reply(
            parcel,
            &pm::GetPreferredActivitiesReply {
                result: 0,
                out_filters: Some(filters),
                out_activities: Some(components),
            },
        )
    }))
}

pub fn backup(
    calling_uid: i32,
    data: &mut Reader<'_>,
    owner: &dyn Owner,
) -> Result<Parcel, NotModelled> {
    let args = args(pm::GetPreferredActivityBackup::read(data))?;
    if data.remaining() != 0 {
        return Err(NotModelled("trailing preferred backup arguments"));
    }
    if calling_uid != SYSTEM_UID {
        return Ok(reply(|parcel| {
            parcel.write_exception(&Exception::security(
                "Only the system may call getPreferredActivityBackup()",
            ))
        }));
    }
    let snapshot = owner.snapshot(args.user_id)?;
    let empty = Preferred::default();
    let (state, order) = snapshot
        .as_ref()
        .map(|snapshot| (snapshot.state.as_ref(), snapshot.preferred_order.as_slice()))
        .unwrap_or((&empty, &[]));
    // PreferredActivityHelper catches serializer exceptions and returns null;
    // missing/revoked owners above remain distinct native owner errors.
    let bytes = state.preferred_backup(order).ok();
    Ok(reply(|parcel| {
        pm::write_get_preferred_activity_backup_reply(parcel, &bytes)
    }))
}

pub trait MutationOwner: Owner {
    /// Validate the captured generation and atomically persist/publish edits.
    /// This callback runs before any successful result is serialized.
    fn commit_selection(
        &self,
        user: i32,
        generation: u64,
        selection: &crate::package::preferred::Selection,
    ) -> Result<(), NotModelled>;
}

pub fn last_chosen(
    query: &Query<'_>,
    resolution: &crate::package::resolve::Resolution,
    data: &mut Reader<'_>,
    owner: &dyn MutationOwner,
) -> Result<Parcel, crate::package::resolve::QueryError> {
    use crate::package::{
        component_resolver::ResolveInfo,
        intent::Intent,
        resolve::{QueryError, ResolutionError},
    };
    let args = args(pm::GetLastChosenActivity::<Intent>::read(data))?;
    if data.remaining() != 0 {
        return Err(NotModelled("trailing last chosen arguments").into());
    }
    if apps_filter::instant_app_package_name(query.state, query.calling_uid)?.is_some() {
        return Ok(reply(|parcel| {
            pm::write_get_last_chosen_activity_reply::<ResolveInfo>(parcel, None)
        }));
    }
    let user = user_id(query.calling_uid);
    if !query.state.users.contains_key(&user) {
        return Ok(reply(|parcel| {
            pm::write_get_last_chosen_activity_reply::<ResolveInfo>(parcel, None)
        }));
    }
    // ComputerEngine.queryIntentActivitiesInternal first dereferences getPackage
    // after user existence, instant-app and same-user permission checks.
    let Some(intent) = args.intent.as_ref() else {
        return Ok(reply(|parcel| {
            parcel.write_exception(&Exception::new(
            aim_binder_host::parcel::EX_NULL_POINTER,
            "Attempt to invoke virtual method 'java.lang.String android.content.Intent.getPackage()' on a null object reference"))
        }));
    };
    let snapshot = owner.snapshot(user)?;
    let empty = Preferred::default();
    let preferred = snapshot
        .as_ref()
        .map_or(&empty, |snapshot| snapshot.state.as_ref());
    let plan = match resolution.plan_last_chosen(
        intent,
        args.resolved_type.as_deref(),
        i64::from(args.flags),
        user,
        query.calling_uid,
        preferred,
    ) {
        Ok(plan) => plan,
        Err(ResolutionError::Original(error)) => {
            return Ok(reply(|parcel| parcel.write_exception(&error)));
        }
        Err(ResolutionError::NotModelled(error)) => return Err(error.into()),
        Err(ResolutionError::UriMatching(error)) => {
            let Some(exception) = error.binder_exception() else {
                return Err(QueryError::Transport(
                    aim_binder_host::parcel::UNKNOWN_TRANSACTION,
                ));
            };
            return Ok(reply(|parcel| parcel.write_exception(&exception)));
        }
    };
    if !plan.selection.edits.is_empty() {
        let generation = snapshot
            .as_ref()
            .ok_or(NotModelled("preferred mutation source unavailable"))?
            .generation;
        owner.commit_selection(user, generation, &plan.selection)?;
    }
    Ok(reply(|parcel| {
        pm::write_get_last_chosen_activity_reply(parcel, plan.chosen.as_ref())
    }))
}
