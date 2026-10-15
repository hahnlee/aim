//! Source-pinned preferred write endpoints. Independent UM/Role/Permission
//! owners are mandatory native constructor capabilities carried by Handle.
use super::*;
use crate::package::{
    intent::Intent,
    intent_filter::{PATTERN_ADVANCED_GLOB, PatternMatcher, Plain},
    preferred::{
        CrossProfileIntentFilter, Mutation, PersistentPreferredActivity, PreferredActivity,
        registry::{ActionError, Handle},
    },
    resolve::{QueryError, ResolutionError},
};

struct ReadFilter(IntentFilter);
impl ReadParcelable for ReadFilter {
    fn read_from(reader: &mut Reader<'_>) -> ParcelResult<Self> {
        IntentFilter::read(reader, &mut Plain).map(Self)
    }
}
type Action<T> = Result<T, ActionError>;
// Context checks use ActivityManager's live caller permission owner, including
// adopted shell identity. Captured package grants are a different contract.
fn context_permission(query: &Query<'_>, owner: &Handle, name: &str) -> Action<bool> {
    owner.actions.context_permission(query.calling_uid, name)
}
fn permission(query: &Query<'_>, owner: &Handle, name: &str) -> Action<()> {
    if context_permission(query, owner, name)? {
        return Ok(());
    }
    Err(ActionError::Exception(Exception::security(format!(
        "Neither user {} nor current process has {name}.", query.calling_uid
    ))))
}
fn preferred_permission(query: &Query<'_>, owner: &Handle) -> Action<bool> {
    const NAME: &str = "android.permission.SET_PREFERRED_APPLICATIONS";
    if context_permission(query, owner, NAME)? {
        return Ok(true);
    }
    let mut uid = query.calling_uid;
    if apps_filter::is_sdk_sandbox(uid) {
        let package = query
            .state
            .system
            .sdk_sandbox_package
            .as_ref()
            .ok_or(NotModelled(
                "SDK sandbox preferred caller owner unavailable",
            ))?
            .as_ref()
            .ok_or(NotModelled(
                "SDK sandbox preferred caller package unavailable",
            ))?;
        uid = query
            .state
            .packages
            .get(package)
            .ok_or(NotModelled(
                "SDK sandbox preferred caller setting unavailable",
            ))?
            .app_id;
    }
    let sdk = match setting(query.state, app_id(uid)) {
        Some(Setting::Package(package)) => package
            .pkg
            .as_ref()
            .map_or(10000, |code| code.target_sdk_version),
        Some(Setting::Shared(shared)) => {
            let packages: Vec<_> = if let Some(packages) = &shared.native_packages {
                packages.iter().collect()
            } else {
                shared
                    .packages
                    .iter()
                    .filter_map(|name| query.state.packages.get(name))
                    .collect()
            };
            packages
                .into_iter()
                .filter_map(|package| package.pkg.as_ref())
                .map(|code| code.target_sdk_version)
                .min()
                .unwrap_or(10000)
        }
        None => 10000,
    };
    if sdk < 8 {
        return Ok(false);
    }
    permission(query, owner, NAME)?;
    Ok(true)
}
fn cross_user(query: &Query<'_>, owner: &Handle, user: i32, message: &str) -> Action<()> {
    if user < 0 {
        return Err(ActionError::Exception(Exception::illegal_argument(format!("Invalid userId {user}"))));
    }
    let uid = query.calling_uid;
    if user == user_id(uid) || matches!(uid, 0 | SYSTEM_UID)
        || context_permission(query, owner, "android.permission.INTERACT_ACROSS_USERS_FULL")? {
        return Ok(());
    }
    Err(ActionError::Exception(Exception::security(format!(
        "{message}: UID {uid} requires android.permission.INTERACT_ACROSS_USERS_FULL to access user {user}."
    ))))
}
fn system(query: &Query<'_>, method: &str) -> Action<()> {
    if query.calling_uid == SYSTEM_UID {
        return Ok(());
    }
    Err(ActionError::Exception(Exception::security(format!(
        "{method} can only be run by the system"
    ))))
}
fn required<T>(value: Option<T>, _method: &'static str) -> Action<T> {
    value.ok_or_else(|| {
        ActionError::Exception(Exception::new(
            aim_binder_host::parcel::EX_NULL_POINTER,
            "null preferred activity argument",
        ))
    })
}

