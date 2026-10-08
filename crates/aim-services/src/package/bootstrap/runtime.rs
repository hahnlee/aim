//! Settings runtime maps projected from the current permission producer.
//! Ported from Android 16 Settings, Copyright (C) The Android Open Source
//! Project, Apache License 2.0.
use super::Bridge;
use crate::package::{
    permissions::{RuntimePermission, RuntimePermissions},
    scan_snapshot::query_state::Capture,
};

impl Bridge {
    /// Version/fingerprint belong to the persistence owner. Live permission
    /// appId states are fetched here, never substituted with saved migration
    /// records. UID-less settings retain their scoped SettingBase state instead.
    pub fn runtime_permissions(
        &self,
        capture: &Capture,
        user: i32,
        version: i32,
        fingerprint: Option<String>,
    ) -> Result<RuntimePermissions, String> {
        let model = capture.state();
        if user < 0 || !model.users.contains_key(&user) {
            return Err("runtime permission user is outside the captured inventory".into());
        }
        let current = |app_id| -> Result<Vec<RuntimePermission>, String> {
            let state = self
                .legacy_permissions(app_id, &[user])
                .map_err(|error| format!("live runtime permission owner: {error:?}"))?;
            let state = state
                .user(user)
                .ok_or("missing live runtime permission user")?;
            Ok(state
                .permissions
                .iter()
                .map(|p| RuntimePermission {
                    name: p.name.clone(),
                    granted: p.granted,
                    flags: p.flags,
                })
                .collect())
        };
        let mut packages: Vec<_> = capture.scan().owner().settings.packages.iter().collect();
        packages.sort_by_key(|p| crate::package::info::java_hash(&p.name));
        let mut output = RuntimePermissions {
            version,
            fingerprint,
            ..Default::default()
        };
        for setting in packages {
            let ps = model
                .packages
                .get(&setting.name)
                .ok_or("missing current package runtime role")?;
            if ps.shared_user.is_some() {
                continue;
            }
            let permissions = setting_permissions(ps.app_id,user,||{
                capture.scan().owner().validated_legacy_permissions(&ps.name,false)?
                    .ok_or_else(||format!("missing UID-less SettingBase permission owner: {}",ps.name))
            },&current)?;
            if !permissions.is_empty() || ps.is.install_permissions_fixed {
                output.packages.push((Some(ps.name.clone()), permissions));
            }
        }
        let mut groups: Vec<_> = model.shared_users.values().collect();
        groups.sort_by_key(|g| crate::package::info::java_hash(&g.name));
        for group in groups {
            output
                .shared_users
                .push((Some(group.name.clone()), current(group.app_id)?));
        }
        Ok(output)
    }
}

/// Settings.RuntimePermissionPersistence reads each SettingBase's own legacy
/// map. Process.INVALID_UID settings (APEX containers and UID-less SDK libraries)
/// have no live PermissionService appId slot. Their detached constructor/scan
/// owner remains authoritative; regular/system UIDs always use the live service.
fn setting_permissions(
    app_id:i32,user:i32,detached:impl FnOnce()->Result<crate::package::owner::legacy_permissions::State,String>,
    live:&impl Fn(i32)->Result<Vec<RuntimePermission>,String>,
)->Result<Vec<RuntimePermission>,String>{
    if app_id!=-1{return live(app_id);}
    let state=detached()?;
    if state.app_id()!=app_id{return Err("UID-less permission SettingBase identity differs".into());}
    let state=state.user(user).ok_or("UID-less permission user is not captured")?;
    Ok(state.permissions.iter().map(|permission|RuntimePermission{name:permission.name.clone(),granted:permission.granted,flags:permission.flags}).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::owner::legacy_permissions::Migration;
    #[test]
    fn uidless_runtime_projection_uses_scoped_setting_and_keeps_live_uid_boundary(){
        let detached=Migration::default().project(-1,&[0]).unwrap();
        let values=setting_permissions(-1,0,||Ok(detached),&|_|panic!("UID-less setting must not query live application permission slot")).unwrap();
        assert!(values.is_empty());
        for app_id in [1000,10100]{
            let values=setting_permissions(app_id,0,||panic!("positive UID must not substitute saved SettingBase state"),&|called|{
                assert_eq!(called,app_id);Ok(vec![RuntimePermission{name:Some("fixture.live".into()),granted:true,flags:3}])
            }).unwrap();
            assert_eq!(values[0].name.as_deref(),Some("fixture.live"));assert_eq!(values[0].flags,3);
        }
        assert!(setting_permissions(10100,0,||panic!("live failure must not use saved state"),&|_|Err("live owner failed".into())).is_err());
        assert!(setting_permissions(-1,10,||Ok(Migration::default().project(-1,&[0]).unwrap()),&|_|panic!("no live UID-less fallback")).is_err());
        assert!(setting_permissions(-1,0,||Ok(Migration::default().project(10100,&[0]).unwrap()),&|_|panic!("identity mismatch must fail")).is_err());
    }
}
