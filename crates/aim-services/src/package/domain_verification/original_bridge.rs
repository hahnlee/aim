//! Typed original DVS settings publication; merge its delta into the native
//! immutable projection without overwriting unrelated newer native changes.
use super::{State,Package};
use super::owner::Owner;
use std::collections::BTreeSet;
fn field<T:Clone+PartialEq>(before:&T,desired:&T,current:&T,label:&str)->Result<T,String>{
    if before==desired{return Ok(current.clone());}
    if current!=before&&current!=desired{return Err(format!("concurrent native/original domain change: {label}"));}
    Ok(desired.clone())
}
// A retired UUID can appear in a queued export after native migration/removal. It
// carries no delta only when it is exactly the previously admitted DVS state.
fn retired_unchanged(before:&State,after:&Package)->bool {
    before.active.iter().any(|old| old.name==after.name
        && old.id.eq_ignore_ascii_case(&after.id)
        && old.domains==after.domains && old.users==after.users
        && old.uri_relative_filter_groups==after.uri_relative_filter_groups
        && old.has_auto_verify_domains==after.has_auto_verify_domains
        && old.signature==after.signature)
}
/// Native admitted lifecycle state, independent of the last successful DVS save.
#[derive(Clone,Default)]
pub(crate) struct AdmissionReceipts {
    generation:u64,
    packages:std::collections::BTreeMap<(String,String),(u64,Package)>,
}
impl AdmissionReceipts {
    pub(crate) fn record(&mut self,generation:u64,owner:&Owner)->Result<(),String> {
        if generation==0||generation<self.generation{return Err("domain lifecycle admission generation regressed".into());}
        for name in owner.attached_names() {
            let package=owner.package(name).ok_or("native admitted domain package absent")?.clone();
            self.packages.entry((package.name.clone(),package.id.to_ascii_lowercase())).or_insert((generation,package));
        }
        self.generation=generation;Ok(())
    }
    fn unchanged(&self,after:&Package)->bool {
        self.packages.get(&(after.name.clone(),after.id.to_ascii_lowercase()))
            .is_some_and(|(_,old)|retired_unchanged(&State{active:vec![old.clone()],..Default::default()},after))
    }
    pub(crate) fn validate_version(&self,version:u64)->Result<(),String> {
        if version==0||version>self.generation{return Err("original domain export exceeds admitted lifecycle generation".into());}
        Ok(())
    }
    pub(crate) fn acknowledge(&mut self,version:u64,desired:&State,current:&Owner)->Result<(),String> {
        self.validate_version(version)?;
        self.packages.retain(|(name,id),(admitted,_)|version<*admitted
            || current.package(name).is_some_and(|p|p.id.eq_ignore_ascii_case(id))
            || desired.active.iter().any(|p|p.name==*name&&p.id.eq_ignore_ascii_case(id)));
        Ok(())
    }
}
pub fn merge(owner:&Owner,before:&State,desired:State)->Result<Owner,String>{
    merge_with_receipts(owner,before,desired,&AdmissionReceipts::default())
}
pub(crate) fn merge_with_receipts(owner:&Owner,before:&State,desired:State,receipts:&AdmissionReceipts)->Result<Owner,String>{
    let current=owner.persisted();let active=owner.attached_names().collect::<BTreeSet<_>>();
    for name in &active{
        let live=owner.package(name).ok_or("native attached domain owner absent")?;
        let after=desired.active.iter().find(|value|value.name==*name);
        // A native installation can commit before the original DVS lifecycle
        // callback attaches it. An older export owns no delta for that package.
        let Some(after)=after else {
            if before.active.iter().any(|value|value.name==*name&&value.id.eq_ignore_ascii_case(&live.id)) {
                return Err(format!("original DVS did not attach native package {name}"));
            }
            continue;
        };
        if !live.id.eq_ignore_ascii_case(&after.id){
            if retired_unchanged(before,after)||receipts.unchanged(after){continue;}
            return Err(format!("original DVS domain UUID differs: {name}"));
        }
    }
    for after in &desired.active {
        if !current.active.iter().any(|p|p.name==after.name) && !retired_unchanged(before,after) && !receipts.unchanged(after) {
            return Err(format!("foreign original DVS package {}",after.name));
        }
    }
    let mut merged=current.clone();
    for live in &mut merged.active{
        let Some(after)=desired.active.iter().find(|value|value.name==live.name)else{continue;};
        if !live.id.eq_ignore_ascii_case(&after.id){
            if retired_unchanged(before,after)||receipts.unchanged(after){continue;}
            return Err(format!("original DVS domain UUID differs: {}",live.name));
        }
        let old=before.active.iter().find(|value|value.name==live.name&&value.id.eq_ignore_ascii_case(&live.id)).unwrap_or(live);
        let replacement=Package{name:live.name.clone(),id:live.id.clone(),
            has_auto_verify_domains:live.has_auto_verify_domains,signature:live.signature.clone(),
            domains:field(&old.domains,&after.domains,&live.domains,&live.name)?,
            users:field(&old.users,&after.users,&live.users,&live.name)?,
            uri_relative_filter_groups:field(&old.uri_relative_filter_groups,&after.uri_relative_filter_groups,&live.uri_relative_filter_groups,&live.name)?,};
        *live=replacement;
    }
    merged.restored=field(&before.restored,&desired.restored,&current.restored,"restored")?;
    merged.legacy=field(&before.legacy,&desired.legacy,&current.legacy,"legacy")?;
    let mut next=owner.clone();next.rebase_persisted_projection(&current,merged,&active)?;Ok(next)
}
pub fn encode(state:&State)->Result<Vec<u8>,String>{
    let root=aim_android_xml::read(b"<packages/>")?;
    let root=crate::package::owner::domains::replace(&root,state)?;
    aim_android_xml::abx::write(&root)
}
pub fn decode(record:&[u8])->Result<State,String>{
    let root=aim_android_xml::abx::read(record)?;
    Ok(crate::package::settings::Settings::parse(&root)?.domain_verification)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{domain_verification::{User,owner::Input},pkg::{AndroidPackage,Activity},
        intent_filter::{IntentFilter,ParsedIntentInfo},system_config::SystemConfig};
    #[test]
    fn admission_without_export_survives_removal_until_matching_absence_acknowledgement() {
        let config=SystemConfig::default();let mut owner=Owner::new(State::default(),Default::default());
        let before=owner.persisted();let mut receipts=AdmissionReceipts::default();receipts.record(1,&owner).unwrap();
        let code=AndroidPackage{package_name:"admitted".into(),..Default::default()};
        owner.add(Input{id:"00000000-0000-0000-0000-000000000001",name:"admitted",code:Some(&code),signatures:&[],system:false,restrict_domains:true,pre_verified:None},&config).unwrap();
        let admitted=owner.persisted();receipts.record(2,&owner).unwrap();
        owner.clear_package("admitted");receipts.record(3,&owner).unwrap();
        // An export made before admission does not acknowledge the later clear.
        receipts.acknowledge(1,&before,&owner).unwrap();
        let delayed=decode(&encode(&admitted).unwrap()).unwrap();
        assert_eq!(merge_with_receipts(&owner,&before,delayed.clone(),&receipts).unwrap().persisted(),owner.persisted());
        assert!(owner.package("admitted").is_none());
        let mut modified=delayed.clone();modified.active[0].users.push(User{id:0,allow_link_handling:false,enabled_hosts:vec![]});
        assert!(merge_with_receipts(&owner,&before,modified,&receipts).is_err());
        let mut foreign=delayed.clone();foreign.active[0].signature=Some("foreign".into());
        assert!(merge_with_receipts(&owner,&before,foreign,&receipts).is_err());
        let mut uuid=delayed.clone();uuid.active[0].id="00000000-0000-0000-0000-000000000002".into();
        assert!(merge_with_receipts(&owner,&before,uuid,&receipts).is_err());
        receipts.acknowledge(3,&owner.persisted(),&owner).unwrap();
        assert!(merge_with_receipts(&owner,&before,delayed,&receipts).is_err());
    }
    #[test]
    fn admission_generations_reject_regression_and_unconfirmed_export_epochs() {
        let owner=Owner::new(State::default(),Default::default());
        let mut receipts=AdmissionReceipts::default();receipts.record(3,&owner).unwrap();
        assert!(receipts.record(2,&owner).is_err());
        assert!(receipts.validate_version(4).is_err());assert!(receipts.validate_version(0).is_err());
        assert!(receipts.validate_version(3).is_ok());
    }
    fn deleted_native_fixture()->(Owner,State) {
        let config=SystemConfig::default();
        let mut owner=Owner::new(State::default(),Default::default());
        for (name,id) in [("removed","00000000-0000-0000-0000-000000000001"),("retained","00000000-0000-0000-0000-000000000002")] {
            let code=AndroidPackage{package_name:name.into(),..Default::default()};
            owner.add(Input{id,name,code:Some(&code),signatures:&[],system:false,restrict_domains:true,pre_verified:None},&config).unwrap();
            owner.set_link_handling_internal(Some(name),false,0,&[0]).unwrap();
        }
        let before=owner.persisted();
        owner.clear_package("removed");
        owner.set_link_handling_internal(Some("retained"),false,10,&[10]).unwrap();
        (owner,before)
    }
    #[test]
    fn queued_original_export_keeps_actual_native_deletion_and_retained_policy() {
        let (owner,before)=deleted_native_fixture();
        let committed=owner.persisted();
        let exported=decode(&encode(&before).unwrap()).unwrap();
        let merged=merge(&owner,&before,exported.clone()).unwrap();
        assert_eq!(merged.persisted(),committed);
        assert!(merged.package("removed").is_none());
        assert_eq!(merged.package("retained").unwrap(),owner.package("retained").unwrap());
        // Repeated old saves do not recreate the deleted attachment.
        assert_eq!(merge(&merged,&exported,exported.clone()).unwrap().persisted(),committed);
        // The acknowledged post-reconcile baseline closes this retired UUID's
        // window; an earlier export cannot regain ownership later.
        assert!(merge(&merged,&committed,exported).is_err());
    }
    #[test]
    fn deleted_original_export_rejects_unknown_name_and_changed_uuid_or_signature() {
        let (owner,before)=deleted_native_fixture();
        for which in 0..3 {
            let mut exported=before.clone();
            let removed=exported.active.iter_mut().find(|p|p.name=="removed").unwrap();
            match which {
                0=>removed.name="foreign".into(),
                1=>removed.id="00000000-0000-0000-0000-000000000003".into(),
                _=>removed.signature=Some("foreign-signer".into()),
            }
            assert!(merge(&owner,&before,exported).is_err());
        }
    }
    #[test]
    fn deleted_original_export_rejects_retired_user_domain_and_group_edits() {
        let (owner,before)=deleted_native_fixture();
        for which in 0..4 {
            let mut exported=before.clone();
            let removed=exported.active.iter_mut().find(|p|p.name=="removed").unwrap();
            match which {
                0=>removed.users[0].allow_link_handling=true,
                1=>removed.domains.push((Some("edited.example".into()),2)),
                2=>removed.uri_relative_filter_groups.push((Some("edited.example".into()),Vec::new())),
                _=>removed.has_auto_verify_domains=true,
            }
            assert!(merge(&owner,&before,exported).is_err());
        }
    }
    #[test]
    fn queued_original_export_preserves_migrated_uuid_and_rejects_retired_edits() {
        let config=SystemConfig::default();
        let code=AndroidPackage{package_name:"updated".into(),..Default::default()};
        let old="00000000-0000-0000-0000-000000000001";
        let new="00000000-0000-0000-0000-000000000002";
        let mut owner=Owner::new(State::default(),Default::default());
        owner.add(Input{id:old,name:"updated",code:Some(&code),signatures:&[],system:false,restrict_domains:true,pre_verified:None},&config).unwrap();
        owner.set_link_handling_internal(Some("updated"),false,0,&[0]).unwrap();
        let before=owner.persisted();
        owner.migrate(old,Some(&code),Input{id:new,name:"updated",code:Some(&code),signatures:&[],system:false,restrict_domains:true,pre_verified:None},&config).unwrap();
        owner.set_link_handling_internal(Some("updated"),false,10,&[10]).unwrap();
        let committed=owner.persisted();
        let merged=merge(&owner,&before,decode(&encode(&before).unwrap()).unwrap()).unwrap();
        assert_eq!(merged.persisted(),committed);
        assert_eq!(merged.package("updated").unwrap().id,new);
        let mut unknown=before.clone();unknown.active[0].id="00000000-0000-0000-0000-000000000003".into();
        assert!(merge(&owner,&before,unknown).is_err());
        let mut edited=before.clone();edited.active[0].users[0].allow_link_handling=true;
        assert!(merge(&owner,&before,edited).is_err());
        let mut verifier=before.clone();verifier.active[0].domains.push((Some("changed.example".into()),2));
        assert!(merge(&owner,&before,verifier).is_err());
        // A genuine lifecycle attachment acknowledges the new UUID. An old
        // queued export cannot regain ownership after that baseline advances.
        assert!(merge(&merged,&committed,before).is_err());
        assert_eq!(merge(&merged,&committed,committed.clone()).unwrap().persisted(),committed);
    }
    #[test]
    fn original_export_before_install_preserves_new_native_attachment() {
        let config=SystemConfig::default();
        let code=AndroidPackage{package_name:"old".into(),..Default::default()};
        let mut owner=Owner::new(State::default(),Default::default());
        owner.add(Input{id:"00000000-0000-0000-0000-000000000001",name:"old",code:Some(&code),signatures:&[],system:false,restrict_domains:true,pre_verified:None},&config).unwrap();
        let before=owner.persisted();
        let new_code=AndroidPackage{package_name:"new".into(),..Default::default()};
        owner.add(Input{id:"00000000-0000-0000-0000-000000000002",name:"new",code:Some(&new_code),signatures:&[],system:false,restrict_domains:true,pre_verified:None},&config).unwrap();
        owner.set_link_handling_internal(Some("new"),false,10,&[10]).unwrap();
        let added=owner.package("new").unwrap().clone();
        let mut desired=before.clone();
        desired.active[0].users.push(User{id:0,allow_link_handling:false,enabled_hosts:vec![]});
        let merged=merge(&owner,&before,desired.clone()).unwrap();
        assert_eq!(merged.package("new").unwrap(),&added);
        assert_eq!(merged.package("old").unwrap().users,desired.active[0].users);
        // Once DVS genuinely attaches the new package, subsequent exports must
        // retain it and carry its exact native UUID.
        let attached=merged.persisted();
        let mut missing=attached.clone();missing.active.retain(|p|p.name!="new");
        assert!(merge(&merged,&attached,missing).is_err());
        let mut foreign=desired;foreign.active.push(Package{name:"foreign".into(),id:added.id.clone(),..added});
        assert!(merge(&owner,&before,foreign).is_err());
    }
    #[test]
    fn original_domain_delta_preserves_new_native_fields_and_rejects_foreign_ids() {
        let id="00000000-0000-0000-0000-00000000000a";
        let mut filter=IntentFilter{auto_verify:true,..Default::default()};
        filter.add_action("android.intent.action.VIEW");filter.add_category("android.intent.category.BROWSABLE");filter.add_category("android.intent.category.DEFAULT");filter.add_data_scheme("https");filter.add_data_authority("example.org",None);
        let mut activity=Activity::default();activity.main.component.intents=vec![ParsedIntentInfo{filter,..Default::default()}];
        let code=AndroidPackage{package_name:"fixture".into(),activities:vec![activity],..Default::default()};
        let saved=State{active:vec![Package{name:"fixture".into(),id:id.into(),has_auto_verify_domains:true,signature:None,
            domains:vec![(Some("example.org".into()),1)],users:vec![User{id:0,allow_link_handling:true,enabled_hosts:vec![]}],uri_relative_filter_groups:vec![]}],..Default::default()};
        let mut owner=Owner::new(saved,Default::default());
        owner.add(Input{id,name:"fixture",code:Some(&code),signatures:&[],system:false,restrict_domains:true,pre_verified:None},&SystemConfig::default()).unwrap();
        let before=owner.persisted();let bytes=encode(&before).unwrap();assert_eq!(decode(&bytes).unwrap(),before);
        owner.set_link_handling_internal(Some("fixture"),false,0,&[0]).unwrap();
        let mut desired=before.clone();desired.active[0].domains[0].1=2;
        let merged=merge(&owner,&before,desired.clone()).unwrap();
        assert_eq!(merged.package("fixture").unwrap().domains[0].1,2);
        assert!(!merged.package("fixture").unwrap().users[0].allow_link_handling);
        assert_eq!(owner.package("fixture").unwrap().domains[0].1,1);
        let mut foreign=desired.clone();foreign.active[0].id="00000000-0000-0000-0000-00000000000b".into();assert!(merge(&owner,&before,foreign).is_err());
        let mut missing=desired.clone();missing.active.clear();assert!(merge(&owner,&before,missing).is_err());
        let mut changed=owner.persisted();changed.active[0].domains[0].1=3;
        let active=owner.attached_names().map(str::to_owned).collect::<Vec<_>>();
        owner.rebase_persisted_projection(&before,changed,&active.iter().map(String::as_str).collect()).unwrap();
        assert!(merge(&owner,&before,desired).is_err());
    }
}