fn filter_valid(filter: &IntentFilter) -> bool {
    filter
        .paths
        .iter()
        .flatten()
        .chain(filter.ssps.iter().flatten())
        .all(|pattern| {
            pattern.kind != PATTERN_ADVANCED_GLOB
                || PatternMatcher::new(&pattern.pattern, pattern.kind)
                    .is_ok_and(|parsed| &parsed == pattern)
        })
}
fn checked_filter(filter: &IntentFilter) -> Action<()> {
    if filter_valid(filter) {
        Ok(())
    } else {
        Err(ActionError::Exception(Exception::illegal_argument(
            "Invalid intent data paths or scheme specific parts in the filter.",
        )))
    }
}
fn snapshot(owner: &Handle, user: i32) -> Action<Snapshot> {
    owner.snapshot(user)?.ok_or_else(|| {
        ActionError::Unavailable(NotModelled("native preferred user registry unavailable"))
    })
}
fn mutate(owner: &Handle, user: i32, mutation: &Mutation, calling_uid: i32) -> Action<bool> {
    let generation = snapshot(owner, user)?.generation;
    owner
        .actions
        .commit_mutation(user, generation, mutation, calling_uid)
}
fn owner_rights(query: &Query<'_>, package: &str) -> Action<()> {
    if app_id(query.calling_uid) == SYSTEM_UID {
        return Ok(());
    }
    let names = query.packages_for_uid(query.calling_uid)?;
    if !names
        .as_ref()
        .is_some_and(|names| names.iter().any(|name| name.as_deref() == Some(package)))
    {
        return Err(ActionError::Exception(Exception::security(format!(
            "Calling uid {} does not own package {package}",
            query.calling_uid
        ))));
    }
    let user = user_id(query.calling_uid);
    if query
        .package_info(package, -1, 0, user)?
        .map_err(ActionError::Exception)?
        .is_none()
    {
        return Err(ActionError::Exception(Exception::illegal_argument(
            format!("Unknown package {package} on user {user}"),
        )));
    }
    Ok(())
}
fn choice(
    filter: ReadFilter,
    match_: i32,
    set: Option<Vec<Option<ComponentName>>>,
    component: ComponentName,
    always: bool,
) -> PreferredActivity {
    // PreferredComponent aborts the entire set when any slot is null.
    let set = set.and_then(|set| set.into_iter().collect::<Option<Vec<_>>>());
    PreferredActivity::new(filter.0, match_, set, component, always)
}
fn finish(result: Action<Parcel>) -> Result<Parcel, QueryError> {
    match result {
        Ok(parcel) => Ok(parcel),
        Err(ActionError::Unavailable(error)) => Err(error.into()),
        Err(ActionError::Exception(exception)) => {
            Ok(reply(|parcel| parcel.write_exception(&exception)))
        }
    }
}

pub const METHODS: &[u32] = &[
    pm::ADD_PREFERRED_ACTIVITY,
    pm::REPLACE_PREFERRED_ACTIVITY,
    pm::CLEAR_PACKAGE_PREFERRED_ACTIVITIES,
    pm::ADD_PERSISTENT_PREFERRED_ACTIVITY,
    pm::CLEAR_PACKAGE_PERSISTENT_PREFERRED_ACTIVITIES,
    pm::CLEAR_PERSISTENT_PREFERRED_ACTIVITY,
    pm::ADD_CROSS_PROFILE_INTENT_FILTER,
    pm::REMOVE_CROSS_PROFILE_INTENT_FILTER,
    pm::CLEAR_CROSS_PROFILE_INTENT_FILTERS,
    pm::RESTORE_PREFERRED_ACTIVITIES,
    pm::GET_DEFAULT_APPS_BACKUP,
    pm::RESTORE_DEFAULT_APPS,
    pm::RESET_APPLICATION_PREFERENCES,
    pm::SET_LAST_CHOSEN_ACTIVITY,
    pm::SET_HOME_ACTIVITY,
];

