//! Serialization belongs to the immutable registered runtime code, not Settings
//! generations. Raw publicly mutable parse inputs deliberately remain uncached.
use crate::package::{pkg::{AndroidPackage,FacadeEntry},sign};
use std::sync::{Arc,Mutex};
#[derive(Clone)]
pub struct Projection {
    pub package:Arc<AndroidPackage>,
    pub facade:Arc<FacadeEntry>,
    pub parcel:Arc<[u8]>,
}
#[derive(Clone,Default)]
pub(super) struct Cache(Arc<Mutex<Option<(sign::SigningDetails,Projection)>>>);
impl std::fmt::Debug for Cache {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("RegisteredCodeProjection")}
}
impl PartialEq for Cache {fn eq(&self,_:&Self)->bool{true}}
fn build(package:Arc<AndroidPackage>,signing:&sign::SigningDetails)->Result<Projection,String>{
    let facade=Arc::new(package.to_facade_entry(signing)?);
    let parcel=Arc::from(facade.cache.bytes.as_slice());
    Ok(Projection{package,facade,parcel})
}
impl Cache {
    pub(super) fn get(&self,package:&Arc<AndroidPackage>,signing:&sign::SigningDetails)->Result<Projection,String>{
        let mut current=self.0.lock().unwrap();
        if let Some((captured,value))=current.as_ref() {
            if captured==signing {return Ok(value.clone());}
        }
        let value=build(package.clone(),signing)?;
        *current=Some((signing.clone(),value.clone()));
        Ok(value)
    }
}
impl super::LoadedPackage {
    pub fn code_projection(&self)->Result<Projection,String>{
        match &self.runtime_view {
            Some(package)=>self.projection.get(package,&self.collected_signing),
            None=>build(Arc::new(self.package.clone()),&self.collected_signing),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::scan::LoadedPackage;
    fn package()->AndroidPackage {
        AndroidPackage {package_name:"fixture.code".into(),uid:10100,
            path:Some("/data/app/fixture.code".into()),base_apk_path:Some("/data/app/fixture.code/base.apk".into()),
            feature_flag_state:Some(vec![]),..Default::default()}
    }
    #[test]
    fn immutable_registered_code_reuses_projection_and_forks_on_code_changes() {
        let original=package();let signing=sign::SigningDetails::unknown();
        let mut loaded=Arc::new(LoadedPackage::new(original.clone(),signing.clone()).unwrap());
        Arc::get_mut(&mut loaded).unwrap().set_runtime_package(original.clone());
        let first=loaded.code_projection().unwrap();let alias=loaded.clone();
        let second=alias.code_projection().unwrap();let owned_clone=(*loaded).clone().code_projection().unwrap();
        assert!(Arc::ptr_eq(&first.facade,&second.facade));assert!(Arc::ptr_eq(&first.parcel,&second.parcel));
        assert!(Arc::ptr_eq(&first.package,&second.package));assert!(Arc::ptr_eq(&first.facade,&owned_clone.facade));
        assert_eq!(first.parcel.as_ref(),original.to_cache_entry().unwrap().bytes.as_slice());
        let mut updated=original.clone();updated.version_name=Some("updated".into());updated.min_aspect_ratio=-0.0;
        let mut changed=loaded.clone();Arc::make_mut(&mut changed).set_runtime_package(updated.clone());
        let replacement=changed.code_projection().unwrap();
        assert!(!Arc::ptr_eq(&first.facade,&replacement.facade));assert!(!Arc::ptr_eq(&first.package,&replacement.package));
        assert_eq!(replacement.parcel.as_ref(),updated.to_cache_entry().unwrap().bytes.as_slice());
        assert_ne!(first.parcel.as_ref(),replacement.parcel.as_ref());
        assert_eq!(replacement.package.min_aspect_ratio.to_bits(),(-0.0f32).to_bits());
        assert!(Arc::ptr_eq(&first.facade,&loaded.code_projection().unwrap().facade));
        // Signing is independently owned and still validated on a cache hit.
        Arc::make_mut(&mut changed).collected_signing.scheme_version=3;
        assert!(changed.code_projection().is_err());
        // Public mutable raw parse inputs have no immutable-cache authority.
        let mut raw=LoadedPackage::new(original.clone(),signing).unwrap();
        let before=raw.code_projection().unwrap();raw.package.version_name=Some("raw update".into());
        let after=raw.code_projection().unwrap();
        assert_ne!(before.parcel.as_ref(),after.parcel.as_ref());
        assert_eq!(after.parcel.as_ref(),raw.package.to_cache_entry().unwrap().bytes.as_slice());
        raw.package.feature_flag_state=None;assert!(raw.code_projection().is_err());
    }
}
