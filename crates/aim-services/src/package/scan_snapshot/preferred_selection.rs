//! Internal preferred selection commits against the retained native registry generation.
use super::query_state::Capture;
use crate::package::{apps_filter::NotModelled,query::Query,intent::{Intent,ComponentName},preferred::{Candidate,SelectionPolicy},query::preferred::{Owner,MutationOwner}};
use super::components::RawError;
impl Capture {
    pub fn preferred_selection(&self,query:&Query<'_>,intent:Option<&Intent>,ty:Option<&str>,flags:i64,
        components:Option<&[Option<ComponentName>]>,matches:Option<&[i32]>,always:bool,remove:bool,filtered:bool,provisioned:bool,user:i32)->Result<Vec<i32>,RawError> {
        let none=||vec![0,-1];
        if !query.state.users.contains_key(&user)||crate::package::apps_filter::instant_app_package_name(query.state,query.calling_uid).map_err(RawError::Gap)?.is_some(){return Ok(none());}
        let intent=intent.ok_or_else(||RawError::Original(aim_binder_host::parcel::Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,"preferred intent is null")))?;
        let intent=intent.selector.as_deref().unwrap_or(intent);
        let (Some(components),Some(matches))=(components,matches)else{return Err(RawError::Transport(aim_binder_host::parcel::BAD_VALUE));};
        if components.len()!=matches.len(){return Err(RawError::Transport(aim_binder_host::parcel::BAD_VALUE));}
        let candidates=components.iter().zip(matches).map(|(component,matched)|component.clone().map(|component|Candidate{component,match_:*matched}).ok_or(RawError::Transport(aim_binder_host::parcel::BAD_VALUE))).collect::<Result<Vec<_>,_>>()?;
        let owner=query.state.system.preferred_owner.as_ref().ok_or(RawError::Gap(NotModelled("native preferred owner unavailable")))?;
        let Some(snapshot)=owner.snapshot(user).map_err(RawError::Gap)? else{return Ok(none());};
        let registrations=snapshot.state.matching_registrations(intent,ty,flags&0x10000!=0).map_err(|error|match error.binder_exception(){Some(error)=>RawError::Original(error),None=>RawError::Transport(aim_binder_host::parcel::UNKNOWN_TRANSACTION)})?;
        let home=intent.action.as_deref()==Some("android.intent.action.MAIN")&&intent.categories.as_ref().is_some_and(|cats|cats.iter().any(|c|c=="android.intent.category.HOME")&&cats.iter().any(|c|c=="android.intent.category.DEFAULT"));
        let exclude=home&&!provisioned;
        let roles=query.state.system.roles.as_ref().ok_or(RawError::Gap(NotModelled("actual setup wizard role owner unavailable")))?;
        let system=Query{state:query.state,filter:query.filter,calling_uid:1000};
        let wizard=roles.package(crate::package::roles::Role::SetupWizard,&system).map_err(RawError::Original)?;
        let policy=SelectionPolicy{always,remove_matches:remove,allow_set_mutation:!exclude&&!filtered,improve_home_behavior:query.state.system.flags.iter().any(|(name,value)|name=="android.content.pm.improve_home_app_behavior"&&*value),home_intent:intent.action.as_deref()==Some("android.intent.action.MAIN")&&intent.categories.as_ref().is_some_and(|cats|cats.iter().any(|c|c=="android.intent.category.HOME")),excluded_setup_wizard:if exclude{wizard.as_deref()}else{None}};
        let failure=std::cell::RefCell::new(None);
        let selection=snapshot.state.select(&registrations,&candidates,policy,|component|{
            match query.activity_info(component,flags|0x200|0xc0000,query.calling_uid,user){Ok(Ok(value))=>Ok(value.is_some()),Ok(Err(error))=>{*failure.borrow_mut()=Some(RawError::Original(error));Err("preferred lookup exception".into())},Err(error)=>{*failure.borrow_mut()=Some(RawError::Gap(error));Err("preferred lookup owner".into())}}
        },|activity|{
            let Some(set)=&activity.set else{return Ok(false);};let mut count=0;
            for candidate in &candidates {
                if exclude&&wizard.as_deref()==Some(candidate.component.package.as_str()){continue;}
                let Some(state)=query.state.packages.get(&candidate.component.package).and_then(|state|state.users.get(&user))else{continue;};
                if state.install_reason==2{continue;}
                if !set.contains(&candidate.component){return Ok(false);}count+=1;
            }
            Ok(count==set.len())
        }).map_err(|_|RawError::Gap(NotModelled("preferred registration selection failed")))?;
        if let Some(error)=failure.into_inner(){return Err(error);}
        let changed=!selection.edits.is_empty();
        if changed {owner.commit_selection(user,snapshot.generation,&selection).map_err(RawError::Gap)?;}
        Ok(vec![i32::from(changed),selection.chosen.map_or(-1,|index|index as i32)])
    }
}