pub fn answer(
    query: &Query<'_>,
    resolution: &crate::package::resolve::Resolution,
    code: u32,
    data: &mut Reader<'_>,
    owner: &Handle,
) -> Option<Result<Parcel, QueryError>> {
    if !METHODS.contains(&code) {
        return None;
    }
    let result: Action<Parcel> = (|| {
        macro_rules! read {
            ($type:ty) => {{
                let args = <$type>::read(data).map_err(|_| {
                    ActionError::Unavailable(NotModelled("malformed preferred mutation"))
                })?;
                if data.remaining() != 0 {
                    return Err(ActionError::Unavailable(NotModelled(
                        "trailing preferred mutation arguments",
                    )));
                }
                args
            }};
        }
        match code {
            pm::ADD_PREFERRED_ACTIVITY => {
                let args = read!(pm::AddPreferredActivity<ReadFilter, ComponentName>);
                let filter = required(args.filter, "null preferred filter owner")?;
                cross_user(query, owner, args.user_id, "add preferred activity")?;
                if !preferred_permission(query, owner)? {
                    return Ok(reply(pm::write_add_preferred_activity_reply));
                }
                if filter.0.actions.is_empty() {
                    return Ok(reply(pm::write_add_preferred_activity_reply));
                }
                let activity = choice(
                    filter,
                    args.r#match,
                    args.set,
                    required(args.activity, "null preferred component owner")?,
                    true,
                );
                mutate(
                    owner,
                    args.user_id,
                    &Mutation::Add {
                        activity,
                        remove_existing: args.remove_existing,
                    },
                    query.calling_uid,
                )?;
                Ok(reply(pm::write_add_preferred_activity_reply))
            }
            pm::REPLACE_PREFERRED_ACTIVITY => {
                let args = read!(pm::ReplacePreferredActivity<ReadFilter, ComponentName>);
                let activity = choice(
                    required(args.filter, "null preferred replacement filter owner")?,
                    args.r#match,
                    args.set,
                    required(args.activity, "null preferred replacement component owner")?,
                    true,
                );
                // The original validates the shape before enforcing caller rights.
                let mut shape = Preferred::default();
                shape
                    .replace_preferred(activity.clone())
                    .map_err(|message| {
                        ActionError::Exception(Exception::illegal_argument(message))
                    })?;
                cross_user(query, owner, args.user_id, "replace preferred activity")?;
                if !preferred_permission(query, owner)? {
                    return Ok(reply(pm::write_replace_preferred_activity_reply));
                }
                mutate(
                    owner,
                    args.user_id,
                    &Mutation::Replace { activity },
                    query.calling_uid,
                )?;
                Ok(reply(pm::write_replace_preferred_activity_reply))
            }
            pm::CLEAR_PACKAGE_PREFERRED_ACTIVITIES => {
                let args = read!(pm::ClearPackagePreferredActivities);
                if apps_filter::instant_app_package_name(query.state, query.calling_uid)?.is_some()
                {
                    return Ok(reply(pm::write_clear_package_preferred_activities_reply));
                }
                let package = args
                    .package_name
                    .as_deref()
                    .and_then(|name| query.state.packages.get(name));
                let same = if let Some(name) = args.package_name.as_deref() {
                    query
                        .internal_caller_same_app(Some(name), query.calling_uid, false)?
                        .map_err(ActionError::Exception)?
                } else {
                    false
                };
                if (package.is_none() || !same) && !preferred_permission(query, owner)? {
                    return Ok(reply(pm::write_clear_package_preferred_activities_reply));
                }
                if package.is_some()
                    && query.filtered(package, query.calling_uid, user_id(query.calling_uid))?
                {
                    return Ok(reply(pm::write_clear_package_preferred_activities_reply));
                }
                mutate(
                    owner,
                    user_id(query.calling_uid),
                    &Mutation::Clear {
                        package: args.package_name,
                    },
                    query.calling_uid,
                )?;
                Ok(reply(pm::write_clear_package_preferred_activities_reply))
            }
            pm::ADD_PERSISTENT_PREFERRED_ACTIVITY => {
                let args = read!(pm::AddPersistentPreferredActivity<ReadFilter, ComponentName>);
                let filter = required(args.filter, "null persistent preferred filter owner")?.0;
                system(query, "addPersistentPreferredActivity")?;
                checked_filter(&filter)?;
                if filter.actions.is_empty() {
                    return Ok(reply(pm::write_add_persistent_preferred_activity_reply));
                }
                mutate(
                    owner,
                    args.user_id,
                    &Mutation::AddPersistent(PersistentPreferredActivity {
                        filter,
                        component: required(
                            args.activity,
                            "null persistent preferred component owner",
                        )?,
                        set_by_dpm: true,
                    }),
                    query.calling_uid,
                )?;
                Ok(reply(pm::write_add_persistent_preferred_activity_reply))
            }
            pm::CLEAR_PACKAGE_PERSISTENT_PREFERRED_ACTIVITIES => {
                let args = read!(pm::ClearPackagePersistentPreferredActivities);
                system(query, "clearPackagePersistentPreferredActivities")?;
                if let Some(package) = args.package_name {
                    mutate(
                        owner,
                        args.user_id,
                        &Mutation::ClearPersistentPackage(package),
                        query.calling_uid,
                    )?;
                }
                Ok(reply(
                    pm::write_clear_package_persistent_preferred_activities_reply,
                ))
            }
            pm::CLEAR_PERSISTENT_PREFERRED_ACTIVITY => {
                let args = read!(pm::ClearPersistentPreferredActivity<ReadFilter>);
                let filter = required(args.filter, "null persistent filter clear owner")?.0;
                system(query, "clearPersistentPreferredActivity")?;
                if !snapshot(owner, args.user_id)?
                    .state
                    .persistent_resolver_present
                {
                    return Err(ActionError::Exception(Exception::new(
                        aim_binder_host::parcel::EX_NULL_POINTER,
                        "persistent preferred resolver is null",
                    )));
                }
                mutate(
                    owner,
                    args.user_id,
                    &Mutation::ClearPersistentFilter(filter),
                    query.calling_uid,
                )?;
                Ok(reply(pm::write_clear_persistent_preferred_activity_reply))
            }
            pm::ADD_CROSS_PROFILE_INTENT_FILTER => {
                let args = read!(pm::AddCrossProfileIntentFilter<ReadFilter>);
                let filter = required(args.intent_filter, "null cross-profile add filter")?.0;
                permission(query, owner, "android.permission.INTERACT_ACROSS_USERS_FULL")?;
                let package = required(args.owner_package, "null cross-profile owner package")?;
                owner_rights(query, &package)?;
                let access = owner.actions.cross_access(
                    query.calling_uid,
                    args.source_user_id,
                    args.target_user_id,
                    true,
                )?;
                owner
                    .actions
                    .enforce_shell_restriction(query.calling_uid, args.source_user_id)?;
                checked_filter(&filter)?;
                if filter.actions.is_empty() {
                    return Ok(reply(pm::write_add_cross_profile_intent_filter_reply));
                }
                mutate(
                    owner,
                    args.source_user_id,
                    &Mutation::CrossAdd(CrossProfileIntentFilter {
                        filter,
                        owner_package: package,
                        target_user_id: args.target_user_id,
                        flags: args.flags,
                        access_control: access,
                    }),
                    query.calling_uid,
                )?;
                Ok(reply(pm::write_add_cross_profile_intent_filter_reply))
            }
            pm::REMOVE_CROSS_PROFILE_INTENT_FILTER => {
                let args = read!(pm::RemoveCrossProfileIntentFilter<ReadFilter>);
                permission(query, owner, "android.permission.INTERACT_ACROSS_USERS_FULL")?;
                let package = required(args.owner_package, "null cross-profile remove owner")?;
                owner_rights(query, &package)?;
                owner.actions.cross_access(
                    query.calling_uid,
                    args.source_user_id,
                    args.target_user_id,
                    false,
                )?;
                owner
                    .actions
                    .enforce_shell_restriction(query.calling_uid, args.source_user_id)?;
                let removed = mutate(
                    owner,
                    args.source_user_id,
                    &Mutation::CrossRemove {
                        filter: required(args.intent_filter, "null cross-profile remove filter")?.0,
                        owner_package: package,
                        target_user: args.target_user_id,
                        flags: args.flags,
                    },
                    query.calling_uid,
                )?;
                Ok(reply(|parcel| {
                    pm::write_remove_cross_profile_intent_filter_reply(parcel, removed)
                }))
            }
            pm::CLEAR_CROSS_PROFILE_INTENT_FILTERS => {
                let args = read!(pm::ClearCrossProfileIntentFilters);
                permission(query, owner, "android.permission.INTERACT_ACROSS_USERS_FULL")?;
                let package = required(args.owner_package, "null cross-profile clear owner")?;
                owner_rights(query, &package)?;
                owner
                    .actions
                    .enforce_shell_restriction(query.calling_uid, args.source_user_id)?;
                let current = snapshot(owner, args.source_user_id)?;
                let mut targets = Vec::new();
                for filter in current
                    .state
                    .cross_profile
                    .entries()
                    .iter()
                    .filter(|filter| filter.owner_package == package)
                {
                    if owner.actions.cross_accessible(
                        query.calling_uid,
                        args.source_user_id,
                        filter.target_user_id,
                    )? {
                        targets.push(filter.target_user_id);
                    }
                }
                mutate(
                    owner,
                    args.source_user_id,
                    &Mutation::CrossClear {
                        owner_package: package,
                        accessible_targets: targets,
                    },
                    query.calling_uid,
                )?;
                Ok(reply(pm::write_clear_cross_profile_intent_filters_reply))
            }
            pm::RESTORE_PREFERRED_ACTIVITIES => {
                let args = read!(pm::RestorePreferredActivities);
                if query.calling_uid != SYSTEM_UID {
                    return Err(ActionError::Exception(Exception::security(
                        "Only the system may call restorePreferredActivities()",
                    )));
                }
                if let Some(backup) = args.backup {
                    // The original catches malformed/wrong-root backup errors.
                    if let Ok(Some(restored)) =
                        crate::package::preferred::restored_preferred_activities(&backup)
                    {
                        mutate(
                            owner,
                            args.user_id,
                            &Mutation::Restore(restored),
                            query.calling_uid,
                        )?;
                    }
                }
                Ok(reply(pm::write_restore_preferred_activities_reply))
            }
            pm::GET_DEFAULT_APPS_BACKUP => {
                let args = read!(pm::GetDefaultAppsBackup);
                if query.calling_uid != SYSTEM_UID {
                    return Err(ActionError::Exception(Exception::security(
                        "Only the system may call getDefaultAppsBackup()",
                    )));
                }
                let browser = owner.actions.default_browser(args.user_id)?;
                let bytes = crate::package::preferred::default_apps_backup(browser.as_deref()).ok();
                Ok(reply(|parcel| {
                    pm::write_get_default_apps_backup_reply(parcel, &bytes)
                }))
            }
            pm::RESTORE_DEFAULT_APPS => {
                let args = read!(pm::RestoreDefaultApps);
                if query.calling_uid != SYSTEM_UID {
                    return Err(ActionError::Exception(Exception::security(
                        "Only the system may call restoreDefaultApps()",
                    )));
                }
                if let Some(backup) = args.backup {
                    if let Ok(Some(package)) =
                        crate::package::preferred::restored_default_browser(&backup)
                    {
                        let installed = query
                            .state
                            .packages
                            .get(&package)
                            .and_then(|package| package.users.get(&args.user_id))
                            .is_some_and(|user| user.installed);
                        owner
                            .actions
                            .restore_browser(args.user_id, &package, installed)?;
                    }
                }
                Ok(reply(pm::write_restore_default_apps_reply))
            }
            pm::RESET_APPLICATION_PREFERENCES => {
                let args = read!(pm::ResetApplicationPreferences);
                permission(query, owner, "android.permission.SET_PREFERRED_APPLICATIONS")?;
                let defaults = owner.actions.default_preferences(args.user_id)?;
                mutate(
                    owner,
                    args.user_id,
                    &Mutation::Reset { defaults },
                    query.calling_uid,
                )?;
                Ok(reply(pm::write_reset_application_preferences_reply))
            }
            pm::SET_HOME_ACTIVITY => {
                let args = read!(pm::SetHomeActivity<ComponentName>);
                if apps_filter::instant_app_package_name(query.state, query.calling_uid)?.is_some()
                {
                    return Ok(reply(pm::write_set_home_activity_reply));
                }
                let home = super::home::plan(query, resolution, args.user_id, owner)?;
                let candidates = home.candidates;
                let set: Vec<_> = candidates
                    .iter()
                    .map(|candidate| {
                        let (package, class) = candidate.component();
                        ComponentName {
                            package: package.to_owned(),
                            class: class.to_owned(),
                        }
                    })
                    .collect();
                let found = args
                    .class_name
                    .as_ref()
                    .is_some_and(|component| set.contains(component));
                if !found {
                    let name = args
                        .class_name
                        .as_ref()
                        .map(|component| {
                            format!("ComponentInfo{{{}/{}}}", component.package, component.class)
                        })
                        .unwrap_or_else(|| "null".into());
                    return Err(ActionError::Exception(Exception::illegal_argument(
                        format!("Component {name} cannot be home on user {}", args.user_id),
                    )));
                }
                let component = args.class_name.expect("matched non-null home component");
                let mut filter = IntentFilter::default();
                filter.add_action("android.intent.action.MAIN");
                filter.add_category("android.intent.category.HOME");
                filter.add_category("android.intent.category.DEFAULT");
                cross_user(query, owner, args.user_id, "replace preferred activity")?;
                if !preferred_permission(query, owner)? {
                    return Ok(reply(pm::write_set_home_activity_reply));
                }
                owner.actions.commit_after_selection(
                    args.user_id,
                    &Mutation::Replace {
                        activity: PreferredActivity::new(
                            filter,
                            crate::package::intent_filter::MATCH_CATEGORY_EMPTY,
                            Some(set),
                            component,
                            true,
                        ),
                    },
                    query.calling_uid,
                )?;
                Ok(reply(pm::write_set_home_activity_reply))
            }
            pm::SET_LAST_CHOSEN_ACTIVITY => {
                let args = read!(pm::SetLastChosenActivity<Intent, ReadFilter, ComponentName>);
                if apps_filter::instant_app_package_name(query.state, query.calling_uid)?.is_some()
                {
                    return Ok(reply(pm::write_set_last_chosen_activity_reply));
                }
                let user = user_id(query.calling_uid);
                let mut intent = required(args.intent, "null last chosen write Intent owner")?;
                intent.component = None;
                let current = snapshot(owner, user)?;
                let plan = resolution
                    .plan_chosen(
                        &intent,
                        args.resolved_type.as_deref(),
                        i64::from(args.flags),
                        user,
                        query.calling_uid,
                        current.state.as_ref(),
                        true,
                    )
                    .map_err(|error| match error {
                        ResolutionError::Original(error) => ActionError::Exception(error),
                        ResolutionError::NotModelled(error) => ActionError::Unavailable(error),
                        ResolutionError::UriMatching(error) => error
                            .binder_exception()
                            .map(ActionError::Exception)
                            .unwrap_or(ActionError::Unavailable(NotModelled(
                                "last chosen write URI bounds exception",
                            ))),
                    })?;
                // Source bookkeeping precedes addPreferredActivity's permission check.
                if !plan.selection.edits.is_empty() {
                    owner.commit_selection(user, current.generation, &plan.selection)?;
                }
                if !preferred_permission(query, owner)? {
                    return Ok(reply(pm::write_set_last_chosen_activity_reply));
                }
                let activity = choice(
                    required(args.filter, "null last chosen write filter owner")?,
                    args.r#match,
                    None,
                    required(args.activity, "null last chosen write component owner")?,
                    false,
                );
                // Root commits against the newly current registry generation after
                // the optional bookkeeping stage, preserving the source ordering.
                owner.actions.commit_after_selection(
                    user,
                    &Mutation::Add {
                        activity,
                        remove_existing: false,
                    },
                    query.calling_uid,
                )?;
                Ok(reply(pm::write_set_last_chosen_activity_reply))
            }
            _ => {
                return Err(ActionError::Unavailable(NotModelled(
                    "not a preferred mutation",
                )));
            }
        }
    })();
    Some(finish(result))
}
