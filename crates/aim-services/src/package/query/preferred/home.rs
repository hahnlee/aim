//! Live Role HOME and native preferred fallback, including its in-memory
//! bookkeeping. Default-home highest-priority fallback is an internal API only.
use super::*;
use crate::package::{
    component_resolver::ResolveInfo,
    preferred::registry::{ActionError, Handle},
    resolve::{QueryError, Resolution, ResolutionError, preferred_owner::HomePlan},
};
pub(super) fn plan(
    query: &Query<'_>,
    resolution: &Resolution,
    user: i32,
    owner: &Handle,
) -> Result<HomePlan, ActionError> {
    let snapshot = owner.snapshot(user)?;
    let empty = Preferred::default();
    let preferred = snapshot
        .as_ref()
        .map_or(&empty, |snapshot| snapshot.state.as_ref());
    let role = owner.actions.default_home(user)?;
    let plan = resolution
        .plan_home(user, query.calling_uid, preferred, role.as_deref())
        .map_err(|error| match error {
            ResolutionError::Original(error) => ActionError::Exception(error),
            ResolutionError::NotModelled(error) => ActionError::Unavailable(error),
            ResolutionError::UriMatching(error) => error
                .binder_exception()
                .map(ActionError::Exception)
                .unwrap_or(ActionError::Unavailable(NotModelled(
                    "home preferred URI bounds owner unavailable",
                ))),
        })?;
    if !plan.selection.edits.is_empty() {
        let snapshot = snapshot
            .as_ref()
            .ok_or(ActionError::Unavailable(NotModelled(
                "home preferred mutation source unavailable",
            )))?;
        owner
            .actions
            .commit_home_selection(user, snapshot.generation, &plan.selection)?;
    }
    Ok(plan)
}
pub fn activities(
    query: &Query<'_>,
    resolution: &Resolution,
    data: &mut Reader<'_>,
    owner: &Handle,
) -> Result<Parcel, QueryError> {
    args(pm::GetHomeActivities::read(data))?;
    if data.remaining() != 0 {
        return Err(NotModelled("trailing home activities arguments").into());
    }
    match plan(query, resolution, user_id(query.calling_uid), owner) {
        Ok(home) => {
            let chosen = home.chosen.as_ref().map(Component);
            let candidates: Vec<_> = home.candidates.into_iter().map(Some).collect();
            Ok(reply(|parcel| {
                pm::write_get_home_activities_reply(
                    parcel,
                    &pm::GetHomeActivitiesReply {
                        result: chosen,
                        out_home_candidates: Some(candidates),
                    },
                )
            }))
        }
        Err(ActionError::Exception(error)) => Ok(reply(|parcel| parcel.write_exception(&error))),
        Err(ActionError::Unavailable(error)) => Err(error.into()),
    }
}

pub fn default_home(query: &Query<'_>, user: i32) -> Result<Option<ComponentName>, NotModelled> {
    let owner = query
        .state
        .system
        .preferred_owner
        .as_deref()
        .ok_or(NotModelled("native default-home owner unavailable"))?;
    let resolution = Resolution::from_native_query(query)
        .map_err(|_| NotModelled("native default-home component owner unavailable"))?;
    plan(query, &resolution, user, owner)
        .map(|home| home.default_home())
        .map_err(|error| match error {
            ActionError::Unavailable(error) => error,
            ActionError::Exception(_) => {
                NotModelled("native default-home lookup threw an exception")
            }
        })
}
