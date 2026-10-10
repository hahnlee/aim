//! ComponentResolver.adjustPriority, android-16.0.0_r1. Copyright AOSP,
//! Apache-2.0. Only activity filters are adjusted; supplied APKs stay unchanged.
use super::super::{intent_filter::IntentFilter,model::PackageState,pkg::Activity};
fn subset<T:PartialEq>(candidate:Option<&[T]>,original:Option<&[T]>)->bool {
    candidate.unwrap_or_default().iter().all(|value|original.unwrap_or_default().contains(value))
}
fn same_activity(a:&Activity,b:&Activity)->bool {
    a.main.component.name==b.main.component.name
        ||a.target_activity.as_deref()==Some(b.main.component.name.as_str())
        ||b.target_activity.as_deref()==Some(a.main.component.name.as_str())
        ||b.target_activity.as_ref().is_some_and(|name|a.target_activity.as_ref()==Some(name))
}
fn image_priority(package:&PackageState,filter:&IntentFilter,wizard:Option<Option<&str>>)->i32 {
    if filter.priority<=0{return filter.priority;}
    if !package.is.privileged{return 0;}
    if filter.actions.iter().any(|action|matches!(action.as_str(),"android.intent.action.VIEW"|"android.intent.action.SEND"|"android.intent.action.SENDTO"|"android.intent.action.SEND_MULTIPLE")) {
        return match wizard {None=>filter.priority,Some(Some(name))if name==package.name=>filter.priority,_=>0};
    }
    filter.priority
}
pub(super) fn adjust(package:&PackageState,factory:Option<&PackageState>,activity:&Activity,
    filter:&IntentFilter,wizard:Option<Option<&str>>)->i32 {
    let priority=image_priority(package,filter,wizard);
    if priority<=0||filter.actions.iter().any(|action|matches!(action.as_str(),"android.intent.action.VIEW"|"android.intent.action.SEND"|"android.intent.action.SENDTO"|"android.intent.action.SEND_MULTIPLE")){return priority;}
    let Some(factory)=factory.and_then(|factory|factory.pkg.as_ref().map(|code|(factory,code)))else{return filter.priority;};
    let Some(original)=factory.1.activities.iter().find(|original|same_activity(activity,original))else{return 0;};
    let cap=original.main.component.intents.iter().filter(|original|{
        let original=&original.filter;
        subset(Some(&filter.actions),Some(&original.actions))
            &&subset(filter.categories.as_deref(),original.categories.as_deref())
            &&subset(filter.schemes.as_deref(),original.schemes.as_deref())
            &&filter.authorities.as_deref().unwrap_or_default().iter().all(|value|original.authorities.as_deref().unwrap_or_default().iter().any(|old|old.host==value.host&&old.wild==value.wild&&old.port==value.port))
    }).map(|original|image_priority(factory.0,&original.filter,wizard)).max().unwrap_or(0).max(0);
    priority.min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::pkg::{AndroidPackage,Component,MainComponent};
    use crate::package::intent_filter::ParsedIntentInfo;
    use std::sync::Arc;
    fn activity(name:&str,action:&str,priority:i32)->Activity {
        let mut filter=IntentFilter::default();filter.add_action(action);filter.priority=priority;
        Activity{main:MainComponent{component:Component{name:name.into(),package_name:"owned.package".into(),intents:vec![ParsedIntentInfo{filter,..Default::default()}],..Default::default()},..Default::default()},..Default::default()}
    }
    fn package(privileged:bool,activities:Vec<Activity>)->PackageState {
        let mut state=PackageState{name:"owned.package".into(),pkg:Some(Arc::new(AndroidPackage{activities,..Default::default()})),..Default::default()};state.is.system=true;state.is.privileged=privileged;state
    }
    #[test]
    fn system_without_privilege_and_protected_privileged_actions_are_capped(){
        let a=activity("owned.Activity","android.intent.action.SEARCH",100);let mut p=package(false,vec![a.clone()]);
        assert_eq!(adjust(&p,None,&a,&a.main.component.intents[0].filter,Some(None)),0);
        p.is.privileged=true;assert_eq!(adjust(&p,None,&a,&a.main.component.intents[0].filter,Some(None)),100);
        for action in ["android.intent.action.VIEW","android.intent.action.SEND","android.intent.action.SENDTO","android.intent.action.SEND_MULTIPLE"] {
            let a=activity("owned.Activity",action,100);let filter=&a.main.component.intents[0].filter;
            assert_eq!(adjust(&p,None,&a,filter,Some(None)),0);
            assert_eq!(adjust(&p,None,&a,filter,Some(Some("owned.package"))),100);
            assert_eq!(adjust(&p,None,&a,filter,None),100,"boot scan defers until real wizard selection");
        }
    }
    #[test]
    fn updates_match_factory_alias_and_all_original_filter_subsets(){
        let original=activity("owned.Activity","android.intent.action.SEARCH",40);let factory=package(true,vec![original.clone()]);
        let mut update=activity("owned.Alias","android.intent.action.SEARCH",100);update.target_activity=Some("owned.Activity".into());
        let p=package(true,vec![update.clone()]);let mut filter=update.main.component.intents[0].filter.clone();
        assert_eq!(adjust(&p,Some(&factory),&update,&filter,Some(None)),40);
        let mut broadened=filter.clone();broadened.schemes=Some(vec!["new.scheme".into()]);assert_eq!(adjust(&p,Some(&factory),&update,&broadened,Some(None)),0);
        let mut broadened=filter.clone();broadened.authorities=Some(vec![crate::package::intent_filter::AuthorityEntry::new("new.example",None)]);assert_eq!(adjust(&p,Some(&factory),&update,&broadened,Some(None)),0);
        let mut broadened=filter.clone();broadened.add_action("new.action");assert_eq!(adjust(&p,Some(&factory),&update,&broadened,Some(None)),0);
        filter.add_category("new.category");assert_eq!(adjust(&p,Some(&factory),&update,&filter,Some(None)),0);
        let new=activity("new.Activity","android.intent.action.SEARCH",100);assert_eq!(adjust(&p,Some(&factory),&new,&new.main.component.intents[0].filter,Some(None)),0);
        let mut protected=original.clone();protected.main.component.intents[0].filter.add_action("android.intent.action.VIEW");
        let protected_factory=package(true,vec![protected]);
        assert_eq!(adjust(&p,Some(&protected_factory),&update,&update.main.component.intents[0].filter,Some(None)),0,"factory protected filter is capped before update subset comparison");
        filter.priority=-10;assert_eq!(adjust(&p,Some(&factory),&update,&filter,Some(None)),-10);
    }
}
