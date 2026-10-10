//! Actual successful package registration facts for original permission initialization.
use super::SigningScan;
use crate::package::pkg::AndroidPackage;
use aim_binder_host::parcel::Parcel;
use std::collections::BTreeMap;

#[derive(Clone,Debug,PartialEq)]
pub struct Admission {
    pub name:String,
    pub scan_as_instant:bool,
    pub old_package:Option<AndroidPackage>,
}
impl SigningScan {
    /// The current registered view replaces earlier admissions of the same name;
    /// the last receipt still owns its actual pre-replacement package.
    pub fn permission_admissions_record(&self)->Result<Vec<u8>,String>{
        let latest=self.permission_admissions.iter().map(|receipt|(receipt.name.as_str(),receipt)).collect::<BTreeMap<_,_>>();
        let registry=self.package_registry()?;
        let names=registry.ordered_package_names();
        let mut parcel=Parcel::new();parcel.write_i32(1);
        parcel.write_i32(i32::try_from(names.len()).map_err(|_|"permission admission inventory too large")?);
        for name in names {
            let receipt=latest.get(name).ok_or_else(||format!("registered package has no committed permission admission: {name}"))?;
            parcel.write_string16(Some(name));parcel.write_bool(receipt.scan_as_instant);
            let cache=receipt.old_package.as_ref().map(|package|package.to_cache_entry().map(|cache|cache.bytes)).transpose()?;
            aim_service_aidl::write_byte_array(&mut parcel,cache.as_deref());
        }
        Ok(parcel.data().to_vec())
    }
}

/// ScanPackageUtils.adjustScanFlagsWithPackageSetting: a null boot user is USER_SYSTEM.
/// An absent sparse user row uses PackageUserStateDefault.instantApp=false.
pub fn boot_scan_as_instant(users:Option<&BTreeMap<String,BTreeMap<i32,crate::package::restrictions::UserState>>>,name:&str)->bool{
    users.and_then(|users|users.get(name)).and_then(|users|users.get(&0)).is_some_and(|user|user.instant_app)
}
