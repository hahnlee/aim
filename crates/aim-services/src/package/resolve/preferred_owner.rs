//! Native preferred bookkeeping uses an owner snapshot and returns its edits
//! with the selected ResolveInfo. The System disk gate commits before replying.
use super::*;
use crate::package::preferred::{Candidate, Selection, SelectionPolicy};

pub struct PreferredPlan {
    pub chosen: Option<ResolveInfo>,
    pub selection: Selection,
}
impl Resolution {
    pub fn plan_last_chosen(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        calling_uid: i32,
        preferred: &Preferred,
    ) -> Result<PreferredPlan> {
        self.plan_chosen(
            intent,
            resolved_type,
            flags,
            user,
            calling_uid,
            preferred,
            false,
        )
    }
    pub fn plan_chosen(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        calling_uid: i32,
        preferred: &Preferred,
        remove_matches: bool,
    ) -> Result<PreferredPlan> {
        self.plan_preferred(
            intent,
            resolved_type,
            flags,
            user,
            calling_uid,
            preferred,
            remove_matches,
            false,
            false,
        )
    }
    pub fn plan_preferred(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        calling_uid: i32,
        preferred: &Preferred,
        remove_matches: bool,
        always: bool,
        query_may_be_filtered: bool,
    ) -> Result<PreferredPlan> {
        if !self.user_exists(user) || instant_app_package_name(&self.state, calling_uid)?.is_some()
        {
            return Ok(PreferredPlan {
                chosen: None,
                selection: Selection::default(),
            });
        }
        let candidates =
            self.query_intent_activities(intent, resolved_type, flags, user, calling_uid)?;
        self.plan_preferred_from_candidates(
            intent,
            resolved_type,
            flags,
            user,
            calling_uid,
            preferred,
            remove_matches,
            always,
            query_may_be_filtered,
            candidates,
        )
    }
    pub fn plan_preferred_from_candidates(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        calling_uid: i32,
        preferred: &Preferred,
        remove_matches: bool,
        always: bool,
        query_may_be_filtered: bool,
        candidates: Vec<ResolveInfo>,
    ) -> Result<PreferredPlan> {
        if !self.user_exists(user) || instant_app_package_name(&self.state, calling_uid)?.is_some()
        {
            return Ok(PreferredPlan {
                chosen: None,
                selection: Selection::default(),
            });
        }
        let capture = self.implicit_image_capture(intent, user, resolved_type, flags)?;
        let flags =
            self.update_flags_for_resolve(flags, user, calling_uid, false, false, capture)?;
        let intent = intent.selector.as_deref().unwrap_or(intent);
        if let Some(chosen) =
            self.find_persistent(intent, resolved_type, flags, &candidates, user, calling_uid)?
        {
            return Ok(PreferredPlan {
                chosen: Some(chosen),
                selection: Selection::default(),
            });
        }
        let matching = preferred
            .matching_registrations(intent, resolved_type, flags & MATCH_DEFAULT_ONLY != 0)
            .map_err(ResolutionError::UriMatching)?;
        let home = intent.action.as_deref() == Some("android.intent.action.MAIN")
            && intent
                .categories
                .iter()
                .flatten()
                .any(|category| category == "android.intent.category.HOME")
            && intent
                .categories
                .iter()
                .flatten()
                .any(|category| category == "android.intent.category.DEFAULT");
        let components: Vec<_> = candidates
            .iter()
            .map(|candidate| {
                let (package, class) = candidate.component();
                Candidate {
                    component: ComponentName {
                        package: package.to_owned(),
                        class: class.to_owned(),
                    },
                    match_: candidate.match_,
                }
            })
            .collect();
        let provisioned=if home {self.device_provisioned()?}else{true};
        let policy = SelectionPolicy {
            always,
            remove_matches,
            allow_set_mutation: !(home && !provisioned)
                && !query_may_be_filtered,
            improve_home_behavior: self.state.system.flags.iter().any(|(name, value)| {
                name == "android.content.pm.improve_home_app_behavior" && *value
            }),
            home_intent: home,
            excluded_setup_wizard: if home && !provisioned {
                self.setup_wizard()?
            } else {
                None
            },
        };
        let lookup = flags
            | super::super::info::flags::MATCH_DISABLED_COMPONENTS
            | super::super::info::flags::MATCH_DIRECT_BOOT_AWARE
            | super::super::info::flags::MATCH_DIRECT_BOOT_UNAWARE;
        let lookup_error = std::cell::RefCell::new(None);
        let selection = preferred.select(
            &matching,
            &components,
            policy,
            |component| match self.activity(component, lookup, calling_uid, user) {
                Ok(found) => Ok(found),
                Err(error) => {
                    *lookup_error.borrow_mut() = Some(error);
                    Err("preferred component lookup failed".into())
                }
            },
            |activity| {
                self.same_set(
                    activity,
                    &candidates,
                    home && !provisioned,
                    user,
                )
                .map_err(|error| {
                    *lookup_error.borrow_mut() = Some(error);
                    "preferred candidate set owner failed".to_owned()
                })
            },
        );
        if let Some(error) = lookup_error.into_inner() {
            return Err(error);
        }
        let selection = selection
            .map_err(|_| NotModelled("native preferred registration identity unavailable"))?;
        let chosen = selection.chosen.map(|index| candidates[index].clone());
        Ok(PreferredPlan { chosen, selection })
    }
}

