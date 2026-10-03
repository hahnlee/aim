//! Captured PackageUserState inputs from persisted and runtime owners.
//! The complete interface adapter and live producers remain under #836.
use super::Snapshot;
use crate::package::restrictions::{SuspendingUser, UserState};
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::write_byte_array;

pub fn captured(
    snapshot: &Snapshot,
    name: &str,
    disabled: bool,
    user: i32,
) -> Result<Option<Vec<u8>>, String> {
    if user < 0 {
        return Err("user state requires a nonnegative user id".into());
    }
    let owner = snapshot.owner();
    let settings = if disabled {
        &owner.settings.disabled_system_packages
    } else {
        &owner.settings.packages
    };
    let Some(setting) = settings.iter().find(|p| p.name == name) else {
        return Ok(None);
    };
    let users = if disabled {
        owner.disabled_user_states(name)
    } else {
        owner.scanned_user_states(name)
    }
    .ok_or("package user-state owner is not captured")?;
    // PackageSetting.readUserState returns the original default when its
    // existing setting has no explicit entry for this user.
    let default = UserState::default();
    let state = users.get(&user).unwrap_or(&default);
    let mut p = Parcel::new();
    p.write_i64(snapshot.version() as i64);
    p.write_string16(Some(name));
    p.write_i32(setting.app_id);
    p.write_bool(disabled);
    p.write_i32(user);
    write(&mut p, state)?;
    if p.position() > i32::MAX as usize {
        return Err("user state exceeds transport range".into());
    }
    Ok(Some(p.data().to_vec()))
}

fn count(p: &mut Parcel, n: usize) -> Result<(), String> {
    p.write_i32(i32::try_from(n).map_err(|_| "user state count exceeds transport range")?);
    Ok(())
}

fn strings(p: &mut Parcel, values: &Option<Vec<String>>) -> Result<(), String> {
    let Some(values) = values else {
        p.write_i32(-1);
        return Ok(());
    };
    count(p, values.len())?;
    for value in values {
        p.write_string16(Some(value));
    }
    Ok(())
}

fn write(p: &mut Parcel, state: &UserState) -> Result<(), String> {
    p.write_i64(state.ce_data_inode);
    p.write_i64(state.de_data_inode);
    for value in [
        state.installed,
        state.stopped,
        state.not_launched,
        state.hidden,
    ] {
        p.write_bool(value);
    }
    p.write_i32(state.distraction_flags);
    p.write_bool(state.instant_app);
    p.write_bool(state.virtual_preload);
    p.write_i32(state.enabled);
    p.write_string16(state.last_disable_app_caller.as_deref());
    strings(p, &state.enabled_components)?;
    strings(p, &state.disabled_components)?;
    p.write_i32(state.install_reason);
    p.write_i32(state.uninstall_reason);
    p.write_string16(state.harmful_app_warning.as_deref());
    p.write_string16(state.splash_screen_theme.as_deref());
    p.write_i64(state.first_install_time);
    p.write_i32(state.min_aspect_ratio);
    p.write_i32(state.domain_verification_status);
    if let Some(suspensions) = &state.suspensions {
        count(p, suspensions.len())?;
    } else {
        p.write_i32(-1);
    }
    for suspension in state.suspensions.iter().flatten() {
        p.write_string16(Some(&suspension.package));
        p.write_bool(matches!(suspension.user, SuspendingUser::Current));
        let user = match suspension.user {
            SuspendingUser::Current => None,
            SuspendingUser::Persisted(user) => user,
        };
        p.write_bool(user.is_some());
        if let Some(user) = user {
            p.write_i32(user);
        }
        p.write_bool(suspension.quarantined);
        p.write_bool(suspension.dialog.is_some());
        if let Some(d) = &suspension.dialog {
            p.write_i32(d.icon);
            p.write_i32(d.title_resource);
            p.write_string16(d.title.as_deref());
            p.write_i32(d.message_resource);
            p.write_string16(d.message.as_deref());
            p.write_i32(d.button_resource);
            p.write_string16(d.button.as_deref());
            p.write_i32(d.button_action);
        }
        for bundle in [&suspension.app_extras, &suspension.launcher_extras] {
            let parcel = bundle.as_ref().map(|b| b.parcel()).transpose()?;
            write_byte_array(p, parcel.as_ref().map(|p| p.data()));
        }
    }
    p.write_bool(state.archive_state.is_some());
    if let Some(archive) = &state.archive_state {
        p.write_string16(Some(&archive.installer_title));
        p.write_i64(archive.archive_time);
        count(p, archive.activities.len())?;
        for activity in &archive.activities {
            p.write_string16(Some(&activity.title));
            p.write_string16(Some(&activity.original_component_name));
            p.write_string16(Some(&activity.icon_path));
            p.write_string16(activity.monochrome_icon_path.as_deref());
        }
    }
    let runtime = &state.runtime;
    paths(p, runtime.overlays())?;
    p.write_bool(runtime.libraries().is_some());
    if let Some(libraries) = runtime.libraries() {
        count(p, libraries.len())?;
        for (name, overlay) in libraries {
            p.write_string16(Some(name));
            paths(p, Some(overlay))?;
        }
    }
    p.write_bool(runtime.overrides().is_some());
    if let Some(overrides) = runtime.overrides() {
        count(p, overrides.len())?;
        for (component, value) in overrides {
            p.write_string16(Some(&component.package));
            p.write_string16(Some(&component.class));
            p.write_string16(value.label.as_deref());
            p.write_bool(value.icon.is_some());
            if let Some(icon) = value.icon {
                p.write_i32(icon);
            }
        }
    }
    Ok(())
}
fn paths(
    p: &mut Parcel,
    paths: Option<&crate::package::model::OverlayPaths>,
) -> Result<(), String> {
    p.write_bool(paths.is_some());
    if let Some(paths) = paths {
        for values in [&paths.resource_dirs, &paths.overlay_paths] {
            count(p, values.len())?;
            for value in values {
                p.write_string16(Some(value));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nullable_component_owners_keep_default_and_initialized_states_distinct() {
        let default = UserState::default();
        let initialized = UserState::initialized();
        assert_ne!(default, initialized);
        let mut empty_suspensions = default.clone();
        empty_suspensions.suspensions = Some(vec![]);
        assert!(!empty_suspensions.is_quarantined(0, true));
        let mut absent = Parcel::new();
        let mut allocated = Parcel::new();
        write(&mut absent, &default).unwrap();
        write(&mut allocated, &empty_suspensions).unwrap();
        assert_ne!(absent.data(), allocated.data());
        for (values, expected) in [
            (None, -1),
            (Some(vec![]), 0),
            (Some(vec!["fixture.Activity".to_string()]), 1),
        ] {
            let mut p = Parcel::new();
            strings(&mut p, &values).unwrap();
            let mut r = aim_binder_host::parcel::Reader::new(p.data(), &[]);
            assert_eq!(r.read_i32().unwrap(), expected);
            if expected == 1 {
                assert_eq!(
                    r.read_string16().unwrap().as_deref(),
                    Some("fixture.Activity")
                );
            }
            assert_eq!(r.remaining(), 0);
        }
        for xml in [
            "<pkg name=\"fixture\"/>",
            "<pkg name=\"fixture\"><enabled-components/><disabled-components/></pkg>",
        ] {
            let root = aim_android_xml::read(
                format!("<package-restrictions>{xml}</package-restrictions>").as_bytes(),
            )
            .unwrap();
            let parsed = crate::package::restrictions::Restrictions::parse(&root).unwrap();
            assert_eq!(parsed.packages[0].1, initialized);
        }
    }
}
