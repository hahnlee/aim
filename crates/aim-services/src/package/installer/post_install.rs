//! Accepted native post-install user scopes, ported from InstallRequest/BroadcastHelper.
//! AOSP android-16.0.0_r1, Apache License 2.0.
use crate::package::scan_snapshot::Snapshot;
use aim_binder_host::parcel::Exception;

#[derive(Clone,Debug)]
pub struct Plan {
    published:std::sync::Arc<Snapshot>,
    pub prior_visibility:Vec<i32>,
    pub name:String,pub app_id:i32,pub path:String,pub version:i64,
    pub first_users:Vec<i32>,pub first_instant:Vec<i32>,pub update_users:Vec<i32>,pub update_instant:Vec<i32>,
    pub removed_users:Vec<i32>,pub removed_instant:Vec<i32>,pub cache_users:Vec<i32>,
    pub replacing:bool,pub dont_kill:bool,pub installer:Option<String>,pub old_installer:Option<String>,
    pub data_loader:i32,pub system:bool,pub virtual_preload:bool,pub static_library:bool,pub old_path:Option<String>,
}
impl Plan {
    pub fn capture(before:&Snapshot,after:&std::sync::Arc<Snapshot>,name:&str,_user:i32,flags:i32,data_loader:i32,static_library:bool,all_users:&[i32])->Result<Self,Exception>{
        let error=|message:&str|Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,format!("Post-install {name}: {message}"));
        let setting=after.owner().settings.packages.iter().find(|setting|setting.name==name).ok_or_else(||error("published setting absent"))?;
        let states=after.owner().scanned_user_states(name).ok_or_else(||error("published user owner absent"))?;
        let old=before.owner().settings.packages.iter().find(|setting|setting.name==name);
        let old_states=if old.is_some(){Some(before.owner().scanned_user_states(name).ok_or_else(||error("prior user owner absent"))?)}else{None};
        if all_users.iter().any(|id|!states.contains_key(id)) {return Err(error("actual user inventory outside published owner"));}
        let targets=all_users.iter().copied().filter(|id|states[id].installed).collect::<Vec<_>>();
        let mut plan=Self{published:after.clone(),prior_visibility:vec![],name:name.into(),app_id:setting.uid_owner_id(),path:setting.code_path.clone(),version:setting.version_code,
            first_users:vec![],first_instant:vec![],update_users:vec![],update_instant:vec![],removed_users:vec![],removed_instant:vec![],
            cache_users:all_users.to_vec(),replacing:old.is_some(),dont_kill:flags&0x1000!=0,
            installer:setting.install_source.installer.clone(),old_installer:old.and_then(|setting|setting.install_source.installer.clone()),data_loader,
            system:setting.flags&1!=0,virtual_preload:flags&0x10000!=0,static_library,old_path:old.map(|setting|setting.code_path.clone())};
        for id in targets {
            let state=states.get(&id).filter(|state|state.installed).ok_or_else(||error("target user not published installed"))?;
            let previous=old_states.and_then(|states|states.get(&id));
            if old_states.is_some()&&previous.is_none(){return Err(error("prior target user outside capture"));}
            let update=previous.is_some_and(|state|state.installed||state.ce_data_inode>0||state.de_data_inode>0);
            let users=match (update,state.instant_app){(false,false)=>&mut plan.first_users,(false,true)=>&mut plan.first_instant,(true,false)=>&mut plan.update_users,(true,true)=>&mut plan.update_instant};users.push(id);
        }
        if let Some(states)=old_states {
            for (id,state) in states.iter().filter(|(id,state)|all_users.contains(id)&&(state.installed||state.ce_data_inode>0||state.de_data_inode>0)){if state.instant_app{plan.removed_instant.push(*id);}else{plan.removed_users.push(*id);}}
        }
        Ok(plan)
    }
    pub fn validate(&self,current:&Snapshot)->Result<(),Exception>{
        let setting=current.owner().settings.packages.iter().find(|setting|setting.name==self.name)
            .ok_or_else(||Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"post-install setting retired"))?;
        if setting.uid_owner_id()!=self.app_id||setting.code_path!=self.path||setting.version_code!=self.version {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"post-install code/UID owner changed"));
        }
        let expected=self.published.owner().settings.packages.iter().find(|setting|setting.name==self.name).unwrap();
        if setting.signatures!=expected.signatures||setting.volume_uuid!=expected.volume_uuid||setting.shared_user!=expected.shared_user
            ||setting.shared_user_app_id!=expected.shared_user_app_id {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"post-install signer/shared UID owner changed"));
        }
        let expected_code=self.published.owner().loaded_packages().get(&self.name);
        let current_code=current.owner().loaded_packages().get(&self.name);
        if expected_code.map(|code|&code.package)!=current_code.map(|code|&code.package) {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"post-install parsed code owner changed"));
        }
        let states=current.owner().scanned_user_states(&self.name).ok_or_else(||Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"post-install user owner retired"))?;
        for (users,instant) in [(&self.first_users,false),(&self.first_instant,true),(&self.update_users,false),(&self.update_instant,true)] {
            if users.iter().any(|user|states.get(user).is_none_or(|state|!state.installed||state.instant_app!=instant)) {
                return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"post-install target user changed"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{restrictions::UserState,scan::{SigningScan,CapturedUsers},settings::{Settings,Package},owner::usage::Usage,scan_snapshot::Store};
    use std::collections::BTreeMap;
    fn snapshot(existing:bool,states:BTreeMap<i32,UserState>)->std::sync::Arc<Snapshot>{
        let settings=Settings{packages:if existing{vec![Package{name:"fixture".into(),app_id:10100,code_path:"/data/app/fixture-new".into(),version_code:2,..Default::default()}]}else{vec![]},..Default::default()};
        let mut scan=SigningScan::new(&Default::default(),&settings,36).unwrap();
        if existing{scan.capture_user_states(BTreeMap::from([(("fixture".into(),false),CapturedUsers{states,active_aliases:Default::default()})])).unwrap();}
        Store::new(scan,Usage::new(if existing{vec!["fixture"]}else{vec![]})).unwrap().capture()
    }
    #[test]
    fn post_install_classifies_global_update_retained_data_and_mixed_instant_users(){
        let user=|installed,instant,inode|UserState{installed,instant_app:instant,ce_data_inode:inode,..Default::default()};
        let before=snapshot(true,BTreeMap::from([(0,user(true,false,0)),(10,user(false,false,123)),(11,user(false,false,0)),(12,user(true,true,0))]));
        let after=snapshot(true,BTreeMap::from([(0,user(true,false,0)),(10,user(true,false,123)),(11,user(true,false,0)),(12,user(true,true,0))]));
        let plan=Plan::capture(&before,&after,"fixture",11,0x1000,0,false,&[0,10,11,12]).unwrap();
        assert_eq!(plan.first_users,vec![11]);assert_eq!(plan.update_users,vec![0,10]);assert_eq!(plan.update_instant,vec![12]);
        assert_eq!(plan.removed_users,vec![0,10]);assert_eq!(plan.removed_instant,vec![12]);assert!(plan.replacing&&plan.dont_kill);
        plan.validate(&after).unwrap();
        let changed=snapshot(true,BTreeMap::from([(0,user(true,false,0)),(10,user(false,false,123)),(11,user(true,false,0)),(12,user(true,true,0))]));
        assert!(plan.validate(&changed).is_err());
        let fresh=Plan::capture(&snapshot(false,BTreeMap::new()),&after,"fixture",0,0,0,false,&[0,10,11,12]).unwrap();
        assert!(!fresh.replacing);assert_eq!(fresh.first_users,vec![0,10,11]);assert_eq!(fresh.first_instant,vec![12]);assert!(fresh.removed_users.is_empty());
        validate_visibility(&[0,10,11,12],&[0,2,1000,10100,10,-1,11,0,12,1,1000]).unwrap();
        assert!(validate_visibility(&[0,10],&[0,-1,0,-1]).is_err());
        assert!(validate_visibility(&[0],&[0,2,1000]).is_err());
        let pending=Pending::default();let mut ordinary=plan.clone();ordinary.prior_visibility=vec![0,1,1000,10,-1,11,0,12,1,1000];ordinary.dont_kill=false;ordinary.old_path=Some("/data/app/fixture-old".into());
        pending.retain(10,vec![ordinary.clone()]).unwrap();
        let calls=std::cell::RefCell::new(vec![]);
        assert!(pending.complete(10,||{calls.borrow_mut().push("ART");Err(Exception::new(-4,"ART failed"))},|_|{calls.borrow_mut().push("broadcast");Ok(())},|_|{calls.borrow_mut().push("cleanup");Ok(())}).is_err());
        assert_eq!(*calls.borrow(),vec!["ART"]);
        assert!(pending.complete(10,||Ok(()),|_|Ok(()),|_|Ok(())).is_err(),"failed completion must retire its plan");
        pending.retain(11,vec![ordinary]).unwrap();calls.borrow_mut().clear();calls.borrow_mut().push("cache");
        pending.complete(11,||{calls.borrow_mut().push("ART");Ok(())},|saved|{assert_eq!(saved.prior_visibility,vec![0,1,1000,10,-1,11,0,12,1,1000]);calls.borrow_mut().push("broadcast");Ok(())},|_|{calls.borrow_mut().push("cleanup");Ok(())}).unwrap();
        assert_eq!(*calls.borrow(),vec!["cache","ART","broadcast","cleanup"]);
        pending.retain(12,vec![plan]).unwrap();calls.borrow_mut().clear();
        pending.complete(12,||Ok(()),|_|Ok(()),|_|{calls.borrow_mut().push("cleanup");Ok(())}).unwrap();assert!(calls.borrow().is_empty(),"dont-kill must retain old code");

    }
}

#[derive(Default)]
pub(super) struct Pending(std::sync::Mutex<std::collections::BTreeMap<u64,Vec<Plan>>>);
impl Pending {
    pub fn retain(&self,generation:u64,plans:Vec<Plan>)->Result<(),Exception>{
        let mut pending=self.0.lock().unwrap();
        if pending.len()>=1024||pending.contains_key(&generation){return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"post-install pending owner capacity/generation differs"));}
        pending.insert(generation,plans);Ok(())
    }
    pub fn complete(&self,generation:u64,completion:impl FnOnce()->Result<(),Exception>,
        mut publish:impl FnMut(&Plan)->Result<(),Exception>,mut cleanup:impl FnMut(&str)->Result<(),Exception>)->Result<(),Exception>{
        let plans=self.0.lock().unwrap().remove(&generation).ok_or_else(||Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"post-install published plan absent"))?;
        completion()?;
        for plan in plans {
            publish(&plan)?;
            if !plan.dont_kill {if let Some(old)=plan.old_path.as_deref().filter(|old|*old!=plan.path&&old.starts_with("/data/app/")){cleanup(old)?;}}
        }
        Ok(())
    }
}

pub(super) fn validate_visibility(users:&[i32],values:&[i32])->Result<(),Exception>{
    let mut seen=std::collections::BTreeSet::new();let mut at=0;
    while at<values.len(){
        if values.len()-at<2{return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"prior visibility truncated"));}
        let user=values[at];let count=values[at+1];at+=2;
        if !users.contains(&user)||!seen.insert(user)||count< -1||count>=0&&count as usize>values.len()-at {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"prior visibility user/count differs"));
        }
        if count>=0 {at+=count as usize;}
    }
    if seen.len()!=users.len(){return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"prior visibility inventory differs"));}
    Ok(())
}