pub struct HomePlan {
    pub candidates: Vec<ResolveInfo>,
    pub chosen: Option<ComponentName>,
    pub selection: Selection,
}
impl Resolution {
    pub fn from_native_query(
        query: &super::super::query::Query<'_>,
    ) -> std::result::Result<Self, MimeGroupError> {
        let state = Arc::new(query.state.clone());
        let preferred = state
            .system
            .preferred_owner
            .as_ref()
            .map(|owner| {
                owner
                    .captured
                    .user_states()
                    .into_iter()
                    .map(|(user, state)| (user, state.as_ref().clone()))
                    .collect()
            })
            .unwrap_or_default();
        let legacy = state
            .users
            .iter()
            .map(|(&user, state)| (user, domains::legacy_domain_states(state)))
            .collect();
        Ok(Self {
            components: ComponentResolver::new(&state)?,
            apps_filter: query.filter.clone(),
            state,
            preferred,
            setup_wizard: OnceLock::new(),
            legacy,
        })
    }
    pub fn plan_home(
        &self,
        user: i32,
        calling_uid: i32,
        preferred: &Preferred,
        role_home: Option<&str>,
    ) -> Result<HomePlan> {
        let intent = Intent {
            action: Some("android.intent.action.MAIN".into()),
            categories: Some(vec![
                "android.intent.category.HOME".into(),
                "android.intent.category.DEFAULT".into(),
            ]),
            ..Intent::default()
        };
        let candidates = self.query_intent_activities(
            &intent,
            None,
            super::super::info::flags::GET_META_DATA,
            user,
            calling_uid,
        )?;
        let mut selection = Selection::default();
        let mut package = role_home.map(str::to_owned);
        if package.is_none() {
            let plan = self.plan_preferred_from_candidates(
                &intent,
                None,
                0,
                user,
                calling_uid,
                preferred,
                false,
                true,
                app_id(calling_uid) >= 10000,
                candidates.clone(),
            )?;
            package = plan
                .chosen
                .as_ref()
                .map(|chosen| chosen.component().0.to_owned());
            selection = plan.selection;
        }
        let chosen = package
            .as_deref()
            .and_then(|package| {
                candidates
                    .iter()
                    .find(|candidate| candidate.component().0 == package)
            })
            .map(|candidate| {
                let (package, class) = candidate.component();
                ComponentName {
                    package: package.to_owned(),
                    class: class.to_owned(),
                }
            });
        Ok(HomePlan {
            candidates,
            chosen,
            selection,
        })
    }
}
impl HomePlan {
    pub fn default_home(&self) -> Option<ComponentName> {
        if self.chosen.is_some() {
            return self.chosen.clone();
        }
        let mut highest = i32::MIN;
        let mut chosen = None;
        for candidate in &self.candidates {
            if candidate.priority > highest {
                highest = candidate.priority;
                let (package, class) = candidate.component();
                chosen = Some(ComponentName {
                    package: package.to_owned(),
                    class: class.to_owned(),
                });
            } else if candidate.priority == highest {
                chosen = None;
            }
        }
        chosen
    }
}
