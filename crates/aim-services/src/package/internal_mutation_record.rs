//! Writable original PackageStateMutator results, before native owner publication.
use aim_binder_host::parcel::{BAD_VALUE, Reader, Result};
use aim_service_aidl::{ReadParcelable, read_long_array, read_string_list};
use super::{model::OverlayPaths, restrictions::{Suspension, SuspendingUser, SuspendParams}, owner::user_runtime::{Component, LabelIcon}};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct Record { pub version: i64, pub active: Vec<Setting>, pub disabled: Vec<Setting> }
#[derive(Clone, Debug)]
pub struct Setting {
    pub name: String, pub factory: bool, pub app_id: i32, pub private_flags: i32,
    pub category: i32, pub page_flags: i32, pub update_available: bool,
    pub loading_progress: f32, pub loading_completed: i64, pub hidden_until_installed: bool,
    pub override_seinfo: Option<String>, pub usage: Vec<i64>, pub installer: Option<String>,
    pub installer_uid: i32, pub update_owner: Option<String>,
    pub mime_groups: Option<Vec<(String, Vec<String>)>>, pub users: Vec<User>,
}
#[derive(Clone, Debug)]
pub struct User {
    pub id: i32, pub installed: bool, pub uninstall_reason: i32, pub distraction_flags: i32,
    pub hidden: bool, pub stopped: bool, pub not_launched: bool,
    pub warning: Option<String>, pub splash: Option<String>, pub min_aspect_ratio: i32, pub overlays: Option<OverlayPaths>,
    pub libraries: Option<Vec<(String, Option<OverlayPaths>)>>,
    pub suspensions: Option<Vec<Suspension>>, pub overrides: Option<Vec<(Component, LabelIcon)>>,
}
fn text(r: &mut Reader<'_>) -> Result<String> { r.read_string16()?.ok_or(BAD_VALUE) }
fn count(r: &mut Reader<'_>) -> Result<usize> {
    let n = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
    if n > r.remaining()/4 { return Err(BAD_VALUE); } Ok(n)
}
fn strings(r: &mut Reader<'_>) -> Result<Vec<String>> {
    read_string_list(r)?.ok_or(BAD_VALUE)?.into_iter().map(|s|s.ok_or(BAD_VALUE)).collect()
}
fn paths(r: &mut Reader<'_>) -> Result<Option<OverlayPaths>> {
    if !r.read_bool()? { return Ok(None); }
    Ok(Some(OverlayPaths {resource_dirs:strings(r)?, overlay_paths:strings(r)?}))
}
fn optional_list<T>(r:&mut Reader<'_>, mut read:impl FnMut(&mut Reader<'_>)->Result<T>)->Result<Option<Vec<T>>> {
    let n=r.read_i32()?; if n == -1 {return Ok(None);} let n=usize::try_from(n).map_err(|_|BAD_VALUE)?;
    if n > r.remaining()/4 {return Err(BAD_VALUE);} (0..n).map(|_|read(r)).collect::<Result<Vec<_>>>().map(Some)
}
fn typed<T:ReadParcelable>(r:&mut Reader<'_>)->Result<Option<T>> {
    match r.read_i32()? {0=>Ok(None),1=>T::read_from(r).map(Some),_=>Err(BAD_VALUE)}
}
impl Record {
    pub fn read(bytes:&[u8])->Result<Self> {
        let mut r=Reader::new(bytes,&[]); if r.read_i32()? != 2 {return Err(BAD_VALUE);}
        let version=r.read_i64()?; if version <= 0 {return Err(BAD_VALUE);}
        let active=Self::settings(&mut r,false)?; let disabled=Self::settings(&mut r,true)?;
        if r.remaining()!=0 {return Err(BAD_VALUE);} Ok(Self{version,active,disabled})
    }
    fn settings(r:&mut Reader<'_>, factory:bool)->Result<Vec<Setting>> {
        let n=count(r)?; let mut names=BTreeSet::new(); let mut settings=Vec::with_capacity(n);
        for _ in 0..n {
            let name=text(r)?; if !names.insert(name.clone()) || r.read_bool()? != factory {return Err(BAD_VALUE);}
            let app_id=r.read_i32()?; let private_flags=r.read_i32()?; let category=r.read_i32()?;
            let page_flags=r.read_i32()?; let update_available=r.read_bool()?;
            let loading_progress=f32::from_bits(r.read_i32()? as u32); let loading_completed=r.read_i64()?;
            let hidden_until_installed=r.read_bool()?; let override_seinfo=r.read_string16()?;
            let usage=read_long_array(r)?.ok_or(BAD_VALUE)?;
            if usage.len()!=super::owner::usage::REASONS {return Err(BAD_VALUE);}
            let installer=r.read_string16()?; let installer_uid=r.read_i32()?; let update_owner=r.read_string16()?;
            let mut mime_names=BTreeSet::new();
            let mime_groups=optional_list(r,|r| {let name=text(r)?;if !mime_names.insert(name.clone()){return Err(BAD_VALUE);}Ok((name,strings(r)?))})?;
            let n=count(r)?; let mut users=Vec::with_capacity(n); let mut previous=-1;
            for _ in 0..n {
                let id=r.read_i32()?; if id<=previous {return Err(BAD_VALUE);} previous=id;
                let installed=r.read_bool()?; let uninstall_reason=r.read_i32()?; let distraction_flags=r.read_i32()?;
                let hidden=r.read_bool()?; let stopped=r.read_bool()?; let not_launched=r.read_bool()?;
                let warning=r.read_string16()?; let splash=r.read_string16()?; let min_aspect_ratio=r.read_i32()?; let overlays=paths(r)?;
                let mut library_names=BTreeSet::new();
                let libraries=optional_list(r,|r| {let name=text(r)?;if !library_names.insert(name.clone()){return Err(BAD_VALUE);}Ok((name,paths(r)?))})?;
                let mut suspenders=BTreeSet::new();
                let suspensions=optional_list(r,|r| {
                    let package=text(r)?;let user=r.read_i32()?;
                    if !suspenders.insert((package.clone(),user)){return Err(BAD_VALUE);}
                    let params=if r.read_bool()? {Some(SuspendParams {dialog:typed(r)?,app_extras:typed(r)?,launcher_extras:typed(r)?,quarantined:r.read_bool()?})}else{None};
                    Ok(Suspension{package,user:SuspendingUser::Resolved(user),params})
                })?;
                let mut components=BTreeSet::new();
                let overrides=optional_list(r,|r| {
                    let package=text(r)?;let class=text(r)?;
                    if !components.insert((package.clone(),class.clone())) {return Err(BAD_VALUE);}
                    let label=r.read_string16()?;let icon=if r.read_bool()?{Some(r.read_i32()?)}else{None};
                    Ok((Component{package,class},LabelIcon{label,icon}))
                })?;
                users.push(User{id,installed,uninstall_reason,distraction_flags,hidden,stopped,not_launched,warning,splash,min_aspect_ratio,overlays,libraries,suspensions,overrides});
            }
            settings.push(Setting{name,factory,app_id,private_flags,category,page_flags,update_available,loading_progress,loading_completed,hidden_until_installed,override_seinfo,usage,installer,installer_uid,update_owner,mime_groups,users});
        }
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::Parcel;
    #[test]
    fn schema_two_preserves_aspect_ratio_before_nullable_runtime_fields() {
        let mut p = Parcel::new();
        p.write_i32(2); p.write_i64(1); p.write_i32(1);
        p.write_string16(Some("package")); p.write_bool(false);
        for value in [10100, 0, -1, 0] { p.write_i32(value); }
        p.write_bool(false); p.write_i32(1.0f32.to_bits() as i32); p.write_i64(0);
        p.write_bool(false); p.write_string16(None);
        aim_service_aidl::write_long_array(&mut p, Some(&[0; super::super::owner::usage::REASONS]));
        p.write_string16(None); p.write_i32(-1); p.write_string16(None);
        p.write_i32(-1); p.write_i32(1); p.write_i32(10);
        p.write_bool(true); p.write_i32(0); p.write_i32(0);
        p.write_bool(false); p.write_bool(false); p.write_bool(false);
        p.write_string16(None); p.write_string16(Some("theme")); p.write_i32(7);
        p.write_bool(false); p.write_i32(-1); p.write_i32(-1); p.write_i32(-1);
        p.write_i32(0);
        let record = Record::read(p.data()).unwrap();
        let user = &record.active[0].users[0];
        assert_eq!(user.min_aspect_ratio, 7);
        assert_eq!(user.splash.as_deref(), Some("theme"));
        assert!(user.overlays.is_none());
        let mut old = p.data().to_vec(); old[..4].copy_from_slice(&1i32.to_le_bytes());
        assert_eq!(Record::read(&old).unwrap_err(), BAD_VALUE);
    }
}
