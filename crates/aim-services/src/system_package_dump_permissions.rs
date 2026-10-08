//! Live original permission data for a retained native dump target.
use super::*;
use std::collections::BTreeMap;
use crate::package::{model::State,owner::legacy_permissions};

pub(crate) fn validate_identity(before:&State,after:&State,names:&[String])->Result<()> {
    if before.users.keys().ne(after.users.keys()) {
        return Err(failure("package dump permission user inventory changed"));
    }
    for name in names {
        let before=before.packages.get(name).ok_or_else(||failure("package dump target absent"))?;
        let after=after.packages.get(name).ok_or_else(||failure("package dump target retired"))?;
        if before.app_id!=after.app_id || before.shared_user_app_id!=after.shared_user_app_id
            || before.shared_user!=after.shared_user || before.path!=after.path
            || before.version_code!=after.version_code || before.pkg!=after.pkg
            || before.users.keys().ne(after.users.keys())
            || before.users.iter().any(|(id,user)|after.users.get(id).is_none_or(|other|user.installed!=other.installed||user.hidden!=other.hidden)) {
            return Err(failure(format!("package dump permission UID/code owner changed: {name}")));
        }
    }
    Ok(())
}
impl System {
    pub(crate) fn capture_package_dump_permissions(&self,state:&State,names:&[String])->Result<BTreeMap<i32,legacy_permissions::State>> {
        let bridge=self.package_bootstrap()?;self.check_package_bootstrap(&bridge)?;
        validate_identity(state,self.capture_package_queries()?.state(),names)?;
        let users=state.users.keys().copied().collect::<Vec<_>>();
        let mut ids=std::collections::BTreeSet::new();
        for name in names {
            let package=state.packages.get(name).ok_or_else(||failure("package dump target absent"))?;
            let app_id=package.shared_user_app_id.unwrap_or(package.app_id);
            if app_id<0 {
                if package.pkg.as_ref().is_some_and(|code|code.is2(crate::package::pkg::booleans2::APEX)){continue;}
                return Err(failure("package dump permission UID owner absent"));
            }
            ids.insert(app_id);
        }
        let mut live=BTreeMap::new();
        for app_id in ids {
            let value=bridge.legacy_permissions(app_id,&users).map_err(|error|match error{
                legacy_permissions::Error::Owner(error)=>error,
                other=>failure(format!("package dump original permission owner: {other:?}")),
            })?;
            live.insert(app_id,value);
        }
        self.check_package_bootstrap(&bridge)?;
        validate_identity(state,self.capture_package_queries()?.state(),names)?;
        Ok(live)
    }
}
fn failure(message:impl Into<String>)->Exception{Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,message)}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dump_permission_identity_rejects_uid_code_user_and_install_state_changes() {
        let mut before=State::default();
        before.users.insert(0,crate::package::model::User{id:0,..Default::default()});
        let package=crate::package::model::PackageState{name:"fixture.app".into(),app_id:10100,path:"/data/app/fixture".into(),version_code:1,
            users:BTreeMap::from([(0,crate::package::model::PackageUserState{installed:true,..Default::default()})]),..Default::default()};
        before.packages.insert(package.name.clone(),package);
        let names=vec!["fixture.app".into()];validate_identity(&before,&before,&names).unwrap();
        for change in 0..6 {
            let mut after=before.clone();let package=after.packages.get_mut("fixture.app").unwrap();
            match change {0=>package.app_id+=1,1=>package.path.push_str("-changed"),2=>package.shared_user_app_id=Some(10200),
                3=>{package.users.remove(&0);},4=>package.users.get_mut(&0).unwrap().installed=false,_=>package.users.get_mut(&0).unwrap().hidden=true}
            assert!(validate_identity(&before,&after,&names).is_err());
        }
        let mut after=before.clone();after.packages.get_mut("fixture.app").unwrap().users.get_mut(&0).unwrap().stopped=true;
        validate_identity(&before,&after,&names).unwrap();
        after.users.clear();assert!(validate_identity(&before,&after,&names).is_err());
    }
}
